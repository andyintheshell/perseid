//! Bounded message-level history for captured WebSocket traffic.
//!
//! Unlike HTTP history, a WebSocket row is already complete when its event
//! arrives: frame fragmentation and unmasking happen in the proxy. There is no
//! in-flight phase to reconcile, so this model is intentionally simpler than
//! [`crate::history::History`].

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

use crate::proxy::{
    CapturedBody, ProxyEvent, WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
};

/// One complete text or binary WebSocket message.
#[derive(Debug, Clone)]
pub struct WebSocketMessageItem {
    /// Unique event ID used by the UI selection.
    pub id: u64,
    /// One-based row index shown in the WebSockets history table.
    pub number: u64,
    pub timestamp: SystemTime,
    /// Handshake-derived connection metadata, repeated on each row.
    pub connection: WebSocketConnectionInfo,
    pub direction: WebSocketDirection,
    pub opcode: WebSocketOpcode,
    pub payload: CapturedBody,
}

/// Bounded, ordered history of WebSocket messages.
#[derive(Debug)]
pub struct WebSocketHistory {
    items: HashMap<u64, WebSocketMessageItem>,
    order: VecDeque<u64>,
    limit: usize,
    byte_limit: usize,
    retained_bytes: usize,
    evicted: u64,
    next_number: u64,
    revision: u64,
}

impl WebSocketHistory {
    pub fn new(limit: usize, byte_limit: usize) -> Self {
        Self {
            items: HashMap::new(),
            order: VecDeque::new(),
            limit: limit.max(1),
            byte_limit: byte_limit.max(1),
            retained_bytes: 0,
            evicted: 0,
            next_number: 1,
            revision: 0,
        }
    }

    /// Bumped whenever anything changes; lets views cache their row filters.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn byte_limit(&self) -> usize {
        self.byte_limit
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.retained_bytes = 0;
        self.evicted = 0;
        self.next_number = 1;
        self.revision += 1;
    }

    /// Replace the cache with messages loaded from a project.
    ///
    /// Counters refer to the loaded history only, so a previous session's
    /// eviction count is discarded.
    pub fn load(&mut self, items: Vec<WebSocketMessageItem>) {
        self.items.clear();
        self.order.clear();
        self.retained_bytes = 0;
        self.evicted = 0;
        self.next_number = 1;
        for item in items {
            self.next_number = self.next_number.max(item.number.saturating_add(1));
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(item.payload.data().len());
            self.order.push_back(item.id);
            self.items.insert(item.id, item);
        }
        self.revision += 1;
    }

    pub fn get(&self, id: u64) -> Option<&WebSocketMessageItem> {
        self.items.get(&id)
    }

    /// Oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &WebSocketMessageItem> {
        self.order.iter().filter_map(|id| self.items.get(id))
    }

    pub(crate) fn remove_stored(&mut self, ids: &[u64]) {
        for id in ids {
            if let Some(item) = self.items.remove(id) {
                self.retained_bytes = self
                    .retained_bytes
                    .saturating_sub(item.payload.data().len());
            }
        }
        if !ids.is_empty() {
            self.order.retain(|id| self.items.contains_key(id));
            self.revision += 1;
        }
    }

    /// Fold one proxy event into the history. Non-WebSocket events are ignored.
    pub fn apply(&mut self, event: ProxyEvent) -> Option<u64> {
        let ProxyEvent::WebSocketMessage {
            id,
            timestamp,
            connection,
            direction,
            opcode,
            payload,
        } = event
        else {
            return None;
        };
        if self.items.contains_key(&id) {
            return Some(id);
        }
        let item = WebSocketMessageItem {
            id,
            number: self.next_number,
            timestamp,
            connection,
            direction,
            opcode,
            payload,
        };
        self.next_number += 1;
        self.retained_bytes += item.payload.data().len();
        self.items.insert(id, item);
        self.order.push_back(id);
        self.evict_to_limits();
        self.revision += 1;
        Some(id)
    }

    fn evict_to_limits(&mut self) {
        while self.order.len() > self.limit
            || (self.retained_bytes > self.byte_limit && self.order.len() > 1)
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            let Some(item) = self.items.remove(&oldest) else {
                continue;
            };
            self.retained_bytes = self
                .retained_bytes
                .saturating_sub(item.payload.data().len());
            self.evicted += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: u64, connection: u64, payload: &[u8]) -> ProxyEvent {
        ProxyEvent::WebSocketMessage {
            id,
            timestamp: SystemTime::now(),
            connection: WebSocketConnectionInfo {
                id: connection,
                url: "wss://example.com/socket".to_string(),
                host: "example.com".to_string(),
                tls: true,
                listener_port: 8080,
                protocol: Some("chat".to_string()),
            },
            direction: if id.is_multiple_of(2) {
                WebSocketDirection::ToClient
            } else {
                WebSocketDirection::ToServer
            },
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(payload.to_vec()),
        }
    }

    #[test]
    fn folds_messages_and_ignores_http_events() {
        let mut history = WebSocketHistory::new(10, 1024);
        assert_eq!(history.apply(message(1, 7, b"hello")), Some(1));
        assert_eq!(history.len(), 1);
        let item = history.get(1).expect("message");
        assert_eq!(item.number, 1);
        assert_eq!(item.connection.id, 7);
        assert_eq!(item.payload.data(), b"hello");

        history.apply(ProxyEvent::Started {
            id: 2,
            timestamp: SystemTime::now(),
            request: crate::proxy::RequestInfo {
                method: "GET".to_string(),
                uri: "/".to_string(),
                scheme: "http".to_string(),
                host: "example.com".to_string(),
                path: "/".to_string(),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
        });
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn evicts_oldest_by_count_and_bytes_without_vanishing_single_message() {
        let mut history = WebSocketHistory::new(2, 6);
        history.apply(message(1, 1, b"aa"));
        history.apply(message(2, 1, b"bb"));
        history.apply(message(3, 1, b"cc"));
        assert_eq!(history.len(), 2);
        assert!(history.get(1).is_none());
        assert!(history.get(2).is_some());
        assert!(history.get(3).is_some());
        assert_eq!(history.evicted(), 1);
        assert_eq!(history.retained_bytes(), 4);

        let mut tiny = WebSocketHistory::new(10, 1);
        tiny.apply(message(4, 1, b"abc"));
        assert_eq!(tiny.len(), 1);
        assert_eq!(tiny.retained_bytes(), 3);
    }

    #[test]
    fn clear_restarts_row_numbers() {
        let mut history = WebSocketHistory::new(10, 1024);
        history.apply(message(1, 1, b"a"));
        history.clear();
        history.apply(message(2, 1, b"b"));
        assert_eq!(history.get(2).expect("message").number, 1);
        assert_eq!(history.evicted(), 0);
    }

    #[test]
    fn load_discards_previous_eviction_count() {
        let mut history = WebSocketHistory::new(1, 1024);
        history.apply(message(1, 1, b"a"));
        history.apply(message(2, 1, b"b"));
        assert_eq!(history.evicted(), 1);

        history.load(vec![WebSocketMessageItem {
            id: 9,
            number: 9,
            timestamp: SystemTime::now(),
            connection: WebSocketConnectionInfo {
                id: 1,
                url: "wss://example.com/socket".to_string(),
                host: "example.com".to_string(),
                tls: true,
                listener_port: 8080,
                protocol: None,
            },
            direction: WebSocketDirection::ToClient,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(b"hello".to_vec()),
        }]);
        assert_eq!(history.evicted(), 0);
        assert_eq!(history.len(), 1);
        assert_eq!(history.get(9).expect("message").number, 9);
    }
}
