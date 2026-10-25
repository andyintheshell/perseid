//! Diff highlighting: paired differences mapped onto display text and bytes.
//!
//! Nothing here touches a [`egui::Ui`]; the result is plain data that the view
//! renders as text or hex panes.

use std::ops::Range;

use egui::Color32;

use super::state::ResultView;
use crate::comparer::{ChangeKind, DiffSpan, DisplayText};

pub(super) const HEX_BYTES_PER_ROW: usize = 16;
pub(super) const MODIFIED_BG: Color32 = Color32::from_rgb(0xcf, 0xe3, 0xf7);
pub(super) const DELETED_BG: Color32 = Color32::from_rgb(0xf8, 0xd8, 0xd2);
pub(super) const ADDED_BG: Color32 = Color32::from_rgb(0xd7, 0xf0, 0xdc);

#[derive(Clone, Debug)]
pub(super) struct Highlight {
    pub(super) range: Range<usize>,
    pub(super) kind: ChangeKind,
}

pub(super) struct CompareResult {
    mode: crate::comparer::CompareMode,
    old_index: usize,
    new_index: usize,
    /// Title shown in the result window.
    title: String,
    old_data: Vec<u8>,
    new_data: Vec<u8>,
    old_text: DisplayText,
    new_text: DisplayText,
    text_highlights: [Vec<Vec<Highlight>>; 2],
    byte_kinds: [Vec<Option<ChangeKind>>; 2],
    spans: Vec<DiffSpan>,
    differences: usize,
}

impl CompareResult {
    pub(super) fn new(
        mode: crate::comparer::CompareMode,
        old_index: usize,
        new_index: usize,
        old_data: Vec<u8>,
        new_data: Vec<u8>,
        comparison: crate::comparer::Comparison,
    ) -> Self {
        let old_text = DisplayText::new(&old_data);
        let new_text = DisplayText::new(&new_data);
        let text_highlights = [
            build_text_highlights(&old_text, &comparison.spans, 0),
            build_text_highlights(&new_text, &comparison.spans, 1),
        ];
        let byte_kinds = [
            build_byte_kinds(old_data.len(), &comparison.spans, 0),
            build_byte_kinds(new_data.len(), &comparison.spans, 1),
        ];
        let difference_word = if comparison.differences == 1 {
            "difference"
        } else {
            "differences"
        };
        let title = format!(
            "{} compare of #{} and #{} ({} {difference_word})",
            mode.label(),
            old_index + 1,
            new_index + 1,
            comparison.differences
        );
        Self {
            mode,
            old_index,
            new_index,
            title,
            old_data,
            new_data,
            old_text,
            new_text,
            text_highlights,
            byte_kinds,
            spans: comparison.spans,
            differences: comparison.differences,
        }
    }

    pub(super) fn mode(&self) -> crate::comparer::CompareMode {
        self.mode
    }

    pub(super) fn item_indices(&self) -> [usize; 2] {
        [self.old_index, self.new_index]
    }

    pub(super) fn title(&self) -> &str {
        &self.title
    }

    pub(super) fn differences(&self) -> usize {
        self.differences
    }

    pub(super) fn spans(&self) -> &[DiffSpan] {
        &self.spans
    }

    pub(super) fn data(&self, side: usize) -> &[u8] {
        if side == 0 {
            &self.old_data
        } else {
            &self.new_data
        }
    }

    pub(super) fn display(&self, side: usize) -> &DisplayText {
        if side == 0 {
            &self.old_text
        } else {
            &self.new_text
        }
    }

    pub(super) fn text_highlights(&self, side: usize) -> &[Vec<Highlight>] {
        &self.text_highlights[side]
    }

    pub(super) fn byte_kinds(&self, side: usize) -> &[Option<ChangeKind>] {
        &self.byte_kinds[side]
    }

    pub(super) fn side_label(&self, side: usize) -> String {
        format!(
            "#{}",
            if side == 0 {
                self.old_index + 1
            } else {
                self.new_index + 1
            }
        )
    }
}

pub(super) fn build_text_highlights(
    display: &DisplayText,
    spans: &[DiffSpan],
    side: usize,
) -> Vec<Vec<Highlight>> {
    let mut lines = vec![Vec::new(); display.line_count()];
    for span in spans {
        let raw = if side == 0 { &span.old } else { &span.new };
        if raw.is_empty() {
            continue;
        }
        let highlighted = display.display_range(raw.clone());
        if highlighted.is_empty() {
            continue;
        }
        let first_line = display.line_for_display(highlighted.start);
        let last_line = display.line_for_display(highlighted.end.saturating_sub(1));
        for (line, line_highlights) in lines
            .iter_mut()
            .enumerate()
            .take(last_line + 1)
            .skip(first_line)
        {
            let line_range = display.line_range(line);
            let start = highlighted.start.max(line_range.start);
            let end = highlighted.end.min(line_range.end);
            if start < end {
                line_highlights.push(Highlight {
                    range: start - line_range.start..end - line_range.start,
                    kind: span.kind,
                });
            }
        }
    }
    for line in &mut lines {
        line.sort_by_key(|highlight| highlight.range.start);
    }
    lines
}

pub(super) fn build_byte_kinds(
    length: usize,
    spans: &[DiffSpan],
    side: usize,
) -> Vec<Option<ChangeKind>> {
    let mut kinds = vec![None; length];
    for span in spans {
        let raw = if side == 0 { &span.old } else { &span.new };
        if raw.start < raw.end && raw.end <= length {
            kinds[raw.clone()].fill(Some(span.kind));
        }
    }
    kinds
}

pub(super) fn visible_line(display: &DisplayText, raw: &Range<usize>, view: ResultView) -> usize {
    match view {
        ResultView::Text => display.line_for_raw(raw.start),
        ResultView::Hex => raw.start / HEX_BYTES_PER_ROW,
    }
}

pub(super) fn change_background(kind: ChangeKind) -> Color32 {
    match kind {
        ChangeKind::Modified => MODIFIED_BG,
        ChangeKind::Deleted => DELETED_BG,
        ChangeKind::Added => ADDED_BG,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparer::CompareMode;

    fn result(mode: CompareMode, old: &[u8], new: &[u8]) -> CompareResult {
        let comparison = crate::comparer::compare(mode, old, new).unwrap();
        CompareResult::new(mode, 2, 4, old.to_vec(), new.to_vec(), comparison)
    }

    #[test]
    fn word_result_highlights_only_the_changed_word() {
        let result = result(CompareMode::Words, b"GET / HTTP/1.1", b"POST / HTTP/1.1");
        assert_eq!(result.differences, 1);
        let line = &result.text_highlights[0][0];
        assert_eq!(line.len(), 1);
        assert_eq!(&result.old_text.as_str()[line[0].range.clone()], "GET");
        let line = &result.text_highlights[1][0];
        assert_eq!(&result.new_text.as_str()[line[0].range.clone()], "POST");
    }

    #[test]
    fn byte_result_highlights_hex_bytes_and_text() {
        let result = result(CompareMode::Bytes, b"cat", b"cut");
        assert_eq!(result.byte_kinds[0][1], Some(ChangeKind::Modified));
        assert_eq!(result.byte_kinds[1][1], Some(ChangeKind::Modified));
        let line = &result.text_highlights[0][0];
        assert_eq!(&result.old_text.as_str()[line[0].range.clone()], "a");
    }

    #[test]
    fn result_titles_use_one_based_row_numbers() {
        let result = result(CompareMode::Words, b"a", b"b");
        assert_eq!(result.title, "Word compare of #3 and #5 (1 difference)");
    }

    #[test]
    fn text_highlights_clip_at_line_boundaries() {
        let display = DisplayText::new(b"abc\ndef");
        let spans = [
            DiffSpan {
                kind: ChangeKind::Modified,
                old: 2..5,
                new: 2..5,
            },
            // Empty and out-of-range spans map to insertion points and vanish.
            DiffSpan {
                kind: ChangeKind::Added,
                old: 1..1,
                new: 1..1,
            },
            DiffSpan {
                kind: ChangeKind::Deleted,
                old: 100..101,
                new: 100..101,
            },
        ];
        let lines = build_text_highlights(&display, &spans, 0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].range, 2..3);
        assert_eq!(lines[0][0].kind, ChangeKind::Modified);
        assert_eq!(lines[1].len(), 1);
        assert_eq!(lines[1][0].range, 0..1);
    }

    #[test]
    fn visible_line_maps_text_and_hex_offsets() {
        let display = DisplayText::new(b"ab\ncd\nef");
        assert_eq!(visible_line(&display, &(0..0), ResultView::Text), 0);
        assert_eq!(visible_line(&display, &(2..3), ResultView::Text), 0);
        assert_eq!(visible_line(&display, &(3..4), ResultView::Text), 1);
        assert_eq!(visible_line(&display, &(7..8), ResultView::Text), 2);
        assert_eq!(visible_line(&display, &(8..8), ResultView::Text), 2);
        assert_eq!(visible_line(&display, &(0..1), ResultView::Hex), 0);
        assert_eq!(visible_line(&display, &(16..17), ResultView::Hex), 1);
        assert_eq!(visible_line(&display, &(32..33), ResultView::Hex), 2);
    }

    #[test]
    fn change_background_matches_each_change_kind() {
        assert_eq!(change_background(ChangeKind::Modified), MODIFIED_BG);
        assert_eq!(change_background(ChangeKind::Deleted), DELETED_BG);
        assert_eq!(change_background(ChangeKind::Added), ADDED_BG);
        assert_ne!(MODIFIED_BG, DELETED_BG);
        assert_ne!(DELETED_BG, ADDED_BG);
    }
}
