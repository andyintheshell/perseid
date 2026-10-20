//! Editable messages held between the proxy and the UI.
//!
//! Intercept events use a dedicated bounded channel with backpressure. A
//! request or response stays parked until the consumer forwards it, drops it,
//! or drops the event itself (which aborts the exchange).

use tokio::sync::oneshot;

use crate::events::RequestInfo;
use crate::message::{parse_request_head, parse_response_head};

#[derive(Debug)]
pub(crate) enum InterceptDecision {
    Forward { head: String, body: Option<Vec<u8>> },
    Drop,
    Abort,
}

/// Which direction an intercepted message is travelling in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterceptKind {
    Request,
    Response,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RequestInfo;

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
}
