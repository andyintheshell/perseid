//! WebSocket frame relay and message capture.
//!
//! The proxy forwards every byte unchanged: frames are not re-framed,
//! re-masked, or decompressed. A small incremental parser observes the same
//! byte stream, unmasks client payloads, reassembles fragmented messages, and
//! emits one bounded [`ProxyEvent::WebSocketMessage`] per complete text or
//! binary message. Control frames are relayed but do not appear in history.
//!
//! `Sec-WebSocket-Extensions` is removed from the handshake so origins do not
//! negotiate compression. Keeping forwarded frames readable is more useful for
//! inspection than preserving an extension the history cannot decode.

use std::io;
use std::sync::Arc;
use std::time::SystemTime;

use log::{debug, warn};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::proxy::events::{
    CapturedBody, ProxyEvent, WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
};
use crate::proxy::server::Ctx;

/// Maximum bytes read from either side before the parser sees them.
const READ_BUFFER: usize = 16 * 1024;

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

    let upstream = pump(
        &ctx,
        &connection,
        &mut client_read,
        &mut server_write,
        WebSocketDirection::ToServer,
    );
    let downstream = pump(
        &ctx,
        &connection,
        &mut server_read,
        &mut client_write,
        WebSocketDirection::ToClient,
    );

    let (client_to_server, server_to_client) = tokio::join!(upstream, downstream);
    Ok((client_to_server?, server_to_client?))
}

async fn pump<R, W>(
    ctx: &Ctx,
    connection: &WebSocketConnectionInfo,
    reader: &mut R,
    writer: &mut W,
    direction: WebSocketDirection,
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut decoder = FrameDecoder::new(ctx.capture_limit);
    let mut buffer = [0u8; READ_BUFFER];
    let mut total = 0u64;

    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
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
}

impl FrameDecoder {
    fn new(capture_limit: usize) -> Self {
        Self {
            capture_limit,
            header: [0; 14],
            header_len: 0,
            frame: None,
            message: None,
            disabled: false,
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
}
