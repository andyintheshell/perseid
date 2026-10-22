//! Word- and byte-level comparison for the Comparer tool.
//!
//! The comparison engine is deliberately independent of egui. It produces
//! byte ranges for each side plus a change classification, which the UI maps
//! onto text and hex views. Word mode follows Burp's model: data is tokenized
//! on whitespace and the token-level edits are what get highlighted. Byte mode
//! compares the raw bytes.
//!
//! Comparisons are bounded. Word mode limits the input and token count, while
//! byte mode has a tighter size limit because Myers' algorithm is much more
//! expensive on byte sequences. A deadline gives both modes a predictable
//! upper bound even for adversarial input.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, capture_diff_slices_deadline};

/// Maximum item size accepted by the Comparer UI.
pub const MAX_ITEM_BYTES: usize = 8 * 1024 * 1024;
/// Maximum number of items kept in the shared item list.
pub const MAX_ITEMS: usize = 200;
/// Maximum input size for a word comparison, per item.
pub const MAX_WORD_COMPARE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum number of whitespace-delimited words considered in word mode.
pub const MAX_WORD_TOKENS: usize = 250_000;
/// Maximum input size for a byte comparison, per item.
pub const MAX_BYTE_COMPARE_BYTES: usize = 512 * 1024;
/// Wall-clock budget for the diff algorithm itself.
pub const DIFF_DEADLINE: Duration = Duration::from_millis(500);

/// Which comparison the user requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareMode {
    Words,
    Bytes,
}

impl CompareMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Words => "Word",
            Self::Bytes => "Byte",
        }
    }
}

/// How a byte range differs between the two items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The same logical position was replaced with different data.
    Modified,
    /// The data exists only in the first item.
    Deleted,
    /// The data exists only in the second item.
    Added,
}

/// One paired difference. Either range may be empty for a pure deletion or
/// addition, and both ranges are in the original item bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffSpan {
    pub kind: ChangeKind,
    pub old: Range<usize>,
    pub new: Range<usize>,
}

impl DiffSpan {
    fn modified(old: Range<usize>, new: Range<usize>) -> Self {
        Self {
            kind: ChangeKind::Modified,
            old,
            new,
        }
    }

    fn deleted(old: Range<usize>, at_new: usize) -> Self {
        Self {
            kind: ChangeKind::Deleted,
            old,
            new: at_new..at_new,
        }
    }

    fn added(at_old: usize, new: Range<usize>) -> Self {
        Self {
            kind: ChangeKind::Added,
            old: at_old..at_old,
            new,
        }
    }
}

/// The result of one comparison.
#[derive(Clone, Debug)]
pub struct Comparison {
    pub spans: Vec<DiffSpan>,
    /// Number of highlighted changes. A replacement token/block counts once;
    /// a pure insertion or deletion also counts once.
    pub differences: usize,
}

/// A bounded comparison failure suitable for showing in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareError {
    WordsTooLarge {
        limit: usize,
        old_len: usize,
        new_len: usize,
    },
    TooManyWords {
        limit: usize,
        old_count: usize,
        new_count: usize,
    },
    BytesTooLarge {
        limit: usize,
        old_len: usize,
        new_len: usize,
    },
}

impl fmt::Display for CompareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WordsTooLarge {
                limit,
                old_len,
                new_len,
            } => write!(
                f,
                "word comparison is limited to {} per item; the selected items are {old_len} and {new_len} bytes",
                human_limit(*limit)
            ),
            Self::TooManyWords {
                limit,
                old_count,
                new_count,
            } => write!(
                f,
                "word comparison is limited to {limit} words per item; the selected items contain {old_count} and {new_count} words"
            ),
            Self::BytesTooLarge {
                limit,
                old_len,
                new_len,
            } => write!(
                f,
                "byte comparison is limited to {} per item; the selected items are {old_len} and {new_len} bytes. Use Words for larger messages",
                human_limit(*limit)
            ),
        }
    }
}

impl std::error::Error for CompareError {}

fn human_limit(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * 1024;
    if bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else if bytes.is_multiple_of(KIB) {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Compare two items and return the paired differences.
pub fn compare(mode: CompareMode, old: &[u8], new: &[u8]) -> Result<Comparison, CompareError> {
    match mode {
        CompareMode::Words => compare_words(old, new),
        CompareMode::Bytes => compare_bytes(old, new),
    }
}

fn compare_words(old: &[u8], new: &[u8]) -> Result<Comparison, CompareError> {
    if old.len() > MAX_WORD_COMPARE_BYTES || new.len() > MAX_WORD_COMPARE_BYTES {
        return Err(CompareError::WordsTooLarge {
            limit: MAX_WORD_COMPARE_BYTES,
            old_len: old.len(),
            new_len: new.len(),
        });
    }

    let old_ranges = word_ranges(old);
    let new_ranges = word_ranges(new);
    if old_ranges.len() > MAX_WORD_TOKENS || new_ranges.len() > MAX_WORD_TOKENS {
        return Err(CompareError::TooManyWords {
            limit: MAX_WORD_TOKENS,
            old_count: old_ranges.len(),
            new_count: new_ranges.len(),
        });
    }

    let mut ids: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut next_id = 0u32;
    let old_keys = intern_words(old, &old_ranges, &mut ids, &mut next_id);
    let new_keys = intern_words(new, &new_ranges, &mut ids, &mut next_id);
    let ops = capture_diff_slices_deadline(
        Algorithm::Myers,
        &old_keys,
        &new_keys,
        Some(Instant::now() + DIFF_DEADLINE),
    );

    let mut spans = Vec::new();
    for op in ops {
        match op {
            similar::DiffOp::Equal { .. } => {}
            similar::DiffOp::Delete {
                old_index,
                old_len,
                new_index,
                ..
            } => {
                let at_new = new_ranges
                    .get(new_index)
                    .map_or(new.len(), |range| range.start);
                for range in &old_ranges[old_index..old_index + old_len] {
                    spans.push(DiffSpan::deleted(range.clone(), at_new));
                }
            }
            similar::DiffOp::Insert {
                old_index,
                new_index,
                new_len,
                ..
            } => {
                let at_old = old_ranges
                    .get(old_index)
                    .map_or(old.len(), |range| range.start);
                for range in &new_ranges[new_index..new_index + new_len] {
                    spans.push(DiffSpan::added(at_old, range.clone()));
                }
            }
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let paired = old_len.min(new_len);
                for offset in 0..paired {
                    spans.push(DiffSpan::modified(
                        old_ranges[old_index + offset].clone(),
                        new_ranges[new_index + offset].clone(),
                    ));
                }
                for range in &old_ranges[old_index + paired..old_index + old_len] {
                    let at_new = new_ranges
                        .get(new_index + paired)
                        .map_or(new.len(), |range| range.start);
                    spans.push(DiffSpan::deleted(range.clone(), at_new));
                }
                let at_old = old_ranges
                    .get(old_index + paired)
                    .map_or(old.len(), |range| range.start);
                for range in &new_ranges[new_index + paired..new_index + new_len] {
                    spans.push(DiffSpan::added(at_old, range.clone()));
                }
            }
        }
    }

    Ok(Comparison {
        differences: spans.len(),
        spans,
    })
}

fn compare_bytes(old: &[u8], new: &[u8]) -> Result<Comparison, CompareError> {
    if old.len() > MAX_BYTE_COMPARE_BYTES || new.len() > MAX_BYTE_COMPARE_BYTES {
        return Err(CompareError::BytesTooLarge {
            limit: MAX_BYTE_COMPARE_BYTES,
            old_len: old.len(),
            new_len: new.len(),
        });
    }

    let ops = capture_diff_slices_deadline(
        Algorithm::Myers,
        old,
        new,
        Some(Instant::now() + DIFF_DEADLINE),
    );
    let mut spans = Vec::new();
    for op in ops {
        match op {
            similar::DiffOp::Equal { .. } => {}
            similar::DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => spans.push(DiffSpan::deleted(old_index..old_index + old_len, new_index)),
            similar::DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => spans.push(DiffSpan::added(old_index, new_index..new_index + new_len)),
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => spans.push(DiffSpan::modified(
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            )),
        }
    }

    Ok(Comparison {
        differences: spans.len(),
        spans,
    })
}

#[derive(Clone, Debug)]
struct MapSegment {
    raw: Range<usize>,
    display: Range<usize>,
    /// True when offsets inside this segment map one-to-one onto the display
    /// string. ASCII runs and normalised `\r` are linear; multi-byte UTF-8 and
    /// escaped control bytes are not, so a byte range snaps to character
    /// boundaries when it is highlighted.
    linear: bool,
}

/// A display-safe rendering of arbitrary bytes, plus the mapping needed to
/// translate diff ranges in the original bytes back onto it.
#[derive(Clone, Debug)]
pub struct DisplayText {
    text: String,
    raw_len: usize,
    segments: Vec<MapSegment>,
    line_starts: Vec<usize>,
}

impl DisplayText {
    pub fn new(bytes: &[u8]) -> Self {
        let mut text = String::with_capacity(bytes.len());
        let mut segments: Vec<MapSegment> = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            let raw_start = index;
            let display_start = text.len();
            let byte = bytes[index];
            let linear;
            if byte == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                text.push('\n');
                index += 2;
                linear = false;
            } else if byte == b'\r' {
                text.push('\n');
                index += 1;
                linear = true;
            } else if byte == b'\n' || byte == b'\t' || (0x20..=0x7e).contains(&byte) {
                text.push(char::from(byte));
                index += 1;
                linear = true;
            } else if byte < 0x80 {
                text.push('·');
                index += 1;
                linear = false;
            } else if let Some((character, width)) = decode_utf8_char(&bytes[index..]) {
                if character.is_control() {
                    text.push('·');
                } else {
                    text.push(character);
                }
                index += width;
                linear = false;
            } else {
                text.push('·');
                index += 1;
                linear = false;
            }
            push_map_segment(
                &mut segments,
                raw_start..index,
                display_start..text.len(),
                linear,
            );
        }

        let mut line_starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }

        Self {
            text,
            raw_len: bytes.len(),
            segments,
            line_starts,
        }
    }

    #[cfg(test)]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn raw_len(&self) -> usize {
        self.raw_len
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_range(&self, line: usize) -> Range<usize> {
        let Some(&start) = self.line_starts.get(line) else {
            return 0..0;
        };
        let end = self
            .line_starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next.saturating_sub(1));
        start..end
    }

    pub fn line(&self, line: usize) -> &str {
        let range = self.line_range(line);
        self.text.get(range).unwrap_or_default()
    }

    /// Map a range of original bytes onto the display string. Empty ranges are
    /// mapped to an insertion point.
    pub fn display_range(&self, raw: Range<usize>) -> Range<usize> {
        let start = raw.start.min(self.raw_len);
        let end = raw.end.min(self.raw_len);
        if start >= end {
            let point = self.point(start);
            return point..point;
        }

        let start_segment = self.segment_containing(start);
        let end_segment = self.segment_containing(end - 1);
        let display_start = if start_segment.linear {
            start_segment.display.start + (start - start_segment.raw.start)
        } else {
            start_segment.display.start
        };
        let display_end = if end_segment.linear {
            end_segment.display.start + (end - end_segment.raw.start)
        } else {
            end_segment.display.end
        };
        display_start..display_end.max(display_start)
    }

    pub fn line_for_raw(&self, raw: usize) -> usize {
        self.line_for_display(self.point(raw.min(self.raw_len)))
    }

    pub fn line_for_display(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        self.line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    fn point(&self, raw: usize) -> usize {
        if raw >= self.raw_len {
            return self.text.len();
        }
        let segment = self.segment_containing(raw);
        if segment.linear {
            segment.display.start + (raw - segment.raw.start)
        } else {
            segment.display.start
        }
    }

    fn segment_containing(&self, raw: usize) -> &MapSegment {
        let index = self
            .segments
            .partition_point(|segment| segment.raw.end <= raw);
        &self.segments[index.min(self.segments.len().saturating_sub(1))]
    }
}

fn push_map_segment(
    segments: &mut Vec<MapSegment>,
    raw: Range<usize>,
    display: Range<usize>,
    linear: bool,
) {
    if let Some(last) = segments.last_mut()
        && last.linear
        && linear
        && last.raw.end == raw.start
        && last.display.end == display.start
    {
        last.raw.end = raw.end;
        last.display.end = display.end;
        return;
    }
    segments.push(MapSegment {
        raw,
        display,
        linear,
    });
}

fn decode_utf8_char(bytes: &[u8]) -> Option<(char, usize)> {
    let width = match *bytes.first()? {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let slice = bytes.get(..width)?;
    let text = std::str::from_utf8(slice).ok()?;
    let mut chars = text.chars();
    let character = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some((character, width))
}

fn word_ranges(bytes: &[u8]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        ranges.push(start..index);
    }
    ranges
}

fn intern_words(
    bytes: &[u8],
    ranges: &[Range<usize>],
    ids: &mut HashMap<Vec<u8>, u32>,
    next_id: &mut u32,
) -> Vec<u32> {
    ranges
        .iter()
        .map(|range| {
            let word = &bytes[range.clone()];
            if let Some(id) = ids.get(word) {
                *id
            } else {
                let id = *next_id;
                *next_id = next_id.saturating_add(1);
                ids.insert(word.to_vec(), id);
                id
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(comparison: &Comparison) -> Vec<ChangeKind> {
        comparison.spans.iter().map(|span| span.kind).collect()
    }

    #[test]
    fn words_are_split_on_whitespace() {
        let ranges = word_ranges(b"  alpha\tbeta\r\ngamma  ");
        assert_eq!(
            ranges,
            vec![2..7, 8..12, 14..19],
            "ranges should point at the original word bytes"
        );
    }

    #[test]
    fn identical_words_have_no_differences() {
        let result = compare(CompareMode::Words, b"alpha beta\n", b"alpha beta\n").unwrap();
        assert_eq!(result.differences, 0);
        assert!(result.spans.is_empty());
    }

    #[test]
    fn word_replacements_are_modified_and_count_once() {
        let result = compare(CompareMode::Words, b"alpha beta", b"alpha gamma").unwrap();
        assert_eq!(result.differences, 1);
        assert_eq!(kinds(&result), vec![ChangeKind::Modified]);
        assert_eq!(result.spans[0].old, 6..10);
        assert_eq!(result.spans[0].new, 6..11);
    }

    #[test]
    fn word_insertions_and_deletions_are_distinguished() {
        let result = compare(CompareMode::Words, b"alpha beta", b"alpha").unwrap();
        assert_eq!(result.differences, 1);
        assert_eq!(kinds(&result), vec![ChangeKind::Deleted]);
        assert_eq!(result.spans[0].old, 6..10);
        assert!(result.spans[0].new.is_empty());

        let result = compare(CompareMode::Words, b"alpha", b"alpha beta").unwrap();
        assert_eq!(result.differences, 1);
        assert_eq!(kinds(&result), vec![ChangeKind::Added]);
        assert!(result.spans[0].old.is_empty());
        assert_eq!(result.spans[0].new, 6..10);
    }

    #[test]
    fn word_position_argument_does_not_lose_edges() {
        let result = compare(CompareMode::Words, b"one two three", b"one three").unwrap();
        assert_eq!(result.differences, 1);
        assert_eq!(result.spans[0].old, 4..7);
        assert_eq!(result.spans[0].new, 4..4);
    }

    #[test]
    fn byte_differences_keep_their_byte_ranges() {
        let old = b"HTTP/1.1 200 OK\r\nX: cat\r\n\r\nbody";
        let new = b"HTTP/1.1 200 OK\r\nX: cut\r\n\r\nbody";
        let result = compare(CompareMode::Bytes, old, new).unwrap();
        assert_eq!(result.differences, 1);
        assert_eq!(result.spans[0].kind, ChangeKind::Modified);
        let old_changed = &old[result.spans[0].old.clone()];
        let new_changed = &new[result.spans[0].new.clone()];
        assert_eq!(old_changed, b"a");
        assert_eq!(new_changed, b"u");
    }

    #[test]
    fn display_text_sanitizes_control_and_invalid_bytes() {
        let display = DisplayText::new(b"a\r\nb\x00c\xffd");
        assert_eq!(display.as_str(), "a\nb·c·d");
        assert_eq!(display.raw_len(), 8);
        assert_eq!(display.line_count(), 2);
        assert_eq!(display.line(0), "a");
        assert_eq!(display.line(1), "b·c·d");
        assert_eq!(display.line_for_raw(0), 0);
        assert_eq!(display.line_for_raw(4), 1);
    }

    #[test]
    fn display_text_preserves_valid_utf8() {
        let display = DisplayText::new("héllo\nwörld".as_bytes());
        assert_eq!(display.as_str(), "héllo\nwörld");
        assert_eq!(display.line_count(), 2);
        assert_eq!(display.line(0), "héllo");
        assert_eq!(display.line(1), "wörld");
    }

    #[test]
    fn display_ranges_snap_to_character_boundaries() {
        let display = DisplayText::new("aébc".as_bytes());
        let range = display.display_range(1..2);
        assert_eq!(&display.as_str()[range.clone()], "é");
        let range = display.display_range(2..4);
        assert_eq!(&display.as_str()[range], "éb");
    }

    #[test]
    fn display_ranges_account_for_normalised_newlines_and_controls() {
        let display = DisplayText::new(b"a\r\nb\x00c");
        let range = display.display_range(1..3);
        assert_eq!(&display.as_str()[range], "\n");
        let range = display.display_range(3..4);
        assert_eq!(&display.as_str()[range], "b");
        let range = display.display_range(4..5);
        assert_eq!(&display.as_str()[range], "·");
        let range = display.display_range(5..6);
        assert_eq!(&display.as_str()[range], "c");
    }

    #[test]
    fn byte_mode_rejects_large_inputs() {
        let big = vec![b'x'; MAX_BYTE_COMPARE_BYTES + 1];
        let error = compare(CompareMode::Bytes, &big, b"x").unwrap_err();
        assert!(matches!(error, CompareError::BytesTooLarge { .. }));
        assert!(error.to_string().contains("512 KiB"));
    }

    #[test]
    fn word_mode_counts_tokens() {
        let error = compare(
            CompareMode::Words,
            "word ".repeat(MAX_WORD_TOKENS + 1).as_bytes(),
            b"",
        )
        .unwrap_err();
        assert!(matches!(error, CompareError::TooManyWords { .. }));
    }
}
