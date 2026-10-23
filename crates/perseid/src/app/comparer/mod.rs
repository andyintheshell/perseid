//! Comparer: two item lists over one shared collection, plus a result window.
//!
//! The item tables deliberately show the same collection twice. Burp's
//! Comparer works this way: send an item once, then select any two rows—one in
//! each table—to compare them. Paste, Load, Remove, and Clear therefore all
//! operate on the shared list.
//!
//! The comparison itself lives in [`crate::comparer`]. This module builds the
//! item list, sends history messages here, and renders the result with paired
//! text or hex panes. Text and hex views are virtualized by line so a large
//! captured message does not have to be laid out as one galley.
//!
//! The implementation is split by responsibility: [`state`] owns the item
//! collection and comparison state, [`highlight`] maps diff spans onto display
//! text and bytes, and [`view`] draws the tab and result window.

mod highlight;
mod state;
mod view;

pub(super) use state::ComparerState;

use egui::Vec2;

use super::*;
use crate::comparer::{MAX_ITEM_BYTES, MAX_ITEMS};
use crate::view::{request_views, response_views};
use state::{ComparableItem, Feedback, ResultView};

impl PerseidApp {
    pub(super) fn comparer_view(&mut self, ui: &mut egui::Ui) {
        self.comparer.ui(ui);
    }

    pub(super) fn comparer_windows(&mut self, ctx: &egui::Context) {
        self.comparer.result_window(ctx);
        self.comparer.load_window(ctx);
    }

    /// Add the selected history message to the shared Comparer collection.
    /// Requests target table 1 and responses target table 2, so the common
    /// request/response comparison is a two-click operation.
    pub(super) fn send_selected_to_comparer(&mut self, pane: MessagePane) {
        let Some(item) = self
            .selected
            .and_then(|id| self.history.get(id))
            .filter(|item| !item.is_tunnel())
        else {
            return;
        };
        let target = match pane {
            MessagePane::Request => 0,
            MessagePane::Response => 1,
        };
        let raw = match pane {
            MessagePane::Request => request_views(item).raw,
            MessagePane::Response => {
                if item.response.is_none() {
                    return;
                }
                response_views(item).raw
            }
        };
        if let Err(error) = self.comparer.insert_item(raw.into_bytes(), target) {
            self.comparer.feedback = Some(Feedback {
                error: true,
                text: error,
            });
        }
    }

    pub(super) fn comparer_workspace(&self) -> Vec<Vec<u8>> {
        self.comparer
            .items
            .iter()
            .map(|item| item.data.clone())
            .collect()
    }

    pub(super) fn apply_comparer_workspace(&mut self, items: &[Vec<u8>]) {
        let state = &mut self.comparer;
        state.items = items
            .iter()
            .filter(|data| data.len() <= MAX_ITEM_BYTES)
            .take(MAX_ITEMS)
            .map(|data| ComparableItem::new(data.clone()))
            .collect();
        state.selected = [None, None];
        state.active = 0;
        state.feedback = None;
        state.load = None;
        state.paste = None;
        state.result = None;
        state.result_open = false;
        state.result_views = [ResultView::Text, ResultView::Text];
        state.sync_views = false;
        state.diff_cursor = 0;
        state.result_scroll = [Vec2::ZERO; 2];
        state.result_pane_rects = [None, None];
    }
}
