//! History filtering, visible-row caches, and the site-map revision cache.

use super::util::{matches_filter, websocket_matches_filter};
use super::*;
use crate::site_map::{self, SiteMap};

impl PerseidApp {
    pub(super) fn rebuild_visible(&mut self) {
        self.rebuild_http_visible();
        self.rebuild_websocket_visible();
    }

    pub(super) fn rebuild_http_visible(&mut self) {
        let key = HttpVisibleKey {
            quick_search: self.filter.clone(),
            filter: self.history_filter.clone(),
            scope_enabled: self.scope_enabled,
            scope: self.scope.clone(),
            revision: self.history.revision(),
        };
        if self.visible_key.as_ref() == Some(&key) {
            return;
        }
        self.visible.clear();

        let needle = self.filter.trim().to_lowercase();
        let scope_active = self.scope_enabled && !self.scope.is_empty();
        let search = self.history_filter.search();
        for item in self.history.iter() {
            if scope_active && !site_map::host_in_scope(&item.request.host, &self.scope) {
                continue;
            }
            if !needle.is_empty() && !matches_filter(item, &needle) {
                continue;
            }
            if !self.history_filter.matches(item, &self.scope, &search) {
                continue;
            }
            self.visible.push(item.id);
        }
        // Drop titles for exchanges that have been evicted or cleared.
        self.title_cache
            .borrow_mut()
            .retain(|id, _| self.history.get(*id).is_some());
        self.visible_key = Some(key);
    }

    pub(super) fn rebuild_websocket_visible(&mut self) {
        let key = (
            self.websocket_filter.clone(),
            self.websocket_history.revision(),
        );
        if self.websocket_visible_key.as_ref() == Some(&key) {
            return;
        }
        self.websocket_visible.clear();

        let needle = self.websocket_filter.trim().to_lowercase();
        let scope_active = self.scope_enabled && !self.scope.is_empty();
        for item in self.websocket_history.iter() {
            if scope_active && !site_map::host_in_scope(&item.connection.host, &self.scope) {
                continue;
            }
            if needle.is_empty() || websocket_matches_filter(item, &needle) {
                self.websocket_visible.push(item.id);
            }
        }
        self.websocket_visible_key = Some(key);
    }

    pub(super) fn invalidate_views(&mut self) {
        self.visible_key = None;
        self.websocket_visible_key = None;
        self.site_map = None;
    }

    pub(super) fn ensure_site_map(&mut self) {
        let revision = self.history.revision();
        if self
            .site_map
            .as_ref()
            .is_some_and(|(cached_revision, _)| *cached_revision == revision)
        {
            return;
        }
        // Pausing freezes the current snapshot. The cache is rebuilt on the
        // next frame after the user resumes or clears the pause state.
        if self.site_map_ui.paused() && self.site_map.is_some() {
            return;
        }
        let scope = self.scope_enabled.then_some(&self.scope);
        let map = SiteMap::build(&self.history, scope);
        self.site_map = Some((revision, map));
    }
}
