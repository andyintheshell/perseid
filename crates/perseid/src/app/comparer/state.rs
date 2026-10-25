//! Comparer state: the shared item collection, selections, and comparisons.

use std::path::PathBuf;

use egui::{Rect, Vec2};

use super::highlight::{CompareResult, visible_line};
use crate::comparer as engine;
use crate::comparer::{CompareMode, MAX_ITEM_BYTES, MAX_ITEMS};
use crate::view;

const PREVIEW_BYTES: usize = 220;

/// One entry in the shared item collection.
pub(super) struct ComparableItem {
    pub(super) data: Vec<u8>,
}

impl ComparableItem {
    pub(super) fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub(super) fn preview(&self) -> String {
        let mut out = String::with_capacity(PREVIEW_BYTES + 1);
        for &byte in self.data.iter().take(PREVIEW_BYTES) {
            let character = match byte {
                b'\r' | b'\n' | b'\t' => ' ',
                0x20..=0x7e => char::from(byte),
                _ => '·',
            };
            out.push(character);
        }
        if self.data.len() > PREVIEW_BYTES {
            out.push('…');
        }
        out
    }
}

pub(super) struct Feedback {
    pub(super) error: bool,
    pub(super) text: String,
}

pub(super) struct LoadDialog {
    pub(super) target: usize,
    pub(super) path: String,
    pub(super) error: Option<String>,
}

pub(super) struct PasteState {
    pub(super) target: usize,
    pub(super) frames_left: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResultView {
    Text,
    Hex,
}

/// State for the Comparer tab and its result window.
pub(in crate::app) struct ComparerState {
    pub(super) items: Vec<ComparableItem>,
    pub(super) selected: [Option<usize>; 2],
    pub(super) active: usize,
    pub(super) feedback: Option<Feedback>,
    pub(super) load: Option<LoadDialog>,
    pub(super) paste: Option<PasteState>,
    pub(super) result: Option<CompareResult>,
    pub(super) result_open: bool,
    pub(super) result_views: [ResultView; 2],
    pub(super) sync_views: bool,
    pub(super) diff_cursor: usize,
    pub(super) result_scroll: [Vec2; 2],
    pub(super) result_pane_rects: [Option<Rect>; 2],
    pub(super) result_row_height: f32,
}

impl ComparerState {
    pub(in crate::app) fn new() -> Self {
        Self {
            items: Vec::new(),
            selected: [None, None],
            active: 0,
            feedback: None,
            load: None,
            paste: None,
            result: None,
            result_open: false,
            result_views: [ResultView::Text, ResultView::Text],
            sync_views: false,
            diff_cursor: 0,
            result_scroll: [Vec2::ZERO, Vec2::ZERO],
            result_pane_rects: [None, None],
            result_row_height: 14.0,
        }
    }

    pub(super) fn insert_item(&mut self, data: Vec<u8>, target: usize) -> Result<usize, String> {
        if data.len() > MAX_ITEM_BYTES {
            return Err(format!(
                "item is {}; the Comparer accepts items up to {}",
                view::human_bytes(data.len() as u64),
                view::human_bytes(MAX_ITEM_BYTES as u64)
            ));
        }
        if self.items.len() >= MAX_ITEMS {
            return Err(format!(
                "the Comparer holds at most {MAX_ITEMS} items; remove one before adding another"
            ));
        }
        self.items.push(ComparableItem::new(data));
        let index = self.items.len() - 1;
        self.selected[target] = Some(index);
        self.active = target;
        self.feedback = None;
        Ok(index)
    }

    pub(super) fn remove_active(&mut self) {
        let Some(index) = self.selected[self.active] else {
            return;
        };
        if index >= self.items.len() {
            self.selected = [None, None];
            return;
        }
        self.items.remove(index);
        for selected in &mut self.selected {
            *selected = match *selected {
                Some(value) if value == index => None,
                Some(value) if value > index => Some(value - 1),
                other => other,
            };
        }
    }

    pub(super) fn clear_items(&mut self) {
        self.items.clear();
        self.selected = [None, None];
        self.result = None;
        self.result_open = false;
        self.feedback = None;
    }

    pub(super) fn compare(&mut self, mode: CompareMode) {
        let (Some(old_index), Some(new_index)) = (self.selected[0], self.selected[1]) else {
            self.feedback = Some(Feedback {
                error: true,
                text: "select one item in each table before comparing".to_string(),
            });
            return;
        };
        let (Some(old), Some(new)) = (self.items.get(old_index), self.items.get(new_index)) else {
            self.selected = [None, None];
            return;
        };
        match engine::compare(mode, &old.data, &new.data) {
            Ok(comparison) => {
                let result = CompareResult::new(
                    mode,
                    old_index,
                    new_index,
                    old.data.clone(),
                    new.data.clone(),
                    comparison,
                );
                self.result = Some(result);
                self.result_open = true;
                self.diff_cursor = 0;
                self.result_views = [ResultView::Text, ResultView::Text];
                self.result_scroll = [Vec2::ZERO, Vec2::ZERO];
                self.result_pane_rects = [None, None];
                self.feedback = None;
            }
            Err(error) => {
                self.feedback = Some(Feedback {
                    error: true,
                    text: error.to_string(),
                });
            }
        }
    }

    pub(super) fn navigate_result(
        &mut self,
        result: &CompareResult,
        direction: i32,
        row_height: f32,
    ) {
        if result.spans().is_empty() {
            return;
        }
        let length = result.spans().len();
        if direction > 0 {
            self.diff_cursor = (self.diff_cursor + 1) % length;
        } else {
            self.diff_cursor = (self.diff_cursor + length - 1) % length;
        }
        let span = &result.spans()[self.diff_cursor];
        self.result_scroll[0].y =
            visible_line(result.display(0), &span.old, self.result_views[0]) as f32 * row_height;
        self.result_scroll[1].y =
            visible_line(result.display(1), &span.new, self.result_views[1]) as f32 * row_height;
        self.result_scroll[0].y = (self.result_scroll[0].y - row_height * 2.0).max(0.0);
        self.result_scroll[1].y = (self.result_scroll[1].y - row_height * 2.0).max(0.0);
    }
}

pub(super) fn read_comparer_file(path: &str) -> Result<Vec<u8>, String> {
    if path.is_empty() {
        return Err("enter a file path".to_string());
    }
    let path = PathBuf::from(path);
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if metadata.len() > MAX_ITEM_BYTES as u64 {
        return Err(format!(
            "file is {}; the Comparer accepts items up to {}",
            view::human_bytes(metadata.len()),
            view::human_bytes(MAX_ITEM_BYTES as u64)
        ));
    }
    std::fs::read(&path).map_err(|error| format!("could not read {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comparison_result(mode: CompareMode, old: &[u8], new: &[u8]) -> CompareResult {
        let comparison = engine::compare(mode, old, new).unwrap();
        CompareResult::new(mode, 0, 1, old.to_vec(), new.to_vec(), comparison)
    }

    #[test]
    fn removing_an_item_adjusts_both_selections() {
        let mut state = ComparerState::new();
        state.insert_item(b"one".to_vec(), 0).unwrap();
        state.insert_item(b"two".to_vec(), 1).unwrap();
        state.insert_item(b"three".to_vec(), 0).unwrap();
        assert_eq!(state.selected, [Some(2), Some(1)]);
        state.active = 1;
        state.remove_active();
        assert_eq!(state.selected, [Some(1), None]);
        state.active = 0;
        state.remove_active();
        assert_eq!(state.selected, [None, None]);
        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn clearing_items_closes_the_result() {
        let mut state = ComparerState::new();
        state.insert_item(b"one".to_vec(), 0).unwrap();
        state.insert_item(b"two".to_vec(), 1).unwrap();
        state.compare(CompareMode::Words);
        assert!(state.result_open);
        state.clear_items();
        assert!(!state.result_open);
        assert!(state.result.is_none());
    }

    #[test]
    fn previews_stay_single_line_and_bounded() {
        let item = ComparableItem::new(b"a\r\nb\x00c".to_vec());
        assert_eq!(item.preview(), "a  b·c");
    }

    #[test]
    fn insert_item_selects_the_new_row_and_clears_feedback() {
        let mut state = ComparerState::new();
        state.feedback = Some(Feedback {
            error: true,
            text: "old".to_string(),
        });
        assert_eq!(state.insert_item(b"abc".to_vec(), 1), Ok(0));
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].data, b"abc");
        assert_eq!(state.selected, [None, Some(0)]);
        assert_eq!(state.active, 1);
        assert!(state.feedback.is_none());
    }

    #[test]
    fn insert_item_rejects_oversized_items_and_a_full_collection() {
        let mut state = ComparerState::new();
        let error = state
            .insert_item(vec![0; MAX_ITEM_BYTES + 1], 0)
            .unwrap_err();
        assert!(error.contains("accepts items up to"));
        assert!(state.items.is_empty());

        state.items = (0..MAX_ITEMS)
            .map(|_| ComparableItem::new(Vec::new()))
            .collect();
        let error = state.insert_item(b"one more".to_vec(), 0).unwrap_err();
        assert!(error.contains(&format!("at most {MAX_ITEMS} items")));
        assert_eq!(state.items.len(), MAX_ITEMS);
    }

    #[test]
    fn read_comparer_file_reads_bytes_and_reports_missing_files() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "perseid-comparer-file-test-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"hello\x00world").unwrap();
        let read = read_comparer_file(path.to_str().unwrap());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(read.unwrap(), b"hello\x00world");

        assert_eq!(read_comparer_file("").unwrap_err(), "enter a file path");
        let error = read_comparer_file(path.to_str().unwrap()).unwrap_err();
        assert!(error.contains("could not read"));
    }

    #[test]
    fn navigate_result_wraps_and_scrolls_to_the_changed_line() {
        let old = b"same\nsame\nsame\nOLD one\nsame\nOLD two\nsame";
        let new = b"same\nsame\nsame\nNEW one\nsame\nNEW two\nsame";
        let result = comparison_result(CompareMode::Words, old, new);
        assert_eq!(result.spans().len(), 2);

        let mut state = ComparerState::new();
        state.navigate_result(&result, 1, 10.0);
        assert_eq!(state.diff_cursor, 1);
        assert_eq!(state.result_scroll[0].y, 30.0);
        assert_eq!(state.result_scroll[1].y, 30.0);

        state.navigate_result(&result, 1, 10.0);
        assert_eq!(state.diff_cursor, 0);
        assert_eq!(state.result_scroll[0].y, 10.0);

        state.navigate_result(&result, 1, 10.0);
        assert_eq!(state.diff_cursor, 1);

        state.navigate_result(&result, -1, 10.0);
        assert_eq!(state.diff_cursor, 0);

        // Spans on the first line clamp the scroll offset at zero.
        state.diff_cursor = 0;
        state.result_scroll[0] = Vec2::ZERO;
        state.result_scroll[1] = Vec2::ZERO;
        let first_line = comparison_result(CompareMode::Words, b"OLD rest", b"NEW rest");
        state.navigate_result(&first_line, 1, 10.0);
        assert_eq!(state.result_scroll[0].y, 0.0);
        assert_eq!(state.result_scroll[1].y, 0.0);
    }
}
