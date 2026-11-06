//! Pane-local literal search, shared by editable and read-only message views.

use super::*;

#[derive(Clone, Default)]
pub(in crate::app) struct MessageSearch {
    pub query: String,
    pub ranges: Arc<[Range<usize>]>,
    pub active: Option<usize>,
    fingerprint: Option<u64>,
    text_fingerprint: u64,
    pub jump: bool,
}

impl MessageSearch {
    pub fn refresh(&mut self, text: &str, fallback: Option<&[Range<usize>]>) {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hash);
        self.text_fingerprint = hash.finish();
        self.query.hash(&mut hash);
        if self.query.is_empty() {
            fallback.hash(&mut hash);
        }
        let fingerprint = hash.finish();
        if self.fingerprint == Some(fingerprint) {
            return;
        }
        self.fingerprint = Some(fingerprint);
        self.ranges = if self.query.is_empty() {
            fallback.unwrap_or_default().into()
        } else {
            // Escape metacharacters: this field searches literal displayed text,
            // case-insensitively, including Unicode, not a regular expression.
            regex::RegexBuilder::new(&regex::escape(&self.query))
                .case_insensitive(true)
                .build()
                .map(|pattern| {
                    pattern
                        .find_iter(text)
                        .map(|found| found.range())
                        .collect::<Vec<_>>()
                        .into()
                })
                .unwrap_or_default()
        };
        self.active = None;
        // Merely moving between captured messages should not change the scroll
        // position. Typing a query, however, immediately reveals its first hit.
        self.jump = false;
    }

    pub fn advance(&mut self, backwards: bool) {
        let count = self.ranges.len();
        if count == 0 {
            self.active = None;
            self.jump = false;
            return;
        }
        self.active = Some(match (self.active, backwards) {
            (None, false) => 0,
            (None, true) => count - 1,
            (Some(index), false) => (index + 1) % count,
            (Some(index), true) => (index + count - 1) % count,
        });
        self.jump = true;
    }

    pub fn active_range(&self) -> Option<&Range<usize>> {
        self.active.and_then(|index| self.ranges.get(index))
    }

    pub fn paint(&self, job: &mut egui::text::LayoutJob) {
        // TextEdit can lay out a newly edited buffer in this same frame. Old
        // byte offsets may split an inserted UTF-8 character; refresh first.
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        job.text.hash(&mut hash);
        if hash.finish() != self.text_fingerprint {
            let mut updated = self.clone();
            updated.refresh(&job.text, None);
            updated.paint(job);
            return;
        }
        highlight::paint_matches(job, &self.ranges, theme::SEARCH_MATCH);
        if let Some(active) = self.active_range() {
            highlight::paint_matches(job, std::slice::from_ref(active), theme::SEARCH_ACTIVE);
        }
    }

    pub fn reveal(&self, ui: &egui::Ui, galley: &egui::Galley, origin: egui::Pos2) {
        if self.jump
            && let Some(active) = self.active_range()
        {
            scroll_to_match(ui, galley, origin, active);
        }
    }
}

pub(in crate::app) fn searchable_view<R>(
    ui: &mut egui::Ui,
    pane: MessagePane,
    text: &mut dyn egui::TextBuffer,
    fallback: Option<&[Range<usize>]>,
    contents: impl FnOnce(&mut egui::Ui, &mut dyn egui::TextBuffer, &MessageSearch) -> R,
) -> R {
    let key = ui.make_persistent_id(("perseid.message.search", pane as u8));
    let mut search = ui
        .data(|data| data.get_temp::<MessageSearch>(key))
        .unwrap_or_default();
    let viewport = search_footer(ui, &mut search, text.as_str(), fallback);
    let result = ui
        .scope_builder(egui::UiBuilder::new().max_rect(viewport), |ui| {
            contents(ui, text, &search)
        })
        .inner;
    search.jump = false;
    ui.data_mut(|data| data.insert_temp(key, search));
    result
}

/// Allocate the footer first, leaving a bounded viewport above it. This keeps
/// search visible even for very long messages and horizontally scrolled text.
fn search_footer(
    ui: &mut egui::Ui,
    search: &mut MessageSearch,
    text: &str,
    fallback: Option<&[Range<usize>]>,
) -> egui::Rect {
    search.refresh(text, fallback);
    let rect = ui.available_rect_before_wrap();
    let height = ui.spacing().interact_size.y.max(24.0);
    let footer_top = (rect.bottom() - height).max(rect.top());
    let footer = egui::Rect::from_min_max(egui::pos2(rect.left(), footer_top), rect.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
        ui.painter().hline(
            rect.x_range(),
            footer_top - 3.0,
            Stroke::new(1.0, theme::BORDER),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let previous = ui
                .add_enabled(!search.ranges.is_empty(), egui::Button::new("←"))
                .on_hover_text("Previous highlight (Shift+Enter)")
                .clicked();
            let next = ui
                .add_enabled(!search.ranges.is_empty(), egui::Button::new("→"))
                .on_hover_text("Next highlight (Enter)")
                .clicked();
            let count = highlight_count(search.ranges.len());
            let count_width = ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(count, FontId::proportional(11.0), theme::TEXT_MUTED)
                    .size()
                    .x
            });
            let width = (ui.available_width() - count_width - 8.0).max(20.0);
            let response = ui
                .add_sized(
                    egui::vec2(width, height),
                    egui::TextEdit::singleline(&mut search.query)
                        .id(ui.make_persistent_id("search.query"))
                        .hint_text("Search")
                        .font(FontId::proportional(12.0)),
                )
                .on_hover_text("Find literal text (case-insensitive)");
            if response.changed() {
                search.refresh(text, fallback);
                if !search.query.is_empty() {
                    search.advance(false);
                }
            }
            let enter = (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if previous || next || enter {
                search.advance(previous || (enter && ui.input(|input| input.modifiers.shift)));
                if enter {
                    response.request_focus();
                }
            }
            ui.label(
                RichText::new(highlight_count(search.ranges.len()))
                    .size(11.0)
                    .color(theme::TEXT_MUTED),
            )
            .on_hover_text(search.active.map_or_else(
                || "No highlight selected".to_owned(),
                |index| format!("Highlight {} of {}", index + 1, search.ranges.len()),
            ));
        });
    });
    egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), (footer_top - 8.0).max(rect.top())),
    )
}

fn highlight_count(count: usize) -> String {
    format!(
        "{count} {}",
        if count == 1 {
            "highlight"
        } else {
            "highlights"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_count_handles_zero_and_singular() {
        assert_eq!(highlight_count(0), "0 highlights");
        assert_eq!(highlight_count(1), "1 highlight");
        assert_eq!(highlight_count(12), "12 highlights");
    }

    #[test]
    fn literal_unicode_search_and_navigation_wrap() {
        let mut search = MessageSearch {
            query: "é.".into(),
            ..Default::default()
        };
        search.refresh("É. é. ex", None);
        assert_eq!(&*search.ranges, &[0..3, 4..7]);
        search.advance(true);
        assert_eq!(search.active, Some(1));
        search.advance(false);
        assert_eq!(search.active, Some(0));
        search.advance(true);
        assert_eq!(search.active, Some(1));
        search.query = "missing".into();
        search.refresh("É. é. ex", None);
        search.advance(false);
        assert!(search.ranges.is_empty());
        assert_eq!(search.active_range(), None);
        assert!(!search.jump);
    }

    #[test]
    fn refresh_preserves_navigation_until_text_or_query_changes() {
        let mut search = MessageSearch {
            query: "a".into(),
            ..Default::default()
        };
        search.refresh("aaa", None);
        search.advance(false);
        search.advance(false);
        let ranges = search.ranges.clone();
        search.refresh("aaa", None);
        assert!(Arc::ptr_eq(&ranges, &search.ranges));
        assert_eq!(search.active, Some(1));
        search.refresh("a", None);
        assert_eq!(&*search.ranges, std::slice::from_ref(&(0..1)));
        assert_eq!(search.active, None);
        search.query.clear();
        search.refresh("a", None);
        assert!(search.ranges.is_empty());
    }

    #[test]
    fn empty_local_query_keeps_global_search_highlights_navigable() {
        let mut search = MessageSearch::default();
        search.refresh("abc abc", Some(&[0..3, 4..7]));
        search.advance(false);
        assert_eq!(search.active_range(), Some(&(0..3)));
        search.query = "bc".into();
        search.refresh("abc abc", Some(&[0..3, 4..7]));
        assert_eq!(&*search.ranges, &[1..3, 5..7]);
        search.query.clear();
        search.refresh("abc abc", Some(std::slice::from_ref(&(4..7))));
        assert_eq!(&*search.ranges, std::slice::from_ref(&(4..7)));
    }

    #[test]
    fn painting_after_an_edit_uses_fresh_utf8_boundaries() {
        let mut search = MessageSearch {
            query: "a".into(),
            ..Default::default()
        };
        search.refresh("aa", None);
        let mut job = highlight::message_job("😀a", 300.0, &message_font());
        search.paint(&mut job);
        assert_eq!(job.text, "😀a");
        for section in &job.sections {
            assert!(job.text.is_char_boundary(section.byte_range.start.0));
            assert!(job.text.is_char_boundary(section.byte_range.end.0));
        }
        assert!(
            job.sections
                .iter()
                .any(|section| section.format.background == theme::SEARCH_MATCH)
        );
    }
}
