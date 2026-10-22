//! Direct HTTP/HTTPS Intruder engine: marker parsing and payload attacks.
//!
//! Intruder uses the same direct client as Repeater, so attacks work with the
//! proxy stopped and never pass through interception or rewrite policy. The
//! engine applies the optional scope list in [`IntruderPlan::scope`] to every
//! generated request; the desktop workspace fills it when scope enforcement is
//! on and also preflights the first request. A request template uses `$$`
//! around each payload position. Marker contents
//! are the base values: Sniper leaves untargeted positions at their base value,
//! while the other attack types replace every marked position.
//!
//! Work is generated lazily. Only the configured number of requests is in
//! flight, and completed results wait in a bounded channel. The desktop UI
//! drains that channel every frame and keeps its own bounded result history.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use hyper_rustls::{DefaultServerNameResolver, HttpsConnector};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::proxy::policy::host_in_scope;
use crate::proxy::repeater::{
    DEFAULT_REPEATER_TIMEOUT, RepeaterRequest, RepeaterResult, execute as execute_repeater,
};
use crate::proxy::tls::{self, UpstreamTlsError};
use crate::proxy::util::ProxyBody;

/// The pair of dollar signs around a payload position.
pub const MARKER: &str = "$$";

/// Bound on simultaneous in-flight attack requests. The UI offers the same
/// range, and the engine clamps values from library callers.
pub const MAX_INTRUDER_CONCURRENCY: usize = 32;
/// Bound on completed results waiting for the UI to collect them.
const MAX_QUEUED_RESULTS: usize = 64;
/// Retained bytes allowed across queued results before the engine applies
/// backpressure. The desktop UI keeps a separate bounded result history.
const MAX_QUEUED_RESULT_BYTES: usize = 64 * 1024 * 1024;
/// Requests larger than this are not generated. Repeater uses the same request
/// size ceiling.
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

/// Payload assignment strategy, matching Burp Intruder's attack types.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttackType {
    /// Try each payload at each position in turn, leaving other positions at
    /// their base value.
    #[default]
    Sniper,
    /// Put the same payload into every position.
    BatteringRam,
    /// Advance one payload set per position in lockstep.
    Pitchfork,
    /// Test every combination of one payload set per position.
    ClusterBomb,
}

impl AttackType {
    pub const ALL: [Self; 4] = [
        Self::Sniper,
        Self::BatteringRam,
        Self::Pitchfork,
        Self::ClusterBomb,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sniper => "Sniper attack",
            Self::BatteringRam => "Battering ram attack",
            Self::Pitchfork => "Pitchfork attack",
            Self::ClusterBomb => "Cluster bomb attack",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Sniper => {
                "One payload set; each position is tested in turn with the other positions left unchanged."
            }
            Self::BatteringRam => {
                "One payload set; the same payload is inserted into every position at once."
            }
            Self::Pitchfork => {
                "One payload set per position; sets advance together until the smallest set ends."
            }
            Self::ClusterBomb => {
                "One payload set per position; every combination of payloads is tested."
            }
        }
    }

    /// Whether the attack uses one shared payload set or one per position.
    pub fn uses_one_set(self) -> bool {
        matches!(self, Self::Sniper | Self::BatteringRam)
    }
}

/// One configured attack. The template is frozen when the attack starts so
/// editing the request cannot change a running attack underneath the engine.
#[derive(Clone, Debug)]
pub struct IntruderPlan {
    pub attack: AttackType,
    pub scheme: String,
    pub host: String,
    /// Raw request with `$$` delimiters around each payload position.
    pub template: String,
    /// Payload values. Sniper and Battering ram use only the first set.
    pub payload_sets: Vec<Vec<String>>,
    /// Number of simultaneous requests, clamped by the engine.
    pub concurrency: usize,
    /// Scope rules applied to every generated request. An empty list disables
    /// the engine-side check; the desktop workspace fills this only when scope
    /// enforcement is on.
    pub scope: Vec<String>,
}

/// A template with markers parsed once, ready to render sequences.
#[derive(Debug)]
pub struct PreparedIntruderPlan<'a> {
    plan: &'a IntruderPlan,
    positions: Vec<Range<usize>>,
    total: u64,
}

/// One generated request and the payload(s) inserted into it.
#[derive(Clone, Debug)]
pub struct RenderedRequest {
    pub raw: String,
    pub payloads: Vec<String>,
}

impl IntruderPlan {
    /// Parse the template and validate payload sets and request count.
    pub fn prepare(&self) -> Result<PreparedIntruderPlan<'_>, IntruderError> {
        let positions = marker_ranges(&self.template)?;
        if positions.is_empty() {
            return Err(IntruderError::NoPositions);
        }

        let set_count = if self.attack.uses_one_set() {
            1
        } else {
            positions.len()
        };
        if self.payload_sets.first().is_none_or(Vec::is_empty) {
            return Err(IntruderError::EmptyPayloadSet { position: 1 });
        }
        for index in 1..set_count {
            match self.payload_sets.get(index) {
                Some(set) if !set.is_empty() => {}
                Some(_) => {
                    return Err(IntruderError::EmptyPayloadSet {
                        position: index + 1,
                    });
                }
                None => {
                    return Err(IntruderError::MissingPayloadSet {
                        position: index + 1,
                    });
                }
            }
        }

        let total = match self.attack {
            AttackType::Sniper => {
                let sets = self.payload_sets[0].len() as u64;
                (positions.len() as u64)
                    .checked_mul(sets)
                    .ok_or(IntruderError::RequestCountOverflow)?
            }
            AttackType::BatteringRam => self.payload_sets[0].len() as u64,
            AttackType::Pitchfork => (0..set_count)
                .map(|index| self.payload_sets[index].len() as u64)
                .min()
                .unwrap_or(0),
            AttackType::ClusterBomb => {
                let mut total = 1u64;
                for set in self.payload_sets.iter().take(set_count) {
                    total = total
                        .checked_mul(set.len() as u64)
                        .ok_or(IntruderError::RequestCountOverflow)?;
                }
                total
            }
        };
        if total == 0 {
            return Err(IntruderError::EmptyPayloadSet { position: 1 });
        }

        Ok(PreparedIntruderPlan {
            plan: self,
            positions,
            total,
        })
    }

    /// Total number of requests this attack will generate.
    pub fn request_count(&self) -> Result<u64, IntruderError> {
        Ok(self.prepare()?.total())
    }

    /// Render one one-based request sequence without preparing the plan again.
    pub fn render_sequence(&self, sequence: u64) -> Result<RenderedRequest, IntruderError> {
        self.prepare()?.render(sequence)
    }
}

impl PreparedIntruderPlan<'_> {
    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn positions(&self) -> &[Range<usize>] {
        &self.positions
    }

    pub fn render(&self, sequence: u64) -> Result<RenderedRequest, IntruderError> {
        if sequence == 0 || sequence > self.total {
            return Err(IntruderError::SequenceOutOfRange {
                sequence,
                total: self.total,
            });
        }

        let plan = self.plan;
        let position_count = self.positions.len();
        let mut values: Vec<Option<&str>> = vec![None; position_count];
        let mut payloads = Vec::new();

        match plan.attack {
            AttackType::Sniper => {
                let set = &plan.payload_sets[0];
                let payloads_per_position = set.len() as u64;
                let position = ((sequence - 1) / payloads_per_position) as usize;
                let payload = &set[((sequence - 1) % payloads_per_position) as usize];
                values[position] = Some(payload);
                payloads.push(payload.clone());
            }
            AttackType::BatteringRam => {
                let payload = &plan.payload_sets[0][(sequence - 1) as usize];
                values.fill(Some(payload));
                payloads.push(payload.clone());
            }
            AttackType::Pitchfork => {
                let index = (sequence - 1) as usize;
                for (position, value) in values.iter_mut().enumerate() {
                    let payload = &plan.payload_sets[position][index];
                    *value = Some(payload);
                    payloads.push(payload.clone());
                }
            }
            AttackType::ClusterBomb => {
                let mut remaining = sequence - 1;
                for position in (0..position_count).rev() {
                    let set = &plan.payload_sets[position];
                    let index = (remaining % set.len() as u64) as usize;
                    remaining /= set.len() as u64;
                    values[position] = Some(&set[index]);
                }
                for payload in values.iter().flatten() {
                    payloads.push((*payload).to_string());
                }
            }
        }

        let mut raw = String::with_capacity(plan.template.len());
        let mut cursor = 0;
        for (index, range) in self.positions.iter().enumerate() {
            // `range` covers the content between the delimiters. Keep the text
            // before the opening `$$`, then emit the payload (or the original
            // base value) and skip both delimiters.
            let open = range.start - MARKER.len();
            raw.push_str(&plan.template[cursor..open]);
            match values[index] {
                Some(payload) => raw.push_str(payload),
                None => raw.push_str(&plan.template[range.clone()]),
            }
            cursor = range.end + MARKER.len();
        }
        raw.push_str(&plan.template[cursor..]);
        if raw.len() > MAX_REQUEST_BYTES {
            return Err(IntruderError::RequestTooLarge { sequence });
        }

        Ok(RenderedRequest { raw, payloads })
    }
}

/// Return the byte ranges between each pair of `$$` markers.
///
/// An odd number of markers is rejected rather than guessing where a position
/// should end. Ranges are UTF-8 boundaries because the delimiters are ASCII.
pub fn marker_ranges(template: &str) -> Result<Vec<Range<usize>>, IntruderError> {
    let mut ranges = Vec::new();
    let mut search = 0;
    while let Some(relative_open) = template[search..].find(MARKER) {
        let open = search + relative_open;
        let content_start = open + MARKER.len();
        let Some(relative_close) = template[content_start..].find(MARKER) else {
            return Err(IntruderError::UnbalancedMarker { offset: open });
        };
        let close = content_start + relative_close;
        ranges.push(content_start..close);
        search = close + MARKER.len();
    }
    Ok(ranges)
}

/// Remove every marker delimiter, leaving the marked base values in place.
pub fn clear_markers(template: &str) -> String {
    template.replace(MARKER, "")
}

/// Wrap the selection in `$$`, or remove the enclosing markers.
///
/// An empty selection inserts `$$$$` and returns the empty range between the
/// two delimiters. The returned range is always the new payload-position
/// content, so the caller can keep the editor selection on it.
pub fn toggle_marker(template: &str, selection: Range<usize>) -> (String, Range<usize>) {
    let len = template.len();
    let start = selection.start.min(len);
    let end = selection.end.max(start).min(len);
    if !template.is_char_boundary(start) || !template.is_char_boundary(end) {
        return (template.to_string(), start..start);
    }

    if start == end {
        // Toggle an empty position when the caret sits between two delimiters.
        if start >= MARKER.len()
            && template[start - MARKER.len()..start].eq(MARKER)
            && template[start..].starts_with(MARKER)
        {
            let mut out = template.to_string();
            out.replace_range(start - MARKER.len()..start + MARKER.len(), "");
            return (out, start - MARKER.len()..start - MARKER.len());
        }
        let mut out = String::with_capacity(len + MARKER.len() * 2);
        out.push_str(&template[..start]);
        out.push_str(MARKER);
        out.push_str(MARKER);
        out.push_str(&template[start..]);
        return (out, start + MARKER.len()..start + MARKER.len());
    }

    // The selection itself may include the delimiters, as when the user
    // selects `$$value$$` and clicks Add $$ to remove the position.
    if template[start..end].starts_with(MARKER)
        && template[start..end].ends_with(MARKER)
        && end - start >= MARKER.len() * 2
    {
        let mut out = template.to_string();
        out.replace_range(end - MARKER.len()..end, "");
        out.replace_range(start..start + MARKER.len(), "");
        return (out, start..end - MARKER.len() * 2);
    }

    // The selection may sit inside an existing position; remove the outside
    // delimiters.
    if start >= MARKER.len()
        && template[start - MARKER.len()..start].eq(MARKER)
        && template[end..].starts_with(MARKER)
    {
        let mut out = template.to_string();
        out.replace_range(end..end + MARKER.len(), "");
        out.replace_range(start - MARKER.len()..start, "");
        return (out, start - MARKER.len()..end - MARKER.len());
    }

    let mut out = String::with_capacity(len + MARKER.len() * 2);
    out.push_str(&template[..start]);
    out.push_str(MARKER);
    out.push_str(&template[start..end]);
    out.push_str(MARKER);
    out.push_str(&template[end..]);
    (out, start + MARKER.len()..end + MARKER.len())
}

/// Add markers around likely value positions in the request.
///
/// This covers URL query values, `application/x-www-form-urlencoded` body
/// values, and cookie values. Values already inside a marker are left alone.
/// The returned count is the number of new positions.
pub fn auto_mark(template: &str) -> (String, usize) {
    let existing = marker_ranges(template).unwrap_or_default();
    let mut candidates = Vec::new();

    let (head, body) = split_raw_request(template);
    if let Some(request_line) = head.lines().next()
        && let Some(target) = request_line.split_whitespace().nth(1)
    {
        let target_offset = head.find(target).unwrap_or(0);
        if let Some(query_start) = target.find('?') {
            let query = &target[query_start + 1..];
            value_ranges(query, 0, target_offset + query_start + 1, &mut candidates);
        }
    }
    for line in head.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("cookie") {
            let line_offset = line.as_ptr() as usize - head.as_ptr() as usize;
            let value_offset = line_offset + name.len() + 1;
            cookie_value_ranges(value, value_offset, &mut candidates);
        }
    }
    if !body.is_empty() && (looks_like_form(head) || (body.contains('=') && !body.contains('\n'))) {
        let body_offset = template.len() - body.len();
        value_ranges(body, 0, body_offset, &mut candidates);
    }

    candidates.retain(|range| {
        range.start < range.end
            && !existing
                .iter()
                .any(|marker| range.start < marker.end && marker.start < range.end)
    });
    candidates.sort_by_key(|range| range.start);
    candidates.dedup();

    let mut out = template.to_string();
    for range in candidates.iter().rev() {
        out.insert_str(range.end, MARKER);
        out.insert_str(range.start, MARKER);
    }
    (out, candidates.len())
}

/// Split an edited raw request into its head (including the blank line) and
/// body. Both CRLF and LF messages are accepted.
pub fn split_raw_request(raw: &str) -> (&str, &str) {
    if let Some(position) = raw.find("\r\n\r\n") {
        (&raw[..position + 4], &raw[position + 4..])
    } else if let Some(position) = raw.find("\n\n") {
        (&raw[..position + 2], &raw[position + 2..])
    } else {
        (raw, "")
    }
}

fn looks_like_form(head: &str) -> bool {
    head.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("content-type")
                && value
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("application/x-www-form-urlencoded")
        })
    })
}

/// Find `name=value` values in a query or form string, relative to `base`.
fn value_ranges(input: &str, start: usize, base: usize, out: &mut Vec<Range<usize>>) {
    let bytes = input.as_bytes();
    let mut segment_start = start;
    while segment_start <= input.len() {
        let segment_end = input[segment_start..]
            .find('&')
            .map_or(input.len(), |relative| segment_start + relative);
        let segment = &input[segment_start..segment_end];
        if let Some(equals) = segment.find('=') {
            let value_start = segment_start + equals + 1;
            let value_end = segment_end;
            if !segment[equals + 1..].trim().is_empty() {
                out.push(base + value_start..base + value_end);
            }
        }
        let _ = bytes;
        if segment_end == input.len() {
            break;
        }
        segment_start = segment_end + 1;
    }
}

/// Find values in a Cookie header. Leading spaces after `=` and around `;`
/// separators stay outside the position.
fn cookie_value_ranges(input: &str, base: usize, out: &mut Vec<Range<usize>>) {
    let mut segment_start = 0;
    while segment_start <= input.len() {
        let segment_end = input[segment_start..]
            .find(';')
            .map_or(input.len(), |relative| segment_start + relative);
        let segment = &input[segment_start..segment_end];
        if let Some(equals) = segment.find('=') {
            let value = &segment[equals + 1..];
            let leading = value.len() - value.trim_start().len();
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                let start = base + segment_start + equals + 1 + leading;
                out.push(start..start + trimmed.len());
            }
        }
        if segment_end == input.len() {
            break;
        }
        segment_start = segment_end + 1;
    }
}

/// Replace the Host header in an edited request, or add one after the request
/// line. The rest of the request, including marker delimiters, is preserved.
pub fn update_host_header(raw: &str, host: &str) -> String {
    let host = host.trim();
    if host.is_empty() {
        return raw.to_string();
    }
    let newline = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = raw.split_inclusive('\n').map(str::to_string).collect();
    let mut replaced = false;
    let mut insert_at = None;
    for (index, line) in lines.iter_mut().enumerate() {
        if index == 0 {
            continue;
        }
        let content = line.trim_end_matches(['\r', '\n']);
        if content.is_empty() {
            break;
        }
        if content
            .split_once(':')
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("host"))
        {
            let ending = &line[content.len()..];
            *line = format!("Host: {host}{ending}");
            replaced = true;
            break;
        }
        if index == 1 {
            insert_at = Some(1);
        }
    }
    if !replaced {
        if !raw.is_empty() && !raw.ends_with('\n') {
            // A one-line or unterminated request needs a line ending before
            // the inserted header.
            if let Some(first) = lines.first_mut() {
                first.push_str(newline);
            }
        }
        let insert_at = insert_at.filter(|index| *index <= lines.len()).unwrap_or(1);
        lines.insert(insert_at.min(lines.len()), format!("Host: {host}{newline}"));
    }
    lines.concat()
}

/// Why a plan or generated request is invalid.
#[derive(Debug, thiserror::Error)]
pub enum IntruderError {
    #[error("the request has an unclosed $$ position marker at byte {offset}")]
    UnbalancedMarker { offset: usize },
    #[error("add at least one payload position by wrapping a value in $$")]
    NoPositions,
    #[error("payload set {position} is empty")]
    EmptyPayloadSet { position: usize },
    #[error("payload set {position} is missing")]
    MissingPayloadSet { position: usize },
    #[error("the attack request count is too large")]
    RequestCountOverflow,
    #[error("request {sequence} is outside the attack (1..={total})")]
    SequenceOutOfRange { sequence: u64, total: u64 },
    #[error("generated request {sequence} exceeds the 8 MiB request limit")]
    RequestTooLarge { sequence: u64 },
    #[error("an Intruder attack is already running")]
    Busy,
    #[error("the Intruder engine is not running")]
    Closed,
}

/// Runtime settings shared by every attack in one engine.
#[derive(Clone)]
pub struct IntruderConfig {
    /// Maximum response body bytes retained for each result.
    pub capture_limit: usize,
    /// End-to-end timeout for one request.
    pub timeout: Duration,
    /// Extra roots trusted for upstream TLS, mainly for tests and private CAs.
    pub extra_roots_pem: Vec<String>,
    /// Called when an event is ready. The desktop UI uses this to repaint.
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for IntruderConfig {
    fn default() -> Self {
        Self {
            capture_limit: crate::proxy::DEFAULT_CAPTURE_LIMIT,
            timeout: DEFAULT_REPEATER_TIMEOUT,
            extra_roots_pem: Vec::new(),
            notify: None,
        }
    }
}

/// Failure to start the Intruder engine.
#[derive(Debug, thiserror::Error)]
pub enum IntruderStartError {
    #[error("the Intruder capture limit is too large")]
    CaptureLimit,
    #[error("could not start the Intruder runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error("could not spawn the Intruder thread: {0}")]
    Spawn(#[source] std::io::Error),
    #[error(transparent)]
    Tls(#[from] UpstreamTlsError),
    #[error("the Intruder thread stopped before it was ready")]
    Startup,
}

/// One completed request in an attack.
#[derive(Debug, Clone)]
pub struct IntruderResult {
    pub attack_id: u64,
    pub sequence: u64,
    pub payloads: Vec<String>,
    pub duration: Duration,
    pub result: RepeaterResult,
}

/// Events produced by the engine.
#[derive(Debug, Clone)]
pub enum IntruderEvent {
    Result(IntruderResult),
    Finished {
        attack_id: u64,
        completed: u64,
        total: u64,
        cancelled: bool,
    },
}

enum Command {
    Start {
        id: u64,
        plan: IntruderPlan,
        cancel: Arc<AtomicBool>,
    },
    Stop,
    Shutdown,
}

/// A direct HTTP/HTTPS attack engine with a small background runtime.
pub struct Intruder {
    commands: mpsc::UnboundedSender<Command>,
    results: mpsc::Receiver<IntruderEvent>,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Intruder {
    /// Start the attack runtime.
    pub fn start(config: IntruderConfig) -> Result<Self, IntruderStartError> {
        if config.capture_limit > MAX_QUEUED_RESULT_BYTES {
            return Err(IntruderStartError::CaptureLimit);
        }
        let capture_limit = config.capture_limit.max(1);
        let timeout = config.timeout.max(Duration::from_millis(1));
        let notify = config.notify.clone();
        let upstream_tls = tls::upstream_client_config(&config.extra_roots_pem)?;

        // Reserve room for the response body plus a little header/payload
        // overhead. At least one queued result must always fit.
        let result_bytes = capture_limit.saturating_add(64 * 1024);
        let result_capacity = (MAX_QUEUED_RESULT_BYTES / result_bytes).clamp(1, MAX_QUEUED_RESULTS);

        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (result_tx, results) = mpsc::channel(result_capacity);
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let running = Arc::new(AtomicBool::new(false));
        let running_thread = running.clone();

        let thread = thread::Builder::new()
            .name("perseid-intruder".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = startup_tx.send(Err(IntruderStartError::Runtime(err)));
                        return;
                    }
                };

                let mut connector = HttpConnector::new();
                connector.set_connect_timeout(Some(timeout));
                connector.enforce_http(false);
                let connector = HttpsConnector::new(
                    connector,
                    upstream_tls,
                    false,
                    Arc::new(DefaultServerNameResolver::default()),
                );
                let client: Client<HttpsConnector<HttpConnector>, ProxyBody> =
                    Client::builder(TokioExecutor::new())
                        .http1_preserve_header_case(true)
                        .http1_max_buf_size(64 * 1024)
                        .http2_max_header_list_size((64 * 1024) as u32)
                        .build(connector);

                runtime.block_on(async move {
                    let _ = startup_tx.send(Ok(()));
                    let mut current_cancel: Option<Arc<AtomicBool>> = None;
                    while let Some(command) = command_rx.recv().await {
                        match command {
                            Command::Start { id, plan, cancel } => {
                                if let Some(old) = current_cancel.replace(cancel.clone()) {
                                    old.store(true, Ordering::Relaxed);
                                }
                                running_thread.store(true, Ordering::Relaxed);
                                tokio::spawn(run_attack(
                                    client.clone(),
                                    id,
                                    plan,
                                    capture_limit,
                                    timeout,
                                    result_tx.clone(),
                                    cancel,
                                    notify.clone(),
                                    running_thread.clone(),
                                ));
                            }
                            Command::Stop => {
                                if let Some(cancel) = &current_cancel {
                                    cancel.store(true, Ordering::Relaxed);
                                }
                            }
                            Command::Shutdown => {
                                if let Some(cancel) = &current_cancel {
                                    cancel.store(true, Ordering::Relaxed);
                                }
                                break;
                            }
                        }
                    }
                });
            })
            .map_err(IntruderStartError::Spawn)?;

        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(IntruderStartError::Startup);
            }
        }

        Ok(Self {
            commands,
            results,
            running,
            thread: Some(thread),
        })
    }

    /// Queue one attack. Only one attack may run at a time; collect
    /// [`IntruderEvent::Finished`] before starting the next.
    pub fn start_attack(&self, plan: IntruderPlan) -> Result<u64, IntruderError> {
        plan.prepare()?;
        if self
            .running
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Err(IntruderError::Busy);
        }
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        if self
            .commands
            .send(Command::Start { id, plan, cancel })
            .is_err()
        {
            self.running.store(false, Ordering::Relaxed);
            return Err(IntruderError::Closed);
        }
        Ok(id)
    }

    /// Ask the running attack to stop scheduling new requests. Requests that
    /// are already in flight finish and still produce results.
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    /// Take the next result or completion event, if any.
    pub fn try_recv(&mut self) -> Option<IntruderEvent> {
        let event = self.results.try_recv().ok()?;
        if matches!(event, IntruderEvent::Finished { .. }) {
            self.running.store(false, Ordering::Relaxed);
        }
        Some(event)
    }

    /// Whether the engine thread is still alive.
    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for Intruder {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("Intruder thread panicked while shutting down");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_attack(
    client: Client<HttpsConnector<HttpConnector>, ProxyBody>,
    attack_id: u64,
    plan: IntruderPlan,
    capture_limit: usize,
    timeout: Duration,
    results: mpsc::Sender<IntruderEvent>,
    cancel: Arc<AtomicBool>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    running: Arc<AtomicBool>,
) {
    let prepared = match plan.prepare() {
        Ok(prepared) => prepared,
        Err(_) => {
            running.store(false, Ordering::Relaxed);
            let _ = results
                .send(IntruderEvent::Finished {
                    attack_id,
                    completed: 0,
                    total: 0,
                    cancelled: false,
                })
                .await;
            return;
        }
    };
    let total = prepared.total();
    let concurrency = plan.concurrency.clamp(1, MAX_INTRUDER_CONCURRENCY);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut tasks = JoinSet::new();
    let mut sequence = 1u64;
    let mut cancelled = false;

    while sequence <= total {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let Ok(permit) = semaphore.clone().acquire_owned().await else {
            break;
        };
        // Stop may arrive while this task waits for an in-flight request to
        // release a permit. Recheck after admission so stopping cannot schedule
        // one more request.
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            drop(permit);
            break;
        }
        let rendered = match prepared.render(sequence) {
            Ok(rendered) => rendered,
            Err(err) => {
                let event = result_event(
                    attack_id,
                    sequence,
                    Vec::new(),
                    Duration::ZERO,
                    RepeaterResult::Failed {
                        error: err.to_string(),
                    },
                );
                if send_event(&results, event, &notify).await.is_err() {
                    drop(permit);
                    break;
                }
                sequence += 1;
                drop(permit);
                continue;
            }
        };
        let payloads = rendered.payloads;
        let (head, body) = split_raw_request(&rendered.raw);
        let request = match RepeaterRequest::parse(
            head,
            body.as_bytes().to_vec(),
            &plan.scheme,
            &plan.host,
        ) {
            Ok(request) => request,
            Err(err) => {
                let event = result_event(
                    attack_id,
                    sequence,
                    payloads,
                    Duration::ZERO,
                    RepeaterResult::Failed {
                        error: err.to_string(),
                    },
                );
                if send_event(&results, event, &notify).await.is_err() {
                    drop(permit);
                    break;
                }
                sequence = sequence.saturating_add(1);
                drop(permit);
                continue;
            }
        };
        if let Some(error) = out_of_scope_target(&request, &plan.scope) {
            let event = result_event(
                attack_id,
                sequence,
                payloads,
                Duration::ZERO,
                RepeaterResult::Failed { error },
            );
            if send_event(&results, event, &notify).await.is_err() {
                drop(permit);
                break;
            }
            sequence = sequence.saturating_add(1);
            drop(permit);
            continue;
        }
        let client = client.clone();
        let results = results.clone();
        let notify = notify.clone();
        tasks.spawn(async move {
            let started = Instant::now();
            let result = execute_repeater(&client, request, capture_limit, timeout).await;
            let event = result_event(attack_id, sequence, payloads, started.elapsed(), result);
            let _ = send_event(&results, event, &notify).await;
            drop(permit);
        });
        sequence = sequence.saturating_add(1);
    }

    while tasks.join_next().await.is_some() {}
    running.store(false, Ordering::Relaxed);
    let _ = send_event(
        &results,
        IntruderEvent::Finished {
            attack_id,
            completed: total.min(sequence.saturating_sub(1)),
            total,
            cancelled,
        },
        &notify,
    )
    .await;
}

/// Reject a generated request whose effective target is outside the plan's
/// scope. The desktop workspace performs the same check before starting, but a
/// payload can still change an absolute-form request line, so every generated
/// request is checked again.
fn out_of_scope_target(request: &RepeaterRequest, scope: &[String]) -> Option<String> {
    if scope.is_empty() {
        return None;
    }
    let Some(host) = request
        .uri
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_string))
    else {
        return Some(
            "the generated request target could not be checked against the enforced scope"
                .to_string(),
        );
    };
    if host_in_scope(&host, scope.iter().map(String::as_str)) {
        None
    } else {
        Some(format!(
            "{host} is outside the enforced scope; the generated request was not sent"
        ))
    }
}

fn result_event(
    attack_id: u64,
    sequence: u64,
    payloads: Vec<String>,
    duration: Duration,
    result: RepeaterResult,
) -> IntruderEvent {
    IntruderEvent::Result(IntruderResult {
        attack_id,
        sequence,
        payloads,
        duration,
        result,
    })
}

async fn send_event(
    results: &mpsc::Sender<IntruderEvent>,
    event: IntruderEvent,
    notify: &Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<(), ()> {
    results.send(event).await.map_err(|_| ())?;
    if let Some(notify) = notify {
        notify();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(attack: AttackType, template: &str, sets: &[&[&str]]) -> IntruderPlan {
        IntruderPlan {
            attack,
            scheme: "http".to_string(),
            host: "127.0.0.1:1".to_string(),
            template: template.to_string(),
            payload_sets: sets
                .iter()
                .map(|set| set.iter().map(|value| value.to_string()).collect())
                .collect(),
            concurrency: 2,
            scope: Vec::new(),
        }
    }

    #[test]
    fn marker_ranges_are_utf8_safe_and_reject_odd_delimiters() {
        let ranges = marker_ranges("GET /?q=$$héllo$$ HTTP/1.1\n\n").unwrap();
        let text = "GET /?q=$$héllo$$ HTTP/1.1\n\n";
        assert_eq!(&text[ranges[0].clone()], "héllo");
        assert!(matches!(
            marker_ranges("GET /?q=$$unclosed"),
            Err(IntruderError::UnbalancedMarker { .. })
        ));
        assert_eq!(
            marker_ranges("no markers").unwrap(),
            Vec::<Range<usize>>::new()
        );
    }

    #[test]
    fn sniper_tests_each_position_without_replacing_the_others() {
        let plan = plan(
            AttackType::Sniper,
            "GET /?q=$$a$$&lang=$$en$$ HTTP/1.1\n\n",
            &[&["x", "y"]],
        );
        let prepared = plan.prepare().unwrap();
        assert_eq!(prepared.total(), 4);
        let rendered: Vec<_> = (1..=4)
            .map(|sequence| prepared.render(sequence).unwrap())
            .collect();
        assert_eq!(rendered[0].raw, "GET /?q=x&lang=en HTTP/1.1\n\n");
        assert_eq!(rendered[1].raw, "GET /?q=y&lang=en HTTP/1.1\n\n");
        assert_eq!(rendered[2].raw, "GET /?q=a&lang=x HTTP/1.1\n\n");
        assert_eq!(rendered[3].raw, "GET /?q=a&lang=y HTTP/1.1\n\n");
        assert_eq!(rendered[0].payloads, vec!["x"]);
    }

    #[test]
    fn battering_ram_replaces_every_position_with_one_payload() {
        let plan = plan(
            AttackType::BatteringRam,
            "POST / HTTP/1.1\n\nuser=$$a$$&pass=$$b$$",
            &[&["same"]],
        );
        assert_eq!(
            plan.render_sequence(1).unwrap().raw,
            "POST / HTTP/1.1\n\nuser=same&pass=same"
        );
    }

    #[test]
    fn pitchfork_stops_at_the_smallest_set() {
        let plan = plan(
            AttackType::Pitchfork,
            "GET /?u=$$a$$&p=$$b$$ HTTP/1.1\n\n",
            &[&["alice", "bob"], &["one", "two", "three"]],
        );
        assert_eq!(plan.request_count().unwrap(), 2);
        assert_eq!(
            plan.render_sequence(2).unwrap().raw,
            "GET /?u=bob&p=two HTTP/1.1\n\n"
        );
    }

    #[test]
    fn cluster_bomb_advances_the_last_position_fastest() {
        let plan = plan(
            AttackType::ClusterBomb,
            "GET /?u=$$a$$&p=$$b$$ HTTP/1.1\n\n",
            &[&["alice", "bob"], &["one", "two"]],
        );
        assert_eq!(plan.request_count().unwrap(), 4);
        let rendered: Vec<_> = (1..=4)
            .map(|sequence| plan.render_sequence(sequence).unwrap().raw)
            .collect();
        assert_eq!(rendered[0], "GET /?u=alice&p=one HTTP/1.1\n\n");
        assert_eq!(rendered[1], "GET /?u=alice&p=two HTTP/1.1\n\n");
        assert_eq!(rendered[2], "GET /?u=bob&p=one HTTP/1.1\n\n");
        assert_eq!(rendered[3], "GET /?u=bob&p=two HTTP/1.1\n\n");
    }

    #[test]
    fn empty_positions_and_unbalanced_markers_are_rejected() {
        assert!(matches!(
            plan(AttackType::Sniper, "GET / HTTP/1.1\n\n", &[&["x"]]).prepare(),
            Err(IntruderError::NoPositions)
        ));
        assert!(matches!(
            plan(AttackType::Sniper, "GET /?q=$$a$$ HTTP/1.1\n\n", &[&[]]).prepare(),
            Err(IntruderError::EmptyPayloadSet { .. })
        ));
        assert!(matches!(
            plan(
                AttackType::Pitchfork,
                "GET /?q=$$a$$&p=$$b$$ HTTP/1.1\n\n",
                &[&["x"]]
            )
            .prepare(),
            Err(IntruderError::MissingPayloadSet { .. })
        ));
    }

    #[test]
    fn toggle_marker_wraps_unwraps_and_handles_empty_positions() {
        let text = "GET /?q=value HTTP/1.1\n\n";
        let start = text.find("value").unwrap();
        let (marked, range) = toggle_marker(text, start..start + 5);
        assert_eq!(marked, "GET /?q=$$value$$ HTTP/1.1\n\n");
        assert_eq!(&marked[range.clone()], "value");

        let (unmarked, _) = toggle_marker(&marked, range);
        assert_eq!(unmarked, text);

        let (empty, range) = toggle_marker(text, start..start);
        assert_eq!(empty, "GET /?q=$$$$value HTTP/1.1\n\n");
        assert_eq!(range, start + 2..start + 2);
        let (restored, _) = toggle_marker(&empty, range);
        assert_eq!(restored, text);
    }

    #[test]
    fn auto_mark_covers_query_form_and_cookie_values() {
        let request = "POST /search?q=term&page=2 HTTP/1.1\nHost: example.test\nCookie: session=abc; theme=dark\nContent-Type: application/x-www-form-urlencoded\n\nuser=alice&pass=secret";
        let (marked, count) = auto_mark(request);
        assert_eq!(count, 6);
        assert!(marked.contains("q=$$term$$"));
        assert!(marked.contains("page=$$2$$"));
        assert!(marked.contains("session=$$abc$$"));
        assert!(marked.contains("theme=$$dark$$"));
        assert!(marked.contains("user=$$alice$$"));
        assert!(marked.contains("pass=$$secret$$"));

        let (again, count) = auto_mark(&marked);
        assert_eq!(count, 0);
        assert_eq!(again, marked);
    }

    #[test]
    fn auto_mark_skips_values_already_inside_a_position() {
        let request = "GET /?q=$$term$$&page=2 HTTP/1.1\nHost: example.test\n\n";
        let (marked, count) = auto_mark(request);
        assert_eq!(count, 1);
        assert!(marked.contains("q=$$term$$"));
        assert!(marked.contains("page=$$2$$"));
    }

    #[test]
    fn update_host_header_replaces_or_inserts_a_header() {
        let request = "GET / HTTP/1.1\r\nHost: old.test\r\nAccept: */*\r\n\r\n";
        assert_eq!(
            update_host_header(request, "new.test"),
            "GET / HTTP/1.1\r\nHost: new.test\r\nAccept: */*\r\n\r\n"
        );
        let no_host = "GET / HTTP/1.1\nAccept: */*\n\n";
        assert_eq!(
            update_host_header(no_host, "new.test"),
            "GET / HTTP/1.1\nHost: new.test\nAccept: */*\n\n"
        );
        assert_eq!(update_host_header(request, ""), request);
    }

    #[test]
    fn request_counts_fail_instead_of_overflowing() {
        let plan = IntruderPlan {
            attack: AttackType::ClusterBomb,
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            template: "GET /?a=$$a$$&b=$$b$$ HTTP/1.1\n\n".to_string(),
            payload_sets: vec![vec!["x".to_string(); 2], vec!["y".to_string(); 2]],
            concurrency: 1,
            scope: Vec::new(),
        };
        assert_eq!(plan.request_count().unwrap(), 4);

        // 65,536 is 2^16; four non-empty sets make the product exceed u64
        // without allocating billions of payload strings.
        let huge = IntruderPlan {
            attack: AttackType::ClusterBomb,
            template: "GET /?a=$$a$$&b=$$b$$&c=$$c$$&d=$$d$$ HTTP/1.1\n\n".to_string(),
            payload_sets: (0..4).map(|_| vec![String::new(); 65_536]).collect(),
            ..plan
        };
        assert!(matches!(
            huge.request_count(),
            Err(IntruderError::RequestCountOverflow)
        ));
    }
}
