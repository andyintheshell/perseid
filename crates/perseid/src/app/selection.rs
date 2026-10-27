//! History selection, clearing, and keyboard navigation helpers.

use super::*;

impl PerseidApp {
    pub(super) fn clear_history(&mut self) {
        self.history.clear();
        self.title_cache.borrow_mut().clear();
        self.selected = None;
        self.detail = None;
        self.inspector.clear_notes();
        self.search.clear_http_source();
        self.invalidate_views();
        self.project.clear_http_history();
    }

    pub(super) fn clear_websocket_history(&mut self) {
        self.websocket_history.clear();
        self.websocket_selected = None;
        self.websocket_detail = None;
        self.search.clear_websocket_source();
        self.invalidate_views();
        self.project.clear_ws_history();
    }

    pub(super) fn selected_row_index(&self) -> Option<usize> {
        self.selected
            .and_then(|id| self.visible.iter().position(|&visible| visible == id))
    }

    pub(super) fn selected_websocket_row_index(&self) -> Option<usize> {
        self.websocket_selected.and_then(|id| {
            self.websocket_visible
                .iter()
                .position(|&visible| visible == id)
        })
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        match moved_row_index(self.visible.len(), self.selected_row_index(), delta) {
            Some(next) => self.select_id(self.visible[next]),
            None => self.selected = None,
        }
    }

    pub(super) fn move_websocket_selection(&mut self, delta: isize) {
        match moved_row_index(
            self.websocket_visible.len(),
            self.selected_websocket_row_index(),
            delta,
        ) {
            Some(next) => self.select_websocket_id(self.websocket_visible[next]),
            None => self.websocket_selected = None,
        }
    }

    pub(super) fn select_id(&mut self, id: u64) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            self.detail = None;
        }
        self.scroll_to_selected = true;
    }

    pub(super) fn select_websocket_id(&mut self, id: u64) {
        if self.websocket_selected != Some(id) {
            self.websocket_selected = Some(id);
            self.websocket_detail = None;
        }
        self.websocket_scroll_to_selected = true;
    }
}

/// Row index after moving the selection by `delta` within `len` rows.
///
/// Without a selection the first row becomes selected, regardless of the
/// direction; otherwise the target index clamps to the ends and never wraps.
/// `None` means there are no rows.
pub(super) fn moved_row_index(len: usize, current: Option<usize>, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let next = match current {
        Some(position) => position as isize + delta,
        None => 0,
    };
    Some(next.clamp(0, len as isize - 1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_selection_starts_at_the_first_row_and_clamps() {
        assert_eq!(moved_row_index(0, None, 1), None);
        assert_eq!(moved_row_index(5, None, 1), Some(0));
        assert_eq!(moved_row_index(5, None, -1), Some(0));
        assert_eq!(moved_row_index(5, Some(2), 10), Some(4));
        assert_eq!(moved_row_index(5, Some(2), -10), Some(0));
        assert_eq!(moved_row_index(5, Some(0), -1), Some(0));
        assert_eq!(moved_row_index(5, Some(4), 1), Some(4));
    }
}
