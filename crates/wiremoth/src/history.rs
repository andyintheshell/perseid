//! UI-side history: proxy events folded into inspectable exchange records.
//!
//! History is intentionally simple: one map for lookup, one deque for order,
//! and a hard cap. Memory stays bounded because bodies are already capped by
//! the proxy's capture limit and the oldest entries are evicted here.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use wiremoth_proxy::{CapturedBody, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead};

/// Where an exchange is in its lifecycle.
#[derive(Debug)]
pub enum Phase {
    InFlight,
    Finished {
        duration: Duration,
        result: ExchangeResult,
    },
}

/// One request/response exchange as shown in the history table.
#[derive(Debug)]
pub struct HistoryItem {
    pub id: u64,
    pub timestamp: SystemTime,
    pub request: RequestInfo,
    pub request_body: CapturedBody,
    pub response: Option<ResponseHead>,
    pub phase: Phase,
}

impl HistoryItem {
    pub fn is_in_flight(&self) -> bool {
        matches!(self.phase, Phase::InFlight)
    }

    pub fn is_tunnel(&self) -> bool {
        self.request.method.eq_ignore_ascii_case("CONNECT")
    }

    pub fn duration(&self) -> Option<Duration> {
        match &self.phase {
            Phase::InFlight => None,
            Phase::Finished { duration, .. } => Some(*duration),
        }
    }

    pub fn status(&self) -> Option<u16> {
        self.response.as_ref().map(|head| head.status)
    }

    pub fn response_body(&self) -> Option<&CapturedBody> {
        match &self.phase {
            Phase::Finished {
                result: ExchangeResult::Response { body },
                ..
            } => Some(body),
            _ => None,
        }
    }

    pub fn tunnel_stats(&self) -> Option<(u64, u64)> {
        match &self.phase {
            Phase::Finished {
                result:
                    ExchangeResult::Tunnel {
                        client_to_server,
                        server_to_client,
                    },
                ..
            } => Some((*client_to_server, *server_to_client)),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match &self.phase {
            Phase::Finished {
                result: ExchangeResult::Failed { error },
                ..
            } => Some(error),
            _ => None,
        }
    }

    /// Bytes shown in the "size" column: response body when known, else
    /// request body, else total tunneled bytes.
    pub fn transferred(&self) -> u64 {
        if let Some(body) = self.response_body() {
            return body.total_len();
        }
        if let Some((up, down)) = self.tunnel_stats() {
            return up + down;
        }
        self.request_body.total_len()
    }
}

/// Bounded, ordered history of exchanges.
#[derive(Debug)]
pub struct History {
    items: HashMap<u64, HistoryItem>,
    order: VecDeque<u64>,
    limit: usize,
    evicted: u64,
    in_flight: usize,
    revision: u64,
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            items: HashMap::new(),
            order: VecDeque::new(),
            limit: limit.max(1),
            evicted: 0,
            in_flight: 0,
            revision: 0,
        }
    }

    /// Bumped whenever anything changes; lets views cache formatted text.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Number of entries dropped because of the history limit.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.evicted = 0;
        self.in_flight = 0;
        self.revision += 1;
    }

    pub fn get(&self, id: u64) -> Option<&HistoryItem> {
        self.items.get(&id)
    }

    /// Oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &HistoryItem> {
        self.order.iter().filter_map(|id| self.items.get(id))
    }

    /// Fold one proxy event into the history. Returns the exchange id.
    pub fn apply(&mut self, event: ProxyEvent) -> u64 {
        let id = event.id();
        match event {
            ProxyEvent::Started {
                id,
                timestamp,
                request,
            } => {
                self.items.insert(
                    id,
                    HistoryItem {
                        id,
                        timestamp,
                        request,
                        request_body: CapturedBody::default(),
                        response: None,
                        phase: Phase::InFlight,
                    },
                );
                self.order.push_back(id);
                self.in_flight += 1;
                while self.order.len() > self.limit {
                    if let Some(oldest) = self.order.pop_front()
                        && let Some(item) = self.items.remove(&oldest)
                    {
                        if item.is_in_flight() {
                            self.in_flight = self.in_flight.saturating_sub(1);
                        }
                        self.evicted += 1;
                    }
                }
            }
            ProxyEvent::ResponseHead { id, head } => {
                if let Some(item) = self.items.get_mut(&id) {
                    item.response = Some(head);
                }
            }
            ProxyEvent::Finished { id, end } => {
                if let Some(item) = self.items.get_mut(&id) {
                    item.request_body = end.request_body;
                    item.phase = Phase::Finished {
                        duration: end.duration,
                        result: end.result,
                    };
                    self.in_flight = self.in_flight.saturating_sub(1);
                }
            }
        }
        self.revision += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use wiremoth_proxy::{ExchangeEnd, RequestInfo};

    fn started(id: u64, method: &str) -> ProxyEvent {
        ProxyEvent::Started {
            id,
            timestamp: SystemTime::now(),
            request: RequestInfo {
                method: method.to_string(),
                uri: format!("http://example.com/{id}"),
                host: "example.com".to_string(),
                path: format!("/{id}"),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
        }
    }

    fn finished(id: u64) -> ProxyEvent {
        ProxyEvent::Finished {
            id,
            end: ExchangeEnd {
                duration: Duration::from_millis(5),
                request_body: CapturedBody::default(),
                result: ExchangeResult::Response {
                    body: CapturedBody::default(),
                },
            },
        }
    }

    #[test]
    fn folds_events_into_items() {
        let mut history = History::new(10);
        history.apply(started(1, "GET"));
        assert_eq!(history.len(), 1);
        assert_eq!(history.in_flight(), 1);
        let item = history.get(1).unwrap();
        assert!(item.is_in_flight());
        assert_eq!(item.status(), None);

        history.apply(ProxyEvent::ResponseHead {
            id: 1,
            head: ResponseHead {
                version: "HTTP/1.1".to_string(),
                status: 200,
                reason: "OK".to_string(),
                headers: Vec::new(),
            },
        });
        history.apply(finished(1));
        assert_eq!(history.in_flight(), 0);
        let item = history.get(1).unwrap();
        assert!(!item.is_in_flight());
        assert_eq!(item.status(), Some(200));
        assert_eq!(item.duration(), Some(Duration::from_millis(5)));
    }

    #[test]
    fn evicts_oldest_beyond_limit() {
        let mut history = History::new(2);
        for id in 1..=5 {
            history.apply(started(id, "GET"));
        }
        assert_eq!(history.len(), 2);
        assert_eq!(history.evicted(), 3);
        assert!(history.get(1).is_none());
        assert!(history.get(5).is_some());
        let ids: Vec<u64> = history.iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![4, 5]);
    }

    #[test]
    fn finisher_after_eviction_is_ignored() {
        let mut history = History::new(1);
        history.apply(started(1, "GET"));
        history.apply(started(2, "GET"));
        history.apply(finished(1));
        assert_eq!(history.len(), 1);
        assert_eq!(history.in_flight(), 1);
        assert!(history.get(2).unwrap().is_in_flight());
    }
}
