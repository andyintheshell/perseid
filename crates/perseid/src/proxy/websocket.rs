//! WebSocket frame relay and message capture.
//!
//! When interception is off the proxy forwards every byte unchanged. A small incremental parser observes the
//! byte stream, unmasks client payloads, reassembles fragmented messages, and
//! emits one bounded [`ProxyEvent::WebSocketMessage`] per complete text or
//! binary message. Intercepted messages can be edited and reframed. Control
//! frames are relayed but do not appear in history.
//!
//! `Sec-WebSocket-Extensions` is removed from the handshake so origins do not
//! negotiate compression. Keeping forwarded frames readable is more useful for
//! inspection than preserving an extension the history cannot decode.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use log::{debug, warn};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use crate::proxy::events::{
    CapturedBody, ProxyEvent, WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
};
use crate::proxy::intercept::{WebSocketInterceptDecision, WebSocketInterceptItem};
use crate::proxy::server::Ctx;
use tokio::sync::oneshot;

/// Maximum bytes read from either side before the parser sees them.
const READ_BUFFER: usize = 16 * 1024;
const SEND_QUEUE: usize = 16;

type SenderPair = (mpsc::Sender<Outgoing>, mpsc::Sender<Outgoing>);
pub(crate) type Connections = Arc<RwLock<HashMap<u64, SenderPair>>>;

pub(crate) struct Outgoing {
    opcode: WebSocketOpcode,
    payload: Vec<u8>,
}

/// Failure to queue a message for a live WebSocket connection.
#[derive(Debug, thiserror::Error)]
pub enum WebSocketSendError {
    #[error("WebSocket connection is not open")]
    Closed,
    #[error("WebSocket send queue is full")]
    Full,
    #[error("WebSocket message exceeds the send limit")]
    TooLarge,
    #[error("WebSocket text messages must be UTF-8")]
    InvalidText,
}

pub(crate) fn send(
    connections: &Connections,
    limit: usize,
    id: u64,
    direction: WebSocketDirection,
    opcode: WebSocketOpcode,
    payload: Vec<u8>,
) -> Result<(), WebSocketSendError> {
    if payload.len() > limit {
        return Err(WebSocketSendError::TooLarge);
    }
    if opcode == WebSocketOpcode::Text && std::str::from_utf8(&payload).is_err() {
        return Err(WebSocketSendError::InvalidText);
    }
    let connections = connections.read().unwrap_or_else(|e| e.into_inner());
    let pair = connections.get(&id).ok_or(WebSocketSendError::Closed)?;
    let sender = match direction {
        WebSocketDirection::ToServer => &pair.0,
        WebSocketDirection::ToClient => &pair.1,
    };
    sender
        .try_send(Outgoing { opcode, payload })
        .map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => WebSocketSendError::Full,
            mpsc::error::TrySendError::Closed(_) => WebSocketSendError::Closed,
        })
}

struct Registration {
    connections: Connections,
    id: u64,
    stopped: watch::Sender<bool>,
}
impl Registration {
    fn stop(&self) {
        self.connections
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.stopped.send_replace(true);
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.stop();
    }
}

fn encode_message(opcode: WebSocketOpcode, payload: &[u8], masked: bool) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(
        0x80 | match opcode {
            WebSocketOpcode::Text => 1,
            WebSocketOpcode::Binary => 2,
        },
    );
    let flag = if masked { 0x80 } else { 0 };
    match payload.len() {
        n @ 0..=125 => frame.push(flag | n as u8),
        n @ 126..=65535 => {
            frame.push(flag | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(flag | 127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    if masked {
        let random = uuid::Uuid::new_v4();
        let key = &random.as_bytes()[..4];
        frame.extend_from_slice(key);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(i, byte)| byte ^ key[i % 4]),
        );
    } else {
        frame.extend_from_slice(payload);
    }
    frame
}

/// Relay a WebSocket connection in both directions until both halves close.
///
/// Traffic is always forwarded. Capture state is checked for each completed
/// message, so pausing the proxy skips messages without ending the connection;
/// resuming it starts recording the next message.
pub(crate) async fn relay<A, B>(
    ctx: Arc<Ctx>,
    connection: Arc<WebSocketConnectionInfo>,
    client: A,
    server: B,
) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut server_read, mut server_write) = tokio::io::split(server);
    let (up_tx, up_rx) = mpsc::channel(SEND_QUEUE);
    let (down_tx, down_rx) = mpsc::channel(SEND_QUEUE);
    ctx.websocket_connections
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(connection.id, (up_tx, down_tx));
    let (stopped, stop_rx) = watch::channel(false);
    let registration = Registration {
        connections: ctx.websocket_connections.clone(),
        id: connection.id,
        stopped,
    };

    let upstream = pump(
        &ctx,
        &connection,
        &mut client_read,
        &mut server_write,
        WebSocketDirection::ToServer,
        up_rx,
        (&registration, stop_rx.clone()),
    );
    let downstream = pump(
        &ctx,
        &connection,
        &mut server_read,
        &mut client_write,
        WebSocketDirection::ToClient,
        down_rx,
        (&registration, stop_rx),
    );

    tokio::try_join!(upstream, downstream)
}

// Bound fragmentation and interleaved control-frame overhead as well as payload.
const INTERCEPT_WIRE_OVERHEAD: usize = 64 * 1024;

/// Read one complete message, retaining its original wire representation so an
/// unchanged forward preserves fragmentation, masks and control-frame order.
async fn read_held_message<R: AsyncRead + Unpin>(
    reader: &mut R,
    first: u8,
    direction: WebSocketDirection,
    limit: usize,
) -> io::Result<(Vec<u8>, Vec<u8>, Option<(WebSocketOpcode, Vec<u8>)>, bool)> {
    let mut decoder = FrameDecoder::new(limit);
    let mut wire = Vec::new();
    let mut controls = Vec::new();
    let mut first_byte = first;
    let mut completed = None;
    loop {
        let second = reader.read_u8().await?;
        let masked = second & 0x80 != 0;
        if masked != (direction == WebSocketDirection::ToServer) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid WebSocket masking direction",
            ));
        }
        let ext = match second & 0x7f {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        let mut header = vec![first_byte, second];
        header.resize(2 + ext + if masked { 4 } else { 0 }, 0);
        reader.read_exact(&mut header[2..]).await?;
        let len = match ext {
            2 => u16::from_be_bytes([header[2], header[3]]) as u64,
            8 => u64::from_be_bytes(header[2..10].try_into().unwrap()),
            _ => u64::from(second & 0x7f),
        };
        let wire_limit = limit.saturating_add(INTERCEPT_WIRE_OVERHEAD);
        let len = usize::try_from(len)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "WebSocket frame too large"))?;
        if len > limit || wire.len().saturating_add(header.len()).saturating_add(len) > wire_limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "WebSocket interception limit exceeded",
            ));
        }
        let start = wire.len();
        wire.extend_from_slice(&header);
        wire.resize(wire.len() + len, 0);
        reader.read_exact(&mut wire[start + header.len()..]).await?;
        decoder
            .feed(&wire[start..], &mut |opcode, body| {
                completed = Some((opcode, body.data().to_vec(), body.is_truncated()));
            })
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if first_byte & 0x0f >= 8 {
            controls.extend_from_slice(&wire[start..]);
        }
        if let Some((opcode, payload, truncated)) = completed.take() {
            if truncated {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "WebSocket interception limit exceeded",
                ));
            }
            if opcode == WebSocketOpcode::Text && std::str::from_utf8(&payload).is_err() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid WebSocket text",
                ));
            }
            return Ok((wire, controls, Some((opcode, payload)), false));
        }
        if decoder.closed || decoder.at_message_boundary() {
            return Ok((wire, controls, None, decoder.closed));
        }
        first_byte = reader.read_u8().await?;
    }
}

async fn pump<R, W>(
    ctx: &Ctx,
    connection: &WebSocketConnectionInfo,
    reader: &mut R,
    writer: &mut W,
    direction: WebSocketDirection,
    mut outgoing: mpsc::Receiver<Outgoing>,
    status: (&Registration, watch::Receiver<bool>),
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (registration, mut stopped) = status;
    let mut decoder = FrameDecoder::new(ctx.capture_limit);
    let mut buffer = [0u8; READ_BUFFER];
    let mut total = 0u64;
    let mut pending_byte = None;
    // Remove the connection on any early return, including an I/O error.
    struct StopOnDrop<'a>(&'a Registration);
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.stop();
        }
    }
    let _stop_on_drop = StopOnDrop(registration);

    loop {
        // Switch only at a message boundary; the passive decoder may already
        // be partway through a message when the policy is changed.
        let boundary = decoder.at_message_boundary();
        let n = if let Some(byte) = pending_byte.take() {
            buffer[0] = byte;
            1
        } else {
            tokio::select! {
                biased;
                result = stopped.changed(), if !*stopped.borrow() => {
                    if result.is_ok() && *stopped.borrow() {
                        outgoing.close();
                    }
                    continue;
                }
                message = outgoing.recv(), if !*stopped.borrow() && decoder.at_message_boundary() => {
                    if let Some(message) = message {
                        let frame = encode_message(message.opcode, &message.payload, direction == WebSocketDirection::ToServer);
                        writer.write_all(&frame).await?;
                        total += frame.len() as u64;
                        if ctx.capture_enabled() {
                            ctx.emit(ProxyEvent::WebSocketMessage { id: ctx.next_id(), timestamp: SystemTime::now(), connection: connection.clone(), direction, opcode: message.opcode, payload: CapturedBody::from_bytes_limited(&message.payload, ctx.capture_limit) });
                        }
                    }
                    continue;
                }
                result = reader.read(&mut buffer[..if boundary { 1 } else { READ_BUFFER }]) => result?,
            }
        };
        if n == 0 {
            break;
        }
        if boundary && !*stopped.borrow() && ctx.policy().intercept_requests() {
            let held = tokio::select! {
                result = read_held_message(reader, buffer[0], direction, ctx.intercept_limit) => result?,
                _ = stopped.changed() => break,
            };
            let (wire, controls, message, closed) = held;
            if let Some((opcode, original)) = message {
                let (tx, rx) = oneshot::channel();
                let item = WebSocketInterceptItem::new(
                    ctx.next_id(),
                    connection.clone(),
                    direction,
                    opcode,
                    original.clone(),
                    ctx.intercept_limit,
                    tx,
                );
                let sent = tokio::select! {
                    result = ctx.websocket_intercepts.send(item) => result.is_ok(),
                    _ = stopped.changed() => false,
                };
                if !sent {
                    break;
                }
                ctx.wake();
                let mut rx = rx;
                let decision = loop {
                    tokio::select! {
                        result = &mut rx => break result.ok(),
                        _ = stopped.changed() => break None,
                        result = reader.read(&mut buffer[..1]), if pending_byte.is_none() => {
                            match result? {
                                0 => break None,
                                _ if buffer[0] == 0x88 => {
                                    let (close, _, _, _) = read_held_message(reader, buffer[0], direction, ctx.intercept_limit).await?;
                                    writer.write_all(&close).await?;
                                    total += close.len() as u64;
                                    registration.stop();
                                    break None;
                                }
                                _ => pending_byte = Some(buffer[0]),
                            }
                        }
                    }
                };
                match decision {
                    Some(WebSocketInterceptDecision::Forward(payload)) => {
                        if payload == original {
                            writer.write_all(&wire).await?;
                            total += wire.len() as u64;
                        } else {
                            let frame = encode_message(
                                opcode,
                                &payload,
                                direction == WebSocketDirection::ToServer,
                            );
                            writer.write_all(&frame).await?;
                            writer.write_all(&controls).await?;
                            total += (frame.len() + controls.len()) as u64;
                        }
                        if ctx.capture_enabled() {
                            ctx.emit(ProxyEvent::WebSocketMessage {
                                id: ctx.next_id(),
                                timestamp: SystemTime::now(),
                                connection: connection.clone(),
                                direction,
                                opcode,
                                payload: CapturedBody::from_bytes_limited(
                                    &payload,
                                    ctx.capture_limit,
                                ),
                            });
                        }
                    }
                    Some(WebSocketInterceptDecision::Drop) => {
                        writer.write_all(&controls).await?;
                        total += controls.len() as u64;
                    }
                    None => break,
                }
            } else {
                writer.write_all(&wire).await?;
                total += wire.len() as u64;
                if closed {
                    registration.stop();
                    outgoing.close();
                }
            }
            continue;
        }
        // Forward first, then observe. A parser error must never prevent the
        // bytes from reaching the other side.
        writer.write_all(&buffer[..n]).await?;
        total += n as u64;
        if !decoder.is_disabled()
            && let Err(err) = decoder.feed(&buffer[..n], &mut |opcode, payload| {
                if ctx.capture_enabled() {
                    ctx.emit(ProxyEvent::WebSocketMessage {
                        id: ctx.next_id(),
                        timestamp: SystemTime::now(),
                        connection: connection.clone(),
                        direction,
                        opcode,
                        payload,
                    });
                }
            })
        {
            warn!("WebSocket capture disabled after a framing error: {err}");
            decoder.disable();
        }
        if decoder.closed || decoder.is_disabled() {
            registration.stop();
            outgoing.close();
        }
    }

    let _ = writer.shutdown().await;
    debug!(
        "WebSocket {} {} closed after {total} bytes",
        connection.id,
        match direction {
            WebSocketDirection::ToServer => "client->server",
            WebSocketDirection::ToClient => "server->client",
        }
    );
    Ok(total)
}

struct Frame {
    fin: bool,
    opcode: u8,
    mask: Option<[u8; 4]>,
    remaining: u64,
    mask_offset: u64,
}

struct Message {
    opcode: WebSocketOpcode,
    body: CapturedBody,
}

struct FrameDecoder {
    capture_limit: usize,
    header: [u8; 14],
    header_len: usize,
    frame: Option<Frame>,
    message: Option<Message>,
    disabled: bool,
    closed: bool,
}

impl FrameDecoder {
    fn at_message_boundary(&self) -> bool {
        !self.disabled
            && !self.closed
            && self.header_len == 0
            && self.frame.is_none()
            && self.message.is_none()
    }
    fn new(capture_limit: usize) -> Self {
        Self {
            capture_limit,
            header: [0; 14],
            header_len: 0,
            frame: None,
            message: None,
            disabled: false,
            closed: false,
        }
    }

    fn is_disabled(&self) -> bool {
        self.disabled
    }

    fn disable(&mut self) {
        self.disabled = true;
        self.frame = None;
        self.message = None;
        self.header_len = 0;
    }

    fn feed(
        &mut self,
        mut bytes: &[u8],
        emit: &mut dyn FnMut(WebSocketOpcode, CapturedBody),
    ) -> Result<(), String> {
        if self.disabled {
            return Ok(());
        }

        while !bytes.is_empty() {
            if self.frame.is_none() {
                // First gather the two fixed bytes, then use them to learn
                // whether extended lengths and a mask key follow.
                if self.header_len < 2 {
                    let take = (2 - self.header_len).min(bytes.len());
                    self.header[self.header_len..self.header_len + take]
                        .copy_from_slice(&bytes[..take]);
                    self.header_len += take;
                    bytes = &bytes[take..];
                    if self.header_len < 2 {
                        break;
                    }
                }
                let needed = self.header_needed();
                if self.header_len < needed {
                    let take = (needed - self.header_len).min(bytes.len());
                    self.header[self.header_len..self.header_len + take]
                        .copy_from_slice(&bytes[..take]);
                    self.header_len += take;
                    bytes = &bytes[take..];
                    if self.header_len < needed {
                        break;
                    }
                }
                self.begin_frame()?;
                if self
                    .frame
                    .as_ref()
                    .is_some_and(|frame| frame.remaining == 0)
                {
                    self.finish_frame(emit)?;
                }
                continue;
            }

            let frame = self.frame.as_mut().expect("frame checked above");
            let take = frame.remaining.min(bytes.len() as u64) as usize;
            if take > 0 {
                let chunk = &bytes[..take];
                if frame.opcode <= 2
                    && let Some(message) = self.message.as_mut()
                {
                    if let Some(mask) = frame.mask {
                        let mut unmasked = Vec::with_capacity(take);
                        for (index, byte) in chunk.iter().enumerate() {
                            let shift = (frame.mask_offset as usize + index) % 4;
                            unmasked.push(byte ^ mask[shift]);
                        }
                        message.body.append(&unmasked, self.capture_limit);
                    } else {
                        message.body.append(chunk, self.capture_limit);
                    }
                }
                frame.mask_offset += take as u64;
                frame.remaining -= take as u64;
                bytes = &bytes[take..];
            }
            if self
                .frame
                .as_ref()
                .is_some_and(|frame| frame.remaining == 0)
            {
                self.finish_frame(emit)?;
            }
        }
        Ok(())
    }

    /// Header length implied by the first two bytes (2, 4, 10, or 14).
    fn header_needed(&self) -> usize {
        if self.header_len < 2 {
            return 2;
        }
        let masked = self.header[1] & 0x80 != 0;
        let extended = match self.header[1] & 0x7f {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        2 + extended + if masked { 4 } else { 0 }
    }

    fn begin_frame(&mut self) -> Result<(), String> {
        let first = self.header[0];
        let second = self.header[1];
        let fin = first & 0x80 != 0;
        let opcode = first & 0x0f;
        let masked = second & 0x80 != 0;
        if first & 0x70 != 0 {
            return Err("reserved WebSocket bits are set".to_string());
        }

        let mut offset = 2;
        let mut length = u64::from(second & 0x7f);
        if length == 126 {
            length = u64::from(u16::from_be_bytes(
                self.header[offset..offset + 2]
                    .try_into()
                    .expect("two-byte extended length"),
            ));
            offset += 2;
        } else if length == 127 {
            let bytes: [u8; 8] = self.header[offset..offset + 8]
                .try_into()
                .expect("eight-byte extended length");
            length = u64::from_be_bytes(bytes);
            if length & (1 << 63) != 0 {
                return Err("WebSocket frame length has the reserved high bit set".to_string());
            }
            offset += 8;
        }

        let mask = if masked {
            let key: [u8; 4] = self.header[offset..offset + 4]
                .try_into()
                .expect("four-byte mask key");
            Some(key)
        } else {
            None
        };

        if opcode >= 0x8 && (!fin || length > 125) {
            return Err("invalid WebSocket control frame".to_string());
        }

        match opcode {
            0 => {
                if self.message.is_none() {
                    return Err("WebSocket continuation without an open message".to_string());
                }
            }
            1 | 2 => {
                if self.message.is_some() {
                    return Err("WebSocket data frame interrupted an open message".to_string());
                }
                self.message = Some(Message {
                    opcode: if opcode == 1 {
                        WebSocketOpcode::Text
                    } else {
                        WebSocketOpcode::Binary
                    },
                    body: CapturedBody::default(),
                });
            }
            0x8..=0xa => {}
            _ => return Err(format!("unknown WebSocket opcode {opcode:#x}")),
        }

        self.frame = Some(Frame {
            fin,
            opcode,
            mask,
            remaining: length,
            mask_offset: 0,
        });
        self.header_len = 0;
        Ok(())
    }

    fn finish_frame(
        &mut self,
        emit: &mut dyn FnMut(WebSocketOpcode, CapturedBody),
    ) -> Result<(), String> {
        let Some(frame) = self.frame.take() else {
            return Ok(());
        };
        if frame.opcode == 0x8 {
            self.closed = true;
        }
        if frame.fin && matches!(frame.opcode, 0..=2) {
            let Some(mut message) = self.message.take() else {
                return Err("WebSocket message disappeared before completion".to_string());
            };
            message.body.mark_complete();
            emit(message.opcode, message.body);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn held_messages_preserve_fragments_and_control_frames_and_enforce_limits() {
        let mut first = masked_frame(false, 1, b"he");
        let ping = masked_frame(true, 9, b"p");
        first.extend_from_slice(&ping);
        first.extend_from_slice(&masked_frame(true, 0, b"llo"));
        let (mut tx, mut rx) = tokio::io::duplex(256);
        tx.write_all(&first[1..]).await.unwrap();
        let (wire, controls, message, closed) =
            read_held_message(&mut rx, first[0], WebSocketDirection::ToServer, 5)
                .await
                .unwrap();
        assert_eq!(wire, first);
        assert_eq!(controls, ping);
        assert_eq!(message, Some((WebSocketOpcode::Text, b"hello".to_vec())));
        assert!(!closed);

        let oversized = masked_frame(true, 2, b"123456");
        tx.write_all(&oversized[1..]).await.unwrap();
        assert_eq!(
            read_held_message(&mut rx, oversized[0], WebSocketDirection::ToServer, 5)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let unmasked = unmasked_frame(true, 1, b"x");
        let (mut tx, mut rx) = tokio::io::duplex(32);
        tx.write_all(&unmasked[1..]).await.unwrap();
        assert_eq!(
            read_held_message(&mut rx, unmasked[0], WebSocketDirection::ToServer, 5)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    fn masked_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x11, 0x22, 0x33, 0x44];
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(if fin { 0x80 | opcode } else { opcode });
        if payload.len() < 126 {
            frame.push(0x80 | payload.len() as u8);
        } else {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        for (index, byte) in payload.iter().enumerate() {
            frame.push(byte ^ mask[index % 4]);
        }
        frame
    }

    fn unmasked_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(payload.len() + 10);
        frame.push(if fin { 0x80 | opcode } else { opcode });
        if payload.len() < 126 {
            frame.push(payload.len() as u8);
        } else {
            frame.push(126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        frame
    }

    fn collect(chunks: &[&[u8]], limit: usize) -> Vec<(WebSocketOpcode, Vec<u8>)> {
        let mut decoder = FrameDecoder::new(limit);
        let mut messages = Vec::new();
        for chunk in chunks {
            decoder
                .feed(chunk, &mut |opcode, body| {
                    messages.push((opcode, body.data().to_vec()));
                })
                .unwrap();
        }
        messages
    }

    #[test]
    fn captures_masked_text_across_header_and_payload_chunks() {
        let frame = masked_frame(true, 1, br#"{"hello":"world"}"#);
        let messages = collect(
            &[&frame[..1], &frame[1..3], &frame[3..8], &frame[8..]],
            1024,
        );
        assert_eq!(
            messages,
            vec![(WebSocketOpcode::Text, br#"{"hello":"world"}"#.to_vec())]
        );
    }

    #[test]
    fn reassembles_fragmented_messages_and_ignores_ping_pong() {
        let messages = collect(
            &[
                &masked_frame(false, 1, b"hel"),
                &masked_frame(true, 9, b"ping"),
                &masked_frame(false, 0, b"lo "),
                &masked_frame(true, 10, b"pong"),
                &masked_frame(true, 0, b"world"),
            ],
            1024,
        );
        assert_eq!(
            messages,
            vec![(WebSocketOpcode::Text, b"hello world".to_vec())]
        );
    }

    #[test]
    fn captures_unmasked_server_binary_messages() {
        let messages = collect(&[&unmasked_frame(true, 2, &[0, 1, 2, 255])], 1024);
        assert_eq!(
            messages,
            vec![(WebSocketOpcode::Binary, vec![0, 1, 2, 255])]
        );
    }

    #[test]
    fn stops_collecting_after_capture_limit_but_counts_whole_message() {
        let mut decoder = FrameDecoder::new(4);
        let mut messages = Vec::new();
        decoder
            .feed(&masked_frame(true, 1, b"abcdefgh"), &mut |opcode, body| {
                messages.push((opcode, body));
            })
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1.data(), b"abcd");
        assert_eq!(messages[0].1.total_len(), 8);
        assert!(messages[0].1.is_truncated());
        assert!(messages[0].1.is_complete());
    }

    #[test]
    fn rejects_continuation_without_open_message() {
        let mut decoder = FrameDecoder::new(1024);
        let error = decoder
            .feed(&masked_frame(true, 0, b"x"), &mut |_, _| {})
            .unwrap_err();
        assert!(error.contains("continuation"));
    }

    #[test]
    fn encoded_messages_round_trip_at_all_length_boundaries() {
        for size in [0, 125, 126, 65535, 65536] {
            let payload = vec![0xa5; size];
            for masked in [false, true] {
                let frame = encode_message(WebSocketOpcode::Binary, &payload, masked);
                assert_eq!(frame[1] & 0x80 != 0, masked);
                assert_eq!(
                    collect(&[&frame], size),
                    vec![(WebSocketOpcode::Binary, payload.clone())]
                );
            }
        }
    }

    #[test]
    fn send_queue_validates_limits_and_reports_closed_and_full() {
        let connections: Connections = Arc::new(RwLock::new(HashMap::new()));
        let (up, mut rx) = mpsc::channel(1);
        let (down, _) = mpsc::channel(1);
        connections.write().unwrap().insert(7, (up, down));
        assert!(matches!(
            send(
                &connections,
                2,
                7,
                WebSocketDirection::ToServer,
                WebSocketOpcode::Binary,
                vec![1; 3]
            ),
            Err(WebSocketSendError::TooLarge)
        ));
        assert!(matches!(
            send(
                &connections,
                2,
                7,
                WebSocketDirection::ToServer,
                WebSocketOpcode::Text,
                vec![255]
            ),
            Err(WebSocketSendError::InvalidText)
        ));
        send(
            &connections,
            2,
            7,
            WebSocketDirection::ToServer,
            WebSocketOpcode::Text,
            b"ok".to_vec(),
        )
        .unwrap();
        assert!(matches!(
            send(
                &connections,
                2,
                7,
                WebSocketDirection::ToServer,
                WebSocketOpcode::Text,
                vec![]
            ),
            Err(WebSocketSendError::Full)
        ));
        assert_eq!(rx.try_recv().unwrap().payload, b"ok");
        drop(rx);
        assert!(matches!(
            send(
                &connections,
                2,
                7,
                WebSocketDirection::ToServer,
                WebSocketOpcode::Text,
                vec![]
            ),
            Err(WebSocketSendError::Closed)
        ));
        assert!(matches!(
            send(
                &connections,
                2,
                8,
                WebSocketDirection::ToServer,
                WebSocketOpcode::Text,
                vec![]
            ),
            Err(WebSocketSendError::Closed)
        ));
    }

    #[test]
    fn injection_waits_for_complete_message_and_stops_after_close() {
        let mut decoder = FrameDecoder::new(10);
        assert!(decoder.at_message_boundary());
        let first = masked_frame(false, 1, b"a");
        decoder.feed(&first[..1], &mut |_, _| {}).unwrap();
        assert!(!decoder.at_message_boundary());
        decoder.feed(&first[1..], &mut |_, _| {}).unwrap();
        assert!(!decoder.at_message_boundary());
        decoder
            .feed(&masked_frame(true, 0, b"b"), &mut |_, _| {})
            .unwrap();
        assert!(decoder.at_message_boundary());
        decoder
            .feed(&masked_frame(true, 8, b""), &mut |_, _| {})
            .unwrap();
        assert!(!decoder.at_message_boundary());
    }
}
