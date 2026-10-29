//! Editable messages held between the proxy and the UI.
//!
//! Intercept events use a dedicated bounded channel with backpressure. A
//! request or response stays parked until the consumer forwards it, drops it,
//! or drops the event itself (which aborts the exchange).

use std::time::SystemTime;

use tokio::sync::oneshot;

use crate::proxy::events::{
    RequestInfo, WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
};
use crate::proxy::message::{parse_request_head, parse_response_head};

#[derive(Debug)]
pub(crate) enum InterceptDecision {
    Forward { head: String, body: Option<Vec<u8>> },
    Drop,
    Abort,
}

/// Which direction an intercepted message is travelling in.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum InterceptKind {
    Request,
    Response,
}

/// Stable identity for one held message.
///
/// Exchange ids are unique while an item is waiting. The direction is part of
/// the key because a request and its response can otherwise share an id.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct InterceptKey {
    id: u64,
    kind: InterceptKind,
}

impl InterceptKey {
    /// The exchange id, matching `ProxyEvent` ids.
    pub fn id(self) -> u64 {
        self.id
    }

    pub fn kind(self) -> InterceptKind {
        self.kind
    }
}

/// A message waiting for an interception decision.
///
/// Requests and responses share the same state and lifecycle; [`kind`] records
/// which direction the held message is travelling in. The constructors are
/// crate-private so consumers can only act on items received from the proxy.
///
/// [`kind`]: InterceptItem::kind
#[derive(Debug)]
pub struct InterceptItem {
    kind: InterceptKind,
    id: u64,
    timestamp: SystemTime,
    request: RequestInfo,
    head: String,
    body: Option<Vec<u8>>,
    body_is_text: bool,
    responder: Option<oneshot::Sender<InterceptDecision>>,
}

impl InterceptItem {
    pub(crate) fn new_request(
        id: u64,
        request: RequestInfo,
        head: String,
        body: Option<Vec<u8>>,
        responder: oneshot::Sender<InterceptDecision>,
    ) -> Self {
        Self::new(InterceptKind::Request, id, request, head, body, responder)
    }

    pub(crate) fn new_response(
        id: u64,
        request: RequestInfo,
        head: String,
        body: Option<Vec<u8>>,
        responder: oneshot::Sender<InterceptDecision>,
    ) -> Self {
        Self::new(InterceptKind::Response, id, request, head, body, responder)
    }

    fn new(
        kind: InterceptKind,
        id: u64,
        request: RequestInfo,
        head: String,
        body: Option<Vec<u8>>,
        responder: oneshot::Sender<InterceptDecision>,
    ) -> Self {
        let body_is_text = body
            .as_ref()
            .is_some_and(|bytes| std::str::from_utf8(bytes).is_ok());
        Self {
            kind,
            id,
            timestamp: SystemTime::now(),
            request,
            head,
            body,
            body_is_text,
            responder: Some(responder),
        }
    }

    /// The exchange id, matching `ProxyEvent` ids.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn kind(&self) -> InterceptKind {
        self.kind
    }

    /// Stable identity for selection state in the intercept list.
    pub fn key(&self) -> InterceptKey {
        InterceptKey {
            id: self.id,
            kind: self.kind,
        }
    }

    /// When the proxy held this message.
    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }

    /// Parsed status for a held response. Requests have no status.
    pub fn response_status(&self) -> Option<u16> {
        (self.kind == InterceptKind::Response)
            .then(|| parse_response_head(&self.head).ok())
            .flatten()
            .map(|head| head.status.as_u16())
    }

    /// The request that started this exchange. For a response item this is the
    /// request the response belongs to.
    pub fn request(&self) -> &RequestInfo {
        &self.request
    }

    /// The raw message head (request/status line plus headers), terminated by
    /// a blank line.
    pub fn head(&self) -> &str {
        &self.head
    }

    /// The buffered body, or `None` when the body was too large or streaming
    /// and therefore cannot be edited.
    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    /// Whether the buffered body can safely be edited as text.
    pub fn body_is_text(&self) -> bool {
        self.body_is_text
    }

    /// Forward the edited message. `body` must be `None` when the original
    /// body was not buffered; otherwise `None` keeps the original body.
    pub fn forward(&mut self, head: &str, body: Option<Vec<u8>>) -> Result<(), InterceptError> {
        if body.is_some() && self.body.is_none() {
            return Err(InterceptError::BodyUnavailable);
        }
        self.validate_head(head)?;
        let responder = self
            .responder
            .take()
            .ok_or(InterceptError::AlreadyResolved)?;
        responder
            .send(InterceptDecision::Forward {
                head: head.to_string(),
                body,
            })
            .map_err(|_| InterceptError::Closed)
    }

    /// Drop the message. A dropped request is answered with `403 Forbidden`;
    /// a dropped response is replaced with `502 Bad Gateway`.
    pub fn drop_message(&mut self) -> Result<(), InterceptError> {
        let responder = self
            .responder
            .take()
            .ok_or(InterceptError::AlreadyResolved)?;
        responder
            .send(InterceptDecision::Drop)
            .map_err(|_| InterceptError::Closed)
    }

    fn validate_head(&self, head: &str) -> Result<(), InterceptError> {
        match self.kind {
            InterceptKind::Request => parse_request_head(head).map(|_| ()),
            InterceptKind::Response => parse_response_head(head).map(|_| ()),
        }
        .map_err(InterceptError::InvalidHead)
    }
}

impl Drop for InterceptItem {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.send(InterceptDecision::Abort);
        }
    }
}

/// Why an intercept decision could not be delivered.
#[derive(Debug, thiserror::Error)]
pub enum InterceptError {
    #[error("this message was already forwarded or dropped")]
    AlreadyResolved,
    #[error("the proxy is no longer waiting for this message")]
    Closed,
    #[error("{0}")]
    InvalidHead(String),
    #[error("body edits are unavailable because the body was not buffered")]
    BodyUnavailable,
}

/// A complete WebSocket message waiting for a forwarding decision.
///
/// Dropping the item without deciding aborts the held message. Edits are
/// bounded independently of the capture limit and text must remain UTF-8.
#[derive(Debug)]
pub struct WebSocketInterceptItem {
    id: u64,
    timestamp: SystemTime,
    connection: WebSocketConnectionInfo,
    direction: WebSocketDirection,
    opcode: WebSocketOpcode,
    payload: Vec<u8>,
    limit: usize,
    responder: Option<oneshot::Sender<WebSocketInterceptDecision>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WebSocketInterceptDecision {
    Forward(Vec<u8>),
    Drop,
}

impl WebSocketInterceptItem {
    pub(crate) fn new(
        id: u64,
        connection: WebSocketConnectionInfo,
        direction: WebSocketDirection,
        opcode: WebSocketOpcode,
        payload: Vec<u8>,
        limit: usize,
        responder: oneshot::Sender<WebSocketInterceptDecision>,
    ) -> Self {
        Self {
            id,
            timestamp: SystemTime::now(),
            connection,
            direction,
            opcode,
            payload,
            limit,
            responder: Some(responder),
        }
    }
    /// Unique identifier for this held message.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Time at which the message was held.
    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }
    /// Connection on which the message was received.
    pub fn connection(&self) -> &WebSocketConnectionInfo {
        &self.connection
    }
    /// Direction in which the message was travelling.
    pub fn direction(&self) -> WebSocketDirection {
        self.direction
    }
    /// Text or binary message type.
    pub fn opcode(&self) -> WebSocketOpcode {
        self.opcode
    }
    /// Original, complete unmasked message payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Whether the relay is no longer waiting for a decision.
    pub fn is_closed(&self) -> bool {
        self.responder
            .as_ref()
            .is_none_or(oneshot::Sender::is_closed)
    }

    /// Forward the supplied payload, preserving the original message type.
    pub fn forward(&mut self, payload: Vec<u8>) -> Result<(), WebSocketInterceptError> {
        if payload.len() > self.limit {
            return Err(WebSocketInterceptError::TooLarge);
        }
        if self.opcode == WebSocketOpcode::Text && std::str::from_utf8(&payload).is_err() {
            return Err(WebSocketInterceptError::InvalidText);
        }
        self.resolve(WebSocketInterceptDecision::Forward(payload))
    }

    /// Drop this message without forwarding it.
    pub fn drop_message(&mut self) -> Result<(), WebSocketInterceptError> {
        self.resolve(WebSocketInterceptDecision::Drop)
    }

    fn resolve(
        &mut self,
        decision: WebSocketInterceptDecision,
    ) -> Result<(), WebSocketInterceptError> {
        self.responder
            .take()
            .ok_or(WebSocketInterceptError::AlreadyResolved)?
            .send(decision)
            .map_err(|_| WebSocketInterceptError::Closed)
    }
}

/// Why a WebSocket interception decision could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum WebSocketInterceptError {
    #[error("this message was already forwarded or dropped")]
    AlreadyResolved,
    #[error("the WebSocket connection is no longer waiting for this message")]
    Closed,
    #[error("WebSocket message exceeds the interception limit")]
    TooLarge,
    #[error("WebSocket text messages must be UTF-8")]
    InvalidText,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::events::RequestInfo;

    fn websocket_item(
        opcode: WebSocketOpcode,
        limit: usize,
    ) -> (
        WebSocketInterceptItem,
        oneshot::Receiver<WebSocketInterceptDecision>,
    ) {
        let (tx, rx) = oneshot::channel();
        (
            WebSocketInterceptItem::new(
                1,
                WebSocketConnectionInfo {
                    id: 7,
                    url: "ws://example.com/".into(),
                    host: "example.com".into(),
                    tls: false,
                    listener_port: 8080,
                    protocol: None,
                },
                WebSocketDirection::ToServer,
                opcode,
                b"original".to_vec(),
                limit,
                tx,
            ),
            rx,
        )
    }

    #[test]
    fn websocket_edits_are_bounded_and_invalid_edits_leave_item_pending() {
        let (mut item, mut rx) = websocket_item(WebSocketOpcode::Text, 4);
        assert!(matches!(
            item.forward(vec![0; 5]),
            Err(WebSocketInterceptError::TooLarge)
        ));
        assert!(matches!(
            item.forward(vec![255]),
            Err(WebSocketInterceptError::InvalidText)
        ));
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        item.forward(b"edit".to_vec()).unwrap();
        assert_eq!(
            rx.try_recv().unwrap(),
            WebSocketInterceptDecision::Forward(b"edit".to_vec())
        );
        assert!(matches!(
            item.drop_message(),
            Err(WebSocketInterceptError::AlreadyResolved)
        ));
    }

    #[test]
    fn websocket_binary_drop_and_closed_decisions() {
        let (mut item, mut rx) = websocket_item(WebSocketOpcode::Binary, 2);
        item.forward(vec![0, 255]).unwrap();
        assert_eq!(
            rx.try_recv().unwrap(),
            WebSocketInterceptDecision::Forward(vec![0, 255])
        );
        let (mut item, mut rx) = websocket_item(WebSocketOpcode::Binary, 2);
        item.drop_message().unwrap();
        assert_eq!(rx.try_recv().unwrap(), WebSocketInterceptDecision::Drop);
        let (mut item, rx) = websocket_item(WebSocketOpcode::Binary, 2);
        drop(rx);
        assert!(matches!(
            item.drop_message(),
            Err(WebSocketInterceptError::Closed)
        ));
        let (item, mut rx) = websocket_item(WebSocketOpcode::Binary, 2);
        drop(item);
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }

    fn request_info() -> RequestInfo {
        RequestInfo {
            method: "POST".to_string(),
            uri: "http://example.com/".to_string(),
            scheme: "http".to_string(),
            host: "example.com".to_string(),
            path: "/".to_string(),
            version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
        }
    }

    #[test]
    fn forwarding_validates_the_edited_head() {
        let (tx, mut rx) = oneshot::channel();
        let mut item = InterceptItem::new_request(
            1,
            request_info(),
            "POST / HTTP/1.1\r\n\r\n".to_string(),
            Some(Vec::new()),
            tx,
        );

        let error = item
            .forward("not a request line", Some(Vec::new()))
            .unwrap_err();
        assert!(matches!(error, InterceptError::InvalidHead(_)));

        item.forward(
            "POST /changed HTTP/1.1\r\nHost: example.com\r\n\r\n",
            Some(b"x".to_vec()),
        )
        .unwrap();
        match rx.try_recv().unwrap() {
            InterceptDecision::Forward { head, body } => {
                assert!(head.contains("/changed"));
                assert_eq!(body.unwrap(), b"x");
            }
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    #[test]
    fn forwarding_a_response_validates_the_status_line() {
        let (tx, mut rx) = oneshot::channel();
        let mut item = InterceptItem::new_response(
            2,
            request_info(),
            "HTTP/1.1 204 No Content\r\n\r\n".to_string(),
            None,
            tx,
        );

        let error = item.forward("not a response line", None).unwrap_err();
        assert!(matches!(error, InterceptError::InvalidHead(_)));

        item.forward("HTTP/1.1 204 No Content\r\n\r\n", None)
            .unwrap();
        assert!(matches!(
            rx.try_recv().unwrap(),
            InterceptDecision::Forward { .. }
        ));
    }

    #[test]
    fn body_edits_are_rejected_when_the_body_was_not_buffered() {
        let (tx, _rx) = oneshot::channel();
        let mut item = InterceptItem::new_request(
            3,
            request_info(),
            "GET / HTTP/1.1\r\n\r\n".to_string(),
            None,
            tx,
        );

        let error = item
            .forward("GET / HTTP/1.1\r\n\r\n", Some(Vec::new()))
            .unwrap_err();
        assert!(matches!(error, InterceptError::BodyUnavailable));
    }

    #[test]
    fn dropping_the_item_aborts_the_exchange() {
        let (tx, mut rx) = oneshot::channel();
        {
            let _item = InterceptItem::new_request(
                1,
                request_info(),
                "GET / HTTP/1.1\r\n\r\n".to_string(),
                None,
                tx,
            );
        }
        assert!(matches!(rx.try_recv(), Ok(InterceptDecision::Abort)));
    }

    #[test]
    fn keys_distinguish_the_two_directions() {
        let (tx, _rx) = oneshot::channel();
        let request = InterceptItem::new_request(
            7,
            request_info(),
            "GET / HTTP/1.1\r\n\r\n".to_string(),
            None,
            tx,
        );
        let (tx, _rx) = oneshot::channel();
        let response = InterceptItem::new_response(
            7,
            request_info(),
            "HTTP/1.1 200 OK\r\n\r\n".to_string(),
            Some(Vec::new()),
            tx,
        );

        assert_eq!(request.key().id(), 7);
        assert_eq!(request.key().kind(), InterceptKind::Request);
        assert_eq!(response.key().kind(), InterceptKind::Response);
        assert_ne!(request.key(), response.key());
    }

    #[test]
    fn response_status_is_read_from_the_held_status_line() {
        let (tx, _rx) = oneshot::channel();
        let response = InterceptItem::new_response(
            8,
            request_info(),
            "HTTP/1.1 418 I'm a teapot\r\n\r\n".to_string(),
            None,
            tx,
        );
        assert_eq!(response.response_status(), Some(418));

        let (tx, _rx) = oneshot::channel();
        let request = InterceptItem::new_request(
            9,
            request_info(),
            "GET / HTTP/1.1\r\n\r\n".to_string(),
            None,
            tx,
        );
        assert_eq!(request.response_status(), None);
    }

    #[test]
    fn timestamp_is_set_when_the_message_is_held() {
        let before = SystemTime::now();
        let (tx, _rx) = oneshot::channel();
        let item = InterceptItem::new_request(
            10,
            request_info(),
            "GET / HTTP/1.1\r\n\r\n".to_string(),
            None,
            tx,
        );
        let after = SystemTime::now();

        assert!(item.timestamp() >= before);
        assert!(item.timestamp() <= after);
    }
}
