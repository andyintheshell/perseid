use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// Loopback-only by default: captured traffic may contain credentials.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8080";
/// Bytes retained per body. Bodies larger than this are forwarded in full but
/// only the leading bytes are captured.
pub const DEFAULT_CAPTURE_LIMIT: usize = 1024 * 1024;
/// Bound on queued [`crate::ProxyEvent`]s; when full, events are dropped and
/// counted rather than growing memory without limit.
pub const DEFAULT_EVENT_QUEUE: usize = 256;
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Configuration for [`crate::Proxy::start`].
#[derive(Clone)]
pub struct ProxyConfig {
    /// Address to bind. Keep this on loopback unless you know what you're doing.
    pub listen: SocketAddr,
    /// Per-body capture limit in bytes.
    pub capture_limit: usize,
    /// Maximum number of events queued towards the consumer.
    pub event_queue: usize,
    /// Timeout for establishing upstream TCP connections.
    pub connect_timeout: Duration,
    /// Called whenever an event is enqueued, so a GUI can wake up its event
    /// loop instead of polling. Must not block.
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl ProxyConfig {
    pub fn new(listen: SocketAddr) -> Self {
        Self {
            listen,
            ..Self::default()
        }
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN.parse().expect("valid default listen addr"),
            capture_limit: DEFAULT_CAPTURE_LIMIT,
            event_queue: DEFAULT_EVENT_QUEUE,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            notify: None,
        }
    }
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("listen", &self.listen)
            .field("capture_limit", &self.capture_limit)
            .field("event_queue", &self.event_queue)
            .field("connect_timeout", &self.connect_timeout)
            .field("notify", &self.notify.is_some())
            .finish()
    }
}
