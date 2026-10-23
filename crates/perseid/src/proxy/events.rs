//! Model types for captured traffic.
//!
//! The proxy never hands out live internals: it observes an exchange and turns
//! it into immutable [`ProxyEvent`]s that a consumer (the UI, a test, a future
//! disk-backed history) applies to its own state.

use std::time::{Duration, SystemTime};

/// Headers as `(name, value)` pairs, in the order they appeared.
///
/// Header names are normalized to lowercase by the underlying HTTP stack;
/// preserving the exact wire casing would require bypassing `hyper`'s parser
/// and is tracked as a future fidelity improvement.
pub type HeaderList = Vec<(String, String)>;

/// The request line and headers as received from the client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestInfo {
    /// Request method (`GET`, `POST`, `CONNECT`, ...).
    pub method: String,
    /// The request target exactly as the client sent it
    /// (absolute form for plain HTTP, `host:port` for CONNECT).
    pub uri: String,
    /// Scheme of the connection the request arrived on: `http`, or `https`
    /// for an intercepted CONNECT tunnel.
    pub scheme: String,
    /// Logical host, e.g. `example.com` or `example.com:8080`.
    pub host: String,
    /// Origin-form path (`/a?b=c`), empty for CONNECT.
    pub path: String,
    /// HTTP version of the client request, e.g. `HTTP/1.1` or `HTTP/2`.
    pub version: String,
    pub headers: HeaderList,
}

/// Status line and headers of an upstream response.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResponseHead {
    /// HTTP version of the upstream response, e.g. `HTTP/1.1` or `HTTP/2`.
    pub version: String,
    pub status: u16,
    /// Reason phrase; `hyper` only records non-canonical phrases, so this may
    /// be empty even though a canonical phrase was on the wire.
    pub reason: String,
    pub headers: HeaderList,
}

/// The direction a WebSocket message travelled through the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketDirection {
    /// Browser/client to origin server.
    ToServer,
    /// Origin server to browser/client.
    ToClient,
}

/// The WebSocket data opcode used to deliver a complete message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketOpcode {
    Text,
    Binary,
}

/// Handshake-derived metadata shared by every message on one connection.
///
/// Burp assigns a stable WebSocket ID to each connection and repeats it on
/// every message row. The fields here are deliberately small: a busy message
/// history should not retain a full copy of the handshake for every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketConnectionInfo {
    /// Stable connection ID, also used as the WebSocket ID in history.
    pub id: u64,
    /// `ws://` or `wss://` URL of the handshake target.
    pub url: String,
    /// Host used for scope filtering, e.g. `example.com:8080`.
    pub host: String,
    /// Whether the handshake travelled over TLS.
    pub tls: bool,
    /// Local proxy listener port that accepted the connection.
    pub listener_port: u16,
    /// Subprotocol selected by the origin, when one was negotiated.
    pub protocol: Option<String>,
}

/// A body as retained by the capture limits.
///
/// Only the first `capture_limit` bytes are stored; `total` always counts
/// every byte observed, so consumers can tell how much was dropped.
#[derive(Debug, Clone, Default)]
pub struct CapturedBody {
    data: Vec<u8>,
    total: u64,
    truncated: bool,
    complete: bool,
}

impl CapturedBody {
    /// The retained bytes (possibly fewer than the wire body).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Total bytes observed on the wire, including bytes not retained.
    pub fn total_len(&self) -> u64 {
        self.total
    }

    /// Whether bytes were dropped because of the capture limit.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Whether the end of the body was observed. `false` means the transfer
    /// was aborted or the body was never read.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Whether the body was observed to have zero bytes.
    ///
    /// An unread or incomplete body is not empty: it may still grow.
    pub fn is_empty(&self) -> bool {
        self.complete && self.total == 0
    }

    /// Build a complete, untruncated body from bytes that are already bounded.
    ///
    /// The streaming capture path builds bodies incrementally; this constructor
    /// is useful for tools and tests that already hold a bounded body.
    pub fn from_bytes(data: Vec<u8>) -> Self {
        let total = data.len() as u64;
        Self {
            data,
            total,
            truncated: false,
            complete: true,
        }
    }

    /// Restore a body from a stored snapshot. The metadata is authoritative:
    /// `data` may be empty when the caller loaded only a row summary.
    pub(crate) fn from_stored_parts(
        data: Vec<u8>,
        total: u64,
        truncated: bool,
        complete: bool,
    ) -> Self {
        let total = total.max(data.len() as u64);
        Self {
            data,
            total,
            truncated,
            complete,
        }
    }

    /// Build a complete body from bytes, retaining at most `limit` bytes.
    pub(crate) fn from_bytes_limited(data: &[u8], limit: usize) -> Self {
        let mut body = Self::default();
        body.append(data, limit);
        body.mark_complete();
        body
    }

    pub(crate) fn append(&mut self, chunk: &[u8], limit: usize) {
        self.total += chunk.len() as u64;
        if self.data.len() < limit {
            let take = (limit - self.data.len()).min(chunk.len());
            self.data.extend_from_slice(&chunk[..take]);
            if take < chunk.len() {
                self.truncated = true;
            }
        } else if !chunk.is_empty() {
            self.truncated = true;
        }
    }

    pub(crate) fn mark_complete(&mut self) {
        self.complete = true;
    }
}

/// How an exchange ended.
#[derive(Debug, Clone)]
pub enum ExchangeResult {
    /// A regular request/response exchange. The response head is delivered
    /// separately via [`ProxyEvent::ResponseHead`] as soon as it is available.
    Response { body: CapturedBody },
    /// A CONNECT tunnel that was closed normally; byte counts per direction.
    Tunnel {
        client_to_server: u64,
        server_to_client: u64,
    },
    /// A CONNECT tunnel whose TLS was intercepted; `requests` counts the
    /// decrypted HTTP exchanges carried over the tunnel. Those exchanges also
    /// appear individually in history.
    InterceptedTls { requests: u64 },
    /// The user dropped the message in the interceptor. Burp still records the
    /// exchange in HTTP history, but there is no response to show.
    Dropped,
    /// The exchange failed (upstream connect error, malformed request, ...).
    Failed { error: String },
}

/// The final event of an exchange.
#[derive(Debug, Clone)]
pub struct ExchangeEnd {
    /// Wall-clock time from accepting the request to finishing the transfer.
    pub duration: Duration,
    /// Body bytes sent by the client, up to the capture limit.
    pub request_body: CapturedBody,
    pub result: ExchangeResult,
}

/// An observable step in the lifecycle of one proxied exchange.
#[derive(Debug, Clone)]
pub enum ProxyEvent {
    /// The request head was parsed. Sent before the request body is read, so
    /// long-running uploads/streams become visible immediately.
    Started {
        id: u64,
        timestamp: SystemTime,
        request: RequestInfo,
    },
    /// The upstream response head arrived (regular HTTP only).
    ResponseHead { id: u64, head: ResponseHead },
    /// The exchange is over: body relayed, tunnel closed, or failure.
    Finished { id: u64, end: ExchangeEnd },
    /// A complete WebSocket message was observed and reassembled.
    ///
    /// Frames are still forwarded byte-for-byte; this event is a bounded
    /// copy of the unmasked payload for the WebSockets history. Control
    /// frames and incomplete fragments do not produce events.
    WebSocketMessage {
        id: u64,
        timestamp: SystemTime,
        connection: WebSocketConnectionInfo,
        direction: WebSocketDirection,
        opcode: WebSocketOpcode,
        payload: CapturedBody,
    },
}

impl ProxyEvent {
    /// The exchange or message this event belongs to.
    pub fn id(&self) -> u64 {
        match *self {
            ProxyEvent::Started { id, .. }
            | ProxyEvent::ResponseHead { id, .. }
            | ProxyEvent::Finished { id, .. }
            | ProxyEvent::WebSocketMessage { id, .. } => id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CapturedBody;

    #[test]
    fn capture_limit_keeps_total_and_marks_truncation() {
        let mut body = CapturedBody::default();
        body.append(b"hello ", 8);
        body.append(b"world", 8);
        assert_eq!(body.data(), b"hello wo");
        assert_eq!(body.total_len(), 11);
        assert!(body.is_truncated());
        assert!(!body.is_complete());
        body.mark_complete();
        assert!(body.is_complete());
    }

    #[test]
    fn append_after_limit_only_counts_bytes() {
        let mut body = CapturedBody::default();
        body.append(&[0u8; 16], 4);
        body.append(&[1u8; 100], 4);
        assert_eq!(body.data().len(), 4);
        assert_eq!(body.total_len(), 116);
        assert!(body.is_truncated());
    }

    #[test]
    fn empty_appends_do_not_truncate() {
        let mut body = CapturedBody::default();
        body.append(b"abcd", 4);
        body.append(b"", 4);
        assert!(!body.is_truncated());
        assert_eq!(body.data(), b"abcd");
    }

    #[test]
    fn unread_body_is_not_empty() {
        let mut body = CapturedBody::default();
        assert!(!body.is_empty());
        body.mark_complete();
        assert!(body.is_empty());

        let body = CapturedBody::from_bytes(Vec::new());
        assert!(body.is_empty());
        assert!(!CapturedBody::from_bytes(b"x".to_vec()).is_empty());
        assert!(!CapturedBody::default().is_empty());
    }
}
