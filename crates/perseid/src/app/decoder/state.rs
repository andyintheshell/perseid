//! Decoder panel state and the pure helpers used to translate selections.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};

use crate::decoder as engine;
use crate::decoder::normalize_selection;

/// State for the Decoder tab.
pub(in crate::app) struct DecoderState {
    pub(super) panels: Vec<Panel>,
    pub(super) next_id: u64,
    pub(super) scroll_to_end: bool,
}

impl DecoderState {
    pub(in crate::app) fn new() -> Self {
        let mut state = Self {
            panels: Vec::new(),
            next_id: 1,
            scroll_to_end: false,
        };
        state.reset();
        state
    }

    /// Seed the documentation screenshot with a small, readable pipeline.
    #[cfg(feature = "screenshots")]
    pub(in crate::app) fn seed_screenshot(&mut self) {
        self.panels.clear();
        let id = self.allocate_id();
        self.panels.push(Panel::new(
            id,
            b"alice:correct horse battery staple".to_vec(),
            Vec::new(),
            None,
        ));
        self.apply(0, engine::Transform::Encode(engine::Codec::Base64), None);
        self.apply(
            1,
            engine::Transform::Hash(engine::HashAlgorithm::Sha256),
            None,
        );
        self.scroll_to_end = false;
    }

    pub(super) fn reset(&mut self) {
        self.panels.clear();
        let id = self.allocate_id();
        self.panels
            .push(Panel::new(id, Vec::new(), Vec::new(), None));
        self.scroll_to_end = false;
    }

    pub(in crate::app) fn receive(&mut self, bytes: Vec<u8>, source: String, replace: bool) {
        if replace || (self.panels.len() == 1 && self.panels[0].data.is_empty()) {
            self.panels.clear();
        }
        let id = self.allocate_id();
        let mut panel = Panel::new(id, bytes, Vec::new(), None);
        panel.notice = Some(source);
        self.panels.push(panel);
        self.scroll_to_end = true;
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub(super) fn run_action(&mut self, action: Action) {
        match action {
            Action::Transform {
                index,
                operation,
                selection,
            } => self.apply(index, operation, selection),
            Action::Smart { index, selection } => self.smart(index, selection),
        }
    }

    fn apply(
        &mut self,
        index: usize,
        operation: engine::Transform,
        selection: Option<Range<usize>>,
    ) {
        let result = {
            let Some(source) = self.panels.get(index) else {
                return;
            };
            engine::transform(&source.data, selection, operation)
        };
        self.panels[index].error = None;
        self.panels[index].notice = None;
        match result {
            Ok(transformed) => {
                let id = self.allocate_id();
                let panel = Panel::new(id, transformed.data, transformed.spans, Some(operation));
                self.panels.truncate(index + 1);
                self.panels.push(panel);
                self.scroll_to_end = true;
            }
            Err(error) => {
                self.panels[index].error = Some(error.to_string());
            }
        }
    }

    fn smart(&mut self, index: usize, selection: Option<Range<usize>>) {
        let (prefix, selected, suffix) = {
            let Some(source) = self.panels.get(index) else {
                return;
            };
            let range = normalize_selection(source.data.len(), selection);
            (
                source.data[..range.start].to_vec(),
                source.data[range.clone()].to_vec(),
                source.data[range.end..].to_vec(),
            )
        };
        self.panels[index].error = None;
        self.panels[index].notice = None;
        let steps = engine::smart_decode(&selected);
        if steps.is_empty() {
            self.panels[index].notice = Some("No recognizable encoding found.".to_string());
            return;
        }

        let mut panels = Vec::with_capacity(steps.len());
        for step in steps {
            let id = self.allocate_id();
            let mut data = Vec::with_capacity(prefix.len() + step.data.len() + suffix.len());
            data.extend_from_slice(&prefix);
            let start = data.len();
            data.extend_from_slice(&step.data);
            let end = data.len();
            data.extend_from_slice(&suffix);
            let spans = if start == end {
                Vec::new()
            } else {
                vec![engine::Span {
                    start,
                    end,
                    tag: engine::ColorTag::Codec(step.codec),
                }]
            };
            panels.push(Panel::new(
                id,
                data,
                spans,
                Some(engine::Transform::Decode(step.codec)),
            ));
        }
        self.panels.truncate(index + 1);
        self.panels.extend(panels);
        self.scroll_to_end = true;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ViewMode {
    #[default]
    Text,
    Hex,
}

pub(super) struct Panel {
    pub(super) id: u64,
    pub(super) data: Vec<u8>,
    pub(super) text: String,
    pub(super) text_valid: bool,
    pub(super) spans: Vec<engine::Span>,
    pub(super) operation: Option<engine::Transform>,
    pub(super) view: ViewMode,
    pub(super) selection: Option<Range<usize>>,
    pub(super) context_menu_open: bool,
    pub(super) error: Option<String>,
    pub(super) notice: Option<String>,
}

impl Panel {
    pub(super) fn new(
        id: u64,
        data: Vec<u8>,
        spans: Vec<engine::Span>,
        operation: Option<engine::Transform>,
    ) -> Self {
        let text_valid = std::str::from_utf8(&data).is_ok();
        let text = String::from_utf8_lossy(&data).into_owned();
        let view = if text_valid && engine::is_probably_text(&data) {
            ViewMode::Text
        } else {
            ViewMode::Hex
        };
        Self {
            id,
            data,
            text,
            text_valid,
            spans,
            operation,
            view,
            selection: None,
            context_menu_open: false,
            error: None,
            notice: None,
        }
    }

    pub(super) fn editor_id(&self) -> egui::Id {
        egui::Id::unique(("perseid.decoder.editor", self.id))
    }

    pub(super) fn sync_text(&mut self) {
        self.data = self.text.as_bytes().to_vec();
        self.text_valid = true;
        self.spans.clear();
        self.selection = None;
        self.error = None;
        self.notice = None;
    }

    pub(super) fn copy_value(&self) -> String {
        if self.view == ViewMode::Hex || !self.text_valid || !engine::is_probably_text(&self.data) {
            hex_copy(&self.data)
        } else {
            self.text.clone()
        }
    }

    pub(super) fn binary(&self) -> bool {
        !self.text_valid || !engine::is_probably_text(&self.data)
    }
}

pub(super) enum Action {
    Transform {
        index: usize,
        operation: engine::Transform,
        selection: Option<Range<usize>>,
    },
    Smart {
        index: usize,
        selection: Option<Range<usize>>,
    },
}

pub(super) fn selection_for(panel: &Panel) -> Option<Range<usize>> {
    if panel.view != ViewMode::Text || !panel.text_valid {
        return None;
    }
    panel.selection.clone()
}

/// Convert a byte range in `text` to the character range used by egui.
pub(super) fn char_range_for_selection(
    text: &str,
    selection: &Range<usize>,
) -> Option<CCursorRange> {
    text.get(selection.clone())?;
    let start = text[..selection.start].chars().count();
    let end = text[..selection.end].chars().count();
    Some(CCursorRange::two(CCursor::new(start), CCursor::new(end)))
}

pub(super) fn char_range_to_byte_range(
    text: &str,
    start: usize,
    end: usize,
) -> Option<Range<usize>> {
    if start >= end {
        return None;
    }
    let byte_start = text.char_indices().nth(start)?.0;
    let byte_end = text
        .char_indices()
        .nth(end)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len());
    (byte_start < byte_end).then_some(byte_start..byte_end)
}

pub(super) fn tag_at(spans: &[engine::Span], offset: usize) -> Option<engine::ColorTag> {
    spans
        .iter()
        .find(|span| span.start <= offset && offset < span.end)
        .map(|span| span.tag)
}

pub(super) fn panel_control_width(ui: &egui::Ui) -> f32 {
    ui.available_width().max(160.0)
}

pub(super) fn hex_copy(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len().saturating_mul(3));
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_root(state: &mut DecoderState, bytes: &[u8]) {
        let panel = &mut state.panels[0];
        panel.data = bytes.to_vec();
        panel.text = String::from_utf8_lossy(bytes).into_owned();
        panel.text_valid = std::str::from_utf8(bytes).is_ok();
        panel.spans.clear();
        panel.view = ViewMode::Text;
    }

    #[test]
    fn receive_preserves_chains_or_replaces_them_explicitly() {
        let mut state = DecoderState::new();
        state.receive(vec![0, 255], "binary".into(), false);
        assert_eq!(state.panels.len(), 1);
        assert_eq!(state.panels[0].data, [0, 255]);
        assert_eq!(state.panels[0].view, ViewMode::Hex);
        state.receive(b"second".to_vec(), "selection".into(), false);
        assert_eq!(state.panels.len(), 2);
        assert_eq!(state.panels[0].data, [0, 255]);
        assert_eq!(state.panels[1].notice.as_deref(), Some("selection"));
        state.receive(b"new".to_vec(), "request".into(), true);
        assert_eq!(state.panels.len(), 1);
        assert_eq!(state.panels[0].data, b"new");
    }

    #[test]
    fn char_ranges_convert_to_utf8_byte_ranges() {
        let text = "aé中z";
        assert_eq!(char_range_to_byte_range(text, 0, 1), Some(0..1));
        assert_eq!(char_range_to_byte_range(text, 1, 2), Some(1..3));
        assert_eq!(char_range_to_byte_range(text, 2, 3), Some(3..6));
        assert_eq!(char_range_to_byte_range(text, 3, 4), Some(6..7));
        assert_eq!(char_range_to_byte_range(text, 2, 2), None);
    }

    #[test]
    fn context_selection_uses_character_offsets() {
        let range = char_range_for_selection("héllo", &(1..3)).expect("valid selection");
        let range = range.as_sorted_char_range();
        assert_eq!((range.start.0, range.end.0), (1, 2));
        assert!(char_range_for_selection("héllo", &(0..2)).is_none());
    }

    #[test]
    fn operations_replace_the_downstream_chain() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"a%20b");
        state.apply(0, engine::Transform::Decode(engine::Codec::Url), None);
        assert_eq!(state.panels.len(), 2);
        assert_eq!(state.panels[1].data, b"a b");
        assert_eq!(
            state.panels[1].operation,
            Some(engine::Transform::Decode(engine::Codec::Url))
        );

        state.apply(0, engine::Transform::Encode(engine::Codec::Html), None);
        assert_eq!(state.panels.len(), 2);
        assert_eq!(state.panels[1].data, b"&#x61;&#x25;&#x32;&#x30;&#x62;");
    }

    #[test]
    fn selections_are_transformed_without_replacing_the_rest() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"hello world");
        state.apply(
            0,
            engine::Transform::Encode(engine::Codec::Url),
            Some(6..11),
        );
        assert_eq!(state.panels.len(), 2);
        assert_eq!(state.panels[1].data, b"hello %77%6f%72%6c%64");
        assert_eq!(state.panels[1].spans[0].start, 6);
    }

    #[test]
    fn smart_decode_adds_one_panel_per_layer() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"%253Cb%253E");
        state.smart(0, None);
        assert_eq!(state.panels.len(), 3);
        assert_eq!(state.panels[1].data, b"%3Cb%3E");
        assert_eq!(state.panels[2].data, b"<b>");
        assert_eq!(
            state.panels[2].operation,
            Some(engine::Transform::Decode(engine::Codec::Url))
        );
    }

    #[test]
    fn smart_decode_reports_when_nothing_is_recognized() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"hello world");
        state.smart(0, None);
        assert_eq!(state.panels.len(), 1);
        assert!(state.panels[0].notice.is_some());
    }

    #[test]
    fn editing_text_updates_the_canonical_bytes() {
        let mut state = DecoderState::new();
        state.panels[0].text = "updated payload".to_string();
        state.panels[0].sync_text();
        assert_eq!(state.panels[0].data, b"updated payload");
        assert!(state.panels[0].spans.is_empty());
        assert!(state.panels[0].selection.is_none());
    }

    #[test]
    fn saved_selection_is_used_only_for_text_panels() {
        let mut state = DecoderState::new();
        set_root(&mut state, b"hello world");
        state.panels[0].selection = Some(6..11);
        assert_eq!(selection_for(&state.panels[0]), Some(6..11));

        state.panels[0].view = ViewMode::Hex;
        assert_eq!(selection_for(&state.panels[0]), None);

        state.panels[0].view = ViewMode::Text;
        state.panels[0].text_valid = false;
        assert_eq!(selection_for(&state.panels[0]), None);
    }

    #[test]
    fn panels_default_to_text_or_hex_from_their_bytes() {
        let text = Panel::new(1, b"hello".to_vec(), Vec::new(), None);
        assert_eq!(text.view, ViewMode::Text);
        assert!(!text.binary());

        let invalid_utf8 = Panel::new(2, vec![0x00, 0xff], Vec::new(), None);
        assert_eq!(invalid_utf8.view, ViewMode::Hex);
        assert!(invalid_utf8.binary());

        let control_heavy = Panel::new(3, b"ab\x01".to_vec(), Vec::new(), None);
        assert_eq!(control_heavy.view, ViewMode::Hex);
        assert!(control_heavy.binary());
    }

    #[test]
    fn copy_value_uses_text_for_text_panels_and_hex_otherwise() {
        let text = Panel::new(1, b"hello".to_vec(), Vec::new(), None);
        assert_eq!(text.copy_value(), "hello");

        let binary = Panel::new(2, vec![0x00, 0xff], Vec::new(), None);
        assert_eq!(binary.copy_value(), "00 ff");

        let mut forced_hex = Panel::new(3, b"hello".to_vec(), Vec::new(), None);
        forced_hex.view = ViewMode::Hex;
        assert_eq!(forced_hex.copy_value(), "68 65 6c 6c 6f");
    }

    #[test]
    fn hex_copy_formats_every_byte_and_handles_large_inputs() {
        assert_eq!(hex_copy(&[]), "");
        assert_eq!(hex_copy(&[0x00, 0x0f, 0xff]), "00 0f ff");

        let large = vec![0xab; 70_000];
        let copied = hex_copy(&large);
        assert_eq!(copied.len(), large.len() * 3 - 1);
        assert!(copied.starts_with("ab ab ab"));
        assert!(copied.ends_with("ab"));
    }

    #[test]
    fn tag_at_uses_half_open_span_boundaries() {
        let spans = [
            engine::Span {
                start: 2,
                end: 5,
                tag: engine::ColorTag::Codec(engine::Codec::Url),
            },
            engine::Span {
                start: 5,
                end: 7,
                tag: engine::ColorTag::Hash,
            },
        ];
        let url = Some(engine::ColorTag::Codec(engine::Codec::Url));

        assert_eq!(tag_at(&spans, 0), None);
        assert_eq!(tag_at(&spans, 1), None);
        assert_eq!(tag_at(&spans, 2), url);
        assert_eq!(tag_at(&spans, 4), url);
        assert_eq!(tag_at(&spans, 5), Some(engine::ColorTag::Hash));
        assert_eq!(tag_at(&spans, 6), Some(engine::ColorTag::Hash));
        assert_eq!(tag_at(&spans, 7), None);
        assert_eq!(tag_at(&[], 0), None);
    }

    #[test]
    fn panel_control_width_never_drops_below_160_points() {
        egui::__run_test_ui(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(120.0, 40.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| assert_eq!(panel_control_width(ui), 160.0),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(300.0, 40.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| assert_eq!(panel_control_width(ui), 300.0),
            );
        });
    }
}
