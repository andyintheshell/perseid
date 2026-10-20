//! UI-side history: proxy events folded into inspectable exchange records.
//!
//! Transport-level `CONNECT` tunnels are ignored, matching Burp's HTTP history:
//! only the HTTP requests carried through the proxy, including decrypted HTTPS
//! requests, become rows.
//!
//! History is intentionally simple: one map for lookup, one deque for order,
//! and a hard cap. Memory stays bounded because bodies are already capped by
//! the proxy's capture limit and the oldest entries are evicted here.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use perseid_proxy::{CapturedBody, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead};

/// Default cap on captured request/response body bytes retained by history.
///
/// The exchange-count limit alone allowed a worst case of roughly 4 GiB with
/// 1 MiB per body and 2,000 exchanges. This budget bounds the bodies themselves;
/// exchange metadata and display caches add a small amount on top.
pub const DEFAULT_HISTORY_BYTES: usize = 256 * 1024 * 1024;

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
    /// One-based request index shown in the table. This is separate from the
    /// proxy event id because transport-level CONNECT tunnels do not get a row.
    pub number: u64,
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

    /// How many HTTP requests were decrypted inside a TLS-intercepted CONNECT
    /// tunnel. `None` for every other kind of exchange, including untouched
    /// passthrough tunnels.
    pub fn tls_intercepted_requests(&self) -> Option<u64> {
        match &self.phase {
            Phase::Finished {
                result: ExchangeResult::InterceptedTls { requests },
                ..
            } => Some(*requests),
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
    byte_limit: usize,
    retained_bytes: usize,
    evicted: u64,
    in_flight: usize,
    next_number: u64,
    revision: u64,
}

impl History {
    pub fn new(limit: usize, byte_limit: usize) -> Self {
        Self {
            items: HashMap::new(),
            order: VecDeque::new(),
            limit: limit.max(1),
            byte_limit: byte_limit.max(1),
            retained_bytes: 0,
            evicted: 0,
            in_flight: 0,
            next_number: 1,
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

    /// Number of entries dropped because of the history limits.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Captured request/response body bytes currently retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.retained_bytes = 0;
        self.evicted = 0;
        self.in_flight = 0;
        self.next_number = 1;
        self.revision += 1;
    }

    /// Resolve every in-flight exchange as failed.
    ///
    /// This is a recovery path for a lossy event channel or a stopped proxy:
    /// the terminal event may never arrive, but a row must not stay in flight
    /// forever. A later [`ProxyEvent::Finished`] for the same id still updates
    /// the row normally.
    pub fn interrupt_in_flight(&mut self, error: &str) -> usize {
        if self.in_flight == 0 {
            return 0;
        }
        let now = SystemTime::now();
        let mut interrupted = 0;
        for id in &self.order {
            let Some(item) = self.items.get_mut(id) else {
                continue;
            };
            if item.is_in_flight() {
                let duration = now.duration_since(item.timestamp).unwrap_or_default();
                item.phase = Phase::Finished {
                    duration,
                    result: ExchangeResult::Failed {
                        error: error.to_string(),
                    },
                };
                interrupted += 1;
            }
        }
        if interrupted > 0 {
            self.in_flight = 0;
            self.revision += 1;
        }
        interrupted
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
                // CONNECT is transport plumbing, not an HTTP request. Burp's
                // HTTP history lists the requests carried inside the tunnel,
                // not the tunnel itself, so it gets no row or request index.
                if request.method.eq_ignore_ascii_case("CONNECT") {
                    return id;
                }
                let number = self.next_number;
                self.next_number += 1;
                self.items.insert(
                    id,
                    HistoryItem {
                        id,
                        number,
                        timestamp,
                        request,
                        request_body: CapturedBody::default(),
                        response: None,
                        phase: Phase::InFlight,
                    },
                );
                self.order.push_back(id);
                self.in_flight += 1;
                self.evict_to_limits();
            }
            ProxyEvent::ResponseHead { id, head } => {
                if let Some(item) = self.items.get_mut(&id) {
                    item.response = Some(head);
                }
            }
            ProxyEvent::Finished { id, end } => {
                if let Some(item) = self.items.get_mut(&id) {
                    let previous_bytes = item_body_bytes(item);
                    item.request_body = end.request_body;
                    item.phase = Phase::Finished {
                        duration: end.duration,
                        result: end.result,
                    };
                    let current_bytes = item_body_bytes(item);
                    self.retained_bytes = self
                        .retained_bytes
                        .saturating_sub(previous_bytes)
                        .saturating_add(current_bytes);
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.evict_to_limits();
                }
            }
        }
        self.revision += 1;
        id
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
            self.retained_bytes = self.retained_bytes.saturating_sub(item_body_bytes(&item));
            if item.is_in_flight() {
                self.in_flight = self.in_flight.saturating_sub(1);
            }
            self.evicted += 1;
        }
    }
}

fn item_body_bytes(item: &HistoryItem) -> usize {
    item.request_body.data().len() + item.response_body().map_or(0, |body| body.data().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use perseid_proxy::{ExchangeEnd, ExchangeResult, RequestInfo, ResponseHead};
    use std::time::SystemTime;

    fn started(id: u64, method: &str) -> ProxyEvent {
        ProxyEvent::Started {
            id,
            timestamp: SystemTime::now(),
            request: RequestInfo {
                method: method.to_string(),
                uri: format!("http://example.com/{id}"),
                scheme: "http".to_string(),
                host: "example.com".to_string(),
                path: format!("/{id}"),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
        }
    }

    fn connect_started(id: u64) -> ProxyEvent {
        ProxyEvent::Started {
            id,
            timestamp: SystemTime::now(),
            request: RequestInfo {
                method: "CONNECT".to_string(),
                uri: "example.com:443".to_string(),
                scheme: "https".to_string(),
                host: "example.com:443".to_string(),
                path: String::new(),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
        }
    }

    fn finished(id: u64) -> ProxyEvent {
        finished_with_bodies(id, Vec::new(), Vec::new())
    }

    fn finished_with_bodies(id: u64, request: Vec<u8>, response: Vec<u8>) -> ProxyEvent {
        ProxyEvent::Finished {
            id,
            end: ExchangeEnd {
                duration: Duration::from_millis(5),
                request_body: CapturedBody::from_bytes(request),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(response),
                },
            },
        }
    }

    #[test]
    fn folds_events_into_items() {
        let mut history = History::new(10, DEFAULT_HISTORY_BYTES);
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
    fn connect_tunnels_do_not_create_rows_or_consume_request_numbers() {
        let mut history = History::new(10, DEFAULT_HISTORY_BYTES);
        history.apply(connect_started(1));
        history.apply(ProxyEvent::ResponseHead {
            id: 1,
            head: ResponseHead {
                version: "HTTP/1.1".to_string(),
                status: 200,
                reason: "Connection Established".to_string(),
                headers: Vec::new(),
            },
        });
        history.apply(ProxyEvent::Finished {
            id: 1,
            end: ExchangeEnd {
                duration: Duration::from_millis(1),
                request_body: CapturedBody::default(),
                result: ExchangeResult::InterceptedTls { requests: 1 },
            },
        });
        assert_eq!(history.len(), 0);
        assert_eq!(history.in_flight(), 0);

        history.apply(started(2, "GET"));
        history.apply(started(3, "GET"));
        assert_eq!(history.len(), 2);
        let numbers: Vec<u64> = history.iter().map(|item| item.number).collect();
        assert_eq!(numbers, vec![1, 2]);

        history.clear();
        history.apply(started(4, "GET"));
        assert_eq!(history.get(4).unwrap().number, 1);
    }

    #[test]
    fn evicts_oldest_beyond_limit() {
        let mut history = History::new(2, DEFAULT_HISTORY_BYTES);
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
        let mut history = History::new(1, DEFAULT_HISTORY_BYTES);
        history.apply(started(1, "GET"));
        history.apply(started(2, "GET"));
        history.apply(finished(1));
        assert_eq!(history.len(), 1);
        assert_eq!(history.in_flight(), 1);
        assert!(history.get(2).unwrap().is_in_flight());
    }

    #[test]
    fn byte_budget_evicts_oldest_bodies() {
        let mut history = History::new(10, 20);

        history.apply(started(1, "GET"));
        history.apply(finished_with_bodies(1, vec![0; 8], vec![0; 8]));
        assert_eq!(history.retained_bytes(), 16);
        assert_eq!(history.len(), 1);

        history.apply(started(2, "GET"));
        history.apply(finished_with_bodies(2, vec![0; 8], vec![0; 8]));
        assert_eq!(history.len(), 1);
        assert_eq!(history.retained_bytes(), 16);
        assert!(history.get(1).is_none());
        assert!(history.get(2).is_some());
        assert_eq!(history.evicted(), 1);

        history.apply(started(3, "GET"));
        history.apply(finished_with_bodies(3, vec![0; 4], vec![0; 4]));
        assert_eq!(history.retained_bytes(), 8);
        assert_eq!(history.len(), 1);
        assert!(history.get(3).is_some());
    }

    #[test]
    fn single_exchange_can_exceed_byte_budget_without_vanishing() {
        let mut history = History::new(10, 4);
        history.apply(started(1, "GET"));
        history.apply(finished_with_bodies(1, vec![0; 8], Vec::new()));
        assert_eq!(history.len(), 1);
        assert_eq!(history.retained_bytes(), 8);
    }

    #[test]
    fn interrupts_in_flight_rows_and_accepts_late_finish() {
        let mut history = History::new(10, DEFAULT_HISTORY_BYTES);
        history.apply(started(1, "GET"));
        history.apply(started(2, "GET"));

        assert_eq!(history.interrupt_in_flight("events dropped"), 2);
        assert_eq!(history.in_flight(), 0);
        assert_eq!(history.get(1).unwrap().error(), Some("events dropped"));
        assert_eq!(history.get(2).unwrap().error(), Some("events dropped"));

        history.apply(finished(1));
        assert_eq!(history.in_flight(), 0);
        assert!(history.get(1).unwrap().error().is_none());
        assert_eq!(history.interrupt_in_flight("again"), 0);
    }
}
