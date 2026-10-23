//! Intruder templates: attack types, marker parsing, and request rendering.

use std::ops::Range;

/// The pair of dollar signs around a payload position.
pub const MARKER: &str = "$$";

/// Bound on simultaneous in-flight attack requests. The UI offers the same
/// range, and the engine clamps values from library callers.
pub const MAX_INTRUDER_CONCURRENCY: usize = 32;
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
