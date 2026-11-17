//! Disk-backed derived endpoint index and bounded Target presentation.
//!
//! The project remains authoritative. The disposable index stores summaries,
//! never bodies; construction streams one exchange at a time off the UI thread.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Instant;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use super::*;
use crate::project::{ProjectError, ProjectSnapshot};
use crate::site_map::SiteHost;

#[path = "target_viewport.rs"]
mod viewport;

const PAGE: usize = 500;
const ENTRIES: TableDefinition<&str, &[u8]> = TableDefinition::new("endpoints");
const HOST_SCHEMES: TableDefinition<&str, &[u8]> = TableDefinition::new("host_schemes");
type Result<T> = std::result::Result<T, ProjectError>;
type Reply<T> = mpsc::Receiver<Result<T>>;

struct Index {
    // Drop the database before removing its private temporary directory.
    db: Option<Database>,
    directory: PathBuf,
}

impl Drop for Index {
    fn drop(&mut self) {
        self.db.take();
        let _ = std::fs::remove_file(self.directory.join("index.redb"));
        let _ = std::fs::remove_dir(&self.directory);
    }
}

impl Index {
    fn empty() -> Result<Self> {
        let directory =
            std::env::temp_dir().join(format!("perseid-site-map-{}", uuid::Uuid::new_v4()));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&directory)?;
        let mut index = Self {
            db: None,
            directory,
        };
        let mut builder = Database::builder();
        builder.set_cache_size(8 * 1024 * 1024);
        index.db = Some(builder.create(index.directory.join("index.redb"))?);
        Ok(index)
    }

    fn build(snapshot: &ProjectSnapshot, cancelled: &AtomicBool) -> Result<Self> {
        let index = Self::empty()?;
        let db = index.db.as_ref().unwrap();
        let mut write = db.begin_write()?;
        write.set_durability(redb::Durability::None)?;
        write.open_table(ENTRIES)?;
        write.open_table(HOST_SCHEMES)?;
        let mut tx = Some(write);
        let mut error = None;
        let mut count = 0;
        snapshot.scan_http(|item, _, _| {
            if cancelled.load(Ordering::Relaxed) {
                return false;
            }
            let result = (|| -> Result<()> {
                if !item.is_tunnel() {
                    let mut entry = SiteEntry::from_item(&item);
                    let mut schemes_table = tx.as_ref().unwrap().open_table(HOST_SCHEMES)?;
                    let mut schemes = schemes_table
                        .get(entry.host.as_str())?
                        .map(|v| serde_json::from_slice::<BTreeSet<String>>(v.value()))
                        .transpose()?
                        .unwrap_or_default();
                    schemes.insert(entry.scheme.clone());
                    let bytes = serde_json::to_vec(&schemes)?;
                    schemes_table.insert(entry.host.as_str(), bytes.as_slice())?;
                    let key = serde_json::to_string(&(&entry.host, &entry.path, &entry.method))?;
                    let mut table = tx.as_ref().unwrap().open_table(ENTRIES)?;
                    let old = table
                        .get(key.as_str())?
                        .map(|v| serde_json::from_slice::<SiteEntry>(v.value()))
                        .transpose()?;
                    if let Some(old) = old {
                        entry.count += old.count;
                        // Latest means greatest capture ID, not wall clock time.
                        if old.id > entry.id {
                            let count = entry.count;
                            entry = old;
                            entry.count = count;
                        }
                    }
                    let bytes = serde_json::to_vec(&entry)?;
                    table.insert(key.as_str(), bytes.as_slice())?;
                }
                count += 1;
                // Bound dirty pages as well as the decoded-record working set.
                if count % 256 == 0 {
                    tx.take().unwrap().commit()?;
                    let mut write = db.begin_write()?;
                    write.set_durability(redb::Durability::None)?;
                    tx = Some(write);
                }
                Ok(())
            })();
            if let Err(err) = result {
                error = Some(err);
                return false;
            }
            true
        })?;
        if let Some(err) = error {
            return Err(err);
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(index)
    }

    #[cfg(test)]
    // Retained as an independent bounded reference oracle for view ordering.
    fn query(&self, query: &Query, cancelled: &AtomicBool) -> Result<Page> {
        let tx = self.db.as_ref().unwrap().begin_read()?;
        let table = tx.open_table(ENTRIES)?;
        let mut entries: Vec<SiteEntry> = Vec::with_capacity(PAGE + 1);
        let mut children: BTreeMap<String, SiteTreeNode> = BTreeMap::new();
        let mut total = 0;
        for value in table.iter()? {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let (_, value) = value?;
            let entry: SiteEntry = serde_json::from_slice(value.value())?;
            if query
                .scope
                .as_ref()
                .is_some_and(|s| !site_map::host_in_scope(&entry.host, s))
            {
                continue;
            }
            let allowed = query.filter.allows(&entry);
            if let Some(mut node) = child_node(query.node.as_ref(), &entry)
                && (allowed || (!query.filter.hide_empty_folders && node.is_directory()))
            {
                let key = node.label.clone();
                if query.tree_after.as_ref().is_none_or(|after| key > *after) {
                    node.count = usize::from(allowed);
                    if let Some(existing) = children.get_mut(&key) {
                        existing.count += node.count;
                        if node.is_directory() {
                            existing.id = node.id;
                        }
                    } else {
                        children.insert(key, node);
                    }
                    if children.len() > PAGE + 1 {
                        children.pop_last();
                    }
                }
            }
            if !allowed || query.node.as_ref().is_none_or(|n| !n.contains(&entry)) {
                continue;
            }
            let row = query.row(&entry);
            if !query.search.is_empty() && !site_row_matches(&row, &query.search) {
                continue;
            }
            total += 1;
            if let Some(cursor) = &query.cursor {
                let order = query.compare(&entry, cursor);
                if (!query.backwards && !order.is_gt()) || (query.backwards && !order.is_lt()) {
                    continue;
                }
            }
            let position = entries
                .binary_search_by(|other| {
                    let order = query.compare(other, &entry);
                    if query.backwards {
                        order.reverse()
                    } else {
                        order
                    }
                })
                .unwrap_or_else(|i| i);
            if position <= PAGE {
                entries.insert(position, entry);
                entries.truncate(PAGE + 1);
            }
        }
        let more = entries.len() > PAGE;
        entries.truncate(PAGE);
        if query.backwards {
            entries.reverse();
        }
        let tree_more = children.len() > PAGE;
        if tree_more {
            children.pop_last();
        }
        let schemes_table = tx.open_table(HOST_SCHEMES)?;
        for node in children.values_mut() {
            let schemes = schemes_table
                .get(node.id.host())?
                .map(|v| serde_json::from_slice::<BTreeSet<String>>(v.value()))
                .transpose()?
                .unwrap_or_default();
            node.scheme = if schemes.len() == 1 {
                schemes.first().unwrap().clone()
            } else {
                "mixed".into()
            };
        }
        Ok(Page {
            entries,
            children: children.into_values().collect(),
            more,
            tree_more,
            total,
        })
    }
}

#[cfg(test)]
fn child_node(parent: Option<&SiteNodeId>, entry: &SiteEntry) -> Option<SiteTreeNode> {
    let (id, label) = match parent {
        None => (
            SiteNodeId::Host {
                host: entry.host.clone(),
            },
            entry.host.clone(),
        ),
        Some(parent) if parent.contains(entry) => {
            let base = entry.base_path();
            let prefix = match parent {
                SiteNodeId::Host { .. } => "/",
                SiteNodeId::Directory { path, .. } => path.as_str(),
                SiteNodeId::Endpoint { .. } => return None,
            };
            let suffix = base.strip_prefix(prefix)?.trim_start_matches('/');
            if suffix.is_empty() {
                return None;
            }
            let label = suffix.split('/').next()?.to_string();
            let path = format!("{}/{}", prefix.trim_end_matches('/'), label);
            let id = if suffix.contains('/') {
                SiteNodeId::Directory {
                    host: entry.host.clone(),
                    path,
                }
            } else {
                SiteNodeId::Endpoint {
                    host: entry.host.clone(),
                    path,
                }
            };
            (id, label)
        }
        _ => return None,
    };
    Some(SiteTreeNode {
        id,
        label,
        scheme: String::new(),
        methods: vec![],
        count: 0,
        children: vec![],
    })
}

#[derive(Clone, PartialEq)]
struct Query {
    node: Option<SiteNodeId>,
    filter: SiteMapFilter,
    scope: Option<BTreeSet<String>>,
    search: String,
    sort: SiteMapSort,
    ascending: bool,
    notes: BTreeMap<u64, String>,
    #[cfg(test)]
    cursor: Option<SiteEntry>,
    #[cfg(test)]
    backwards: bool,
    #[cfg(test)]
    tree_after: Option<String>,
}

impl Query {
    fn current(app: &PerseidApp) -> Self {
        Self {
            node: app.site_map_ui.selected_node.clone(),
            filter: app.site_map_ui.filter,
            scope: app.scope_enabled.then(|| app.scope.clone()),
            search: app.site_map_ui.search.trim().to_lowercase(),
            sort: app.site_map_ui.sort,
            ascending: app.site_map_ui.sort_ascending,
            notes: app
                .inspector
                .workspace()
                .notes
                .into_iter()
                .map(|n| (n.id, n.text))
                .collect(),
            #[cfg(test)]
            cursor: None,
            #[cfg(test)]
            backwards: false,
            #[cfg(test)]
            tree_after: None,
        }
    }

    fn row<'a>(&'a self, entry: &'a SiteEntry) -> SiteRowRef<'a> {
        SiteRowRef {
            entry,
            item: None,
            notes: self
                .notes
                .get(&entry.id)
                .map(String::as_str)
                .unwrap_or_default(),
        }
    }
    #[cfg(test)]
    fn compare(&self, left: &SiteEntry, right: &SiteEntry) -> std::cmp::Ordering {
        let order = compare_site_rows(&self.row(left), &self.row(right), self.sort);
        let order = if self.ascending {
            order
        } else {
            order.reverse()
        };
        order
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.host.cmp(&right.host))
            .then_with(|| left.method.cmp(&right.method))
    }
}

#[cfg(test)]
#[derive(Default)]
struct Page {
    entries: Vec<SiteEntry>,
    children: Vec<SiteTreeNode>,
    more: bool,
    tree_more: bool,
    total: usize,
}

#[derive(Default)]
pub(in crate::app) struct StoredMap {
    generation: u64,
    revision: (u64, u64, u64),
    index: Option<Arc<Index>>,
    build: Option<Reply<Index>>,
    query: Option<Reply<viewport::View>>,
    view: Option<Arc<viewport::View>>,
    window_query: Option<Reply<viewport::Window>>,
    window: viewport::Window,
    wanted_row: usize,
    wanted_tree: usize,
    pub(super) selected_row: Option<usize>,
    pub(super) pending_row: Option<usize>,
    open: BTreeMap<String, bool>,
    requested: Option<Query>,
    detail_query: Option<(u64, Reply<Option<HistoryItem>>)>,
    detail_selected: Option<u64>,
    pub(in crate::app) detail: Option<HistoryItem>,
    error: Option<String>,
    cancelled: Arc<AtomicBool>,
    query_cancelled: Arc<AtomicBool>,
    refreshed: Option<Instant>,
}

impl Drop for StoredMap {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.query_cancelled.store(true, Ordering::Relaxed);
    }
}

impl StoredMap {
    pub(super) fn total(&self) -> usize {
        self.view.as_ref().map_or(0, |v| v.total)
    }
    pub(super) fn loading_view(&self) -> bool {
        self.view.is_none() || self.query.is_some()
    }
    pub(super) fn offset(&self) -> usize {
        self.window.row
    }
    pub(super) fn want(&mut self, row: usize) {
        self.wanted_row = row.saturating_sub(PAGE / 4);
    }
    pub(super) fn navigate(&mut self, row: usize) {
        self.pending_row = Some(row.min(self.total().saturating_sub(1)));
        self.want(row);
    }
    fn poll_window(&mut self) {
        // The visible tree remains on screen while its replacement is built.
        // Do not read windows from the previous Contents query in that interval.
        if self.query.is_some() {
            return;
        }
        if let Some(result) = receive(&mut self.window_query) {
            match result {
                Ok(window) => self.window = window,
                Err(err) => self.error = Some(err.to_string()),
            }
        }
        let Some(view) = self.view.clone() else {
            return;
        };
        let row = self.wanted_row.min(view.total.saturating_sub(1));
        let tree = self.wanted_tree.min(view.tree_total.saturating_sub(1));
        let row_missing = view.total > 0
            && (row < self.window.row
                || row + PAGE / 2 >= self.window.row + self.window.entries.len())
            && (self.window.row != row || self.window.entries.is_empty());
        let tree_missing = view.tree_total > 0
            && (tree < self.window.tree
                || tree + PAGE / 2 >= self.window.tree + self.window.nodes.len())
            && (self.window.tree != tree || self.window.nodes.is_empty());
        if self.window_query.is_none() && (row_missing || tree_missing) {
            let (tx, rx) = mpsc::channel();
            self.window_query = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(view.window(row, tree));
            });
        }
    }
    pub(super) fn active(&self) -> bool {
        self.generation != 0
    }
    pub(in crate::app) fn item(&self, id: u64) -> Option<&HistoryItem> {
        self.detail.as_ref().filter(|item| item.id == id)
    }
    pub(in crate::app) fn loading(&self, id: u64) -> bool {
        self.detail_query
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
    }

    fn reset(&mut self, generation: u64) {
        *self = Self::default();
        self.generation = generation;
    }

    fn select(&mut self, snapshot: ProjectSnapshot, id: u64) {
        if self.detail_selected == Some(id) {
            return;
        }
        self.detail_selected = Some(id);
        self.detail = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.detail_query = Some((id, rx));
        std::thread::spawn(move || {
            let _ = tx.send(snapshot.get_http(id));
        });
    }
}

fn receive<T>(slot: &mut Option<Reply<T>>) -> Option<Result<T>> {
    match slot.as_ref()?.try_recv() {
        Ok(result) => {
            *slot = None;
            Some(result)
        }
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => {
            *slot = None;
            Some(Err(ProjectError::ServiceUnavailable))
        }
    }
}

impl PerseidApp {
    pub(in crate::app) fn stored_site_map_view(&mut self, ui: &mut egui::Ui) {
        let Some(status) = self.project.status() else {
            return;
        };
        let Some(snapshot) = self.project.handle().and_then(|h| h.database_snapshot()) else {
            return;
        };
        self.site_map_toolbar(ui);
        let mut stored = std::mem::take(&mut self.site_map_ui.stored);
        if stored.generation != status.generation {
            stored.reset(status.generation);
            self.site_map_ui.selected_node = None;
            self.selected = None;
            self.detail = None;
        }
        let revision = (
            status.last_committed_sequence,
            status.retention_evictions,
            status.http_count,
        );
        let evicted = stored.revision.1 != revision.1 || status.http_count < stored.revision.2;
        if evicted {
            stored.reset(status.generation);
            self.selected = None;
            self.detail = None;
        }
        if stored.build.is_none()
            && (stored.index.is_none()
                || (!self.site_map_ui.paused
                    && stored.revision != revision
                    && stored
                        .refreshed
                        .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))))
        {
            if stored.error.is_some() {
                self.site_map_ui.stored = stored;
                ui.colored_label(
                    theme::RED,
                    self.site_map_ui.stored.error.as_deref().unwrap(),
                );
                if ui.button("Retry site map").clicked() {
                    self.site_map_ui.stored.error = None;
                }
                return;
            }
            stored.revision = revision;
            stored.refreshed = Some(Instant::now());
            let (tx, rx) = mpsc::channel();
            stored.build = Some(rx);
            let snapshot = snapshot.clone();
            let cancelled = stored.cancelled.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Index::build(&snapshot, &cancelled));
            });
        }
        if let Some(result) = (!self.site_map_ui.paused || stored.index.is_none())
            .then(|| receive(&mut stored.build))
            .flatten()
        {
            match result {
                Ok(index) => {
                    stored.index = Some(Arc::new(index));
                    stored.requested = None;
                    stored.query_cancelled.store(true, Ordering::Relaxed);
                    stored.query = None;
                    stored.detail = None;
                    stored.detail_query = None;
                    stored.detail_selected = None;
                    self.detail = None;
                }
                Err(err) => {
                    stored.error = Some(err.to_string());
                    stored.refreshed = Some(Instant::now());
                }
            }
        }
        let current = Query::current(self);
        if let Some(result) = receive(&mut stored.query)
            && stored.requested.as_ref() == Some(&current)
            && stored.open == self.site_map_ui.open
        {
            match result {
                Ok(view) => {
                    if view.total == 0 {
                        stored.pending_row = None;
                    }
                    if let Some((id, ordinal)) = view.selected
                        && self.selected == Some(id)
                    {
                        stored.selected_row = Some(ordinal);
                        if stored.pending_row.is_some() {
                            stored.pending_row = Some(ordinal);
                            stored.want(ordinal);
                        }
                    }
                    let view = Arc::new(view);
                    let row = stored.wanted_row.min(view.total.saturating_sub(1));
                    let tree = stored.wanted_tree.min(view.tree_total.saturating_sub(1));
                    stored.view = Some(view.clone());
                    stored.window.entries.clear();
                    let (tx, rx) = mpsc::channel();
                    stored.window_query = Some(rx);
                    std::thread::spawn(move || {
                        let _ = tx.send(view.window(row, tree));
                    });
                }
                Err(err) => stored.error = Some(err.to_string()),
            }
        }
        if let Some((id, rx)) = &stored.detail_query {
            match rx.try_recv() {
                Ok(Ok(item)) => {
                    if self.selected == Some(*id) {
                        stored.detail = item;
                        self.detail = None;
                        if stored.detail.is_none() {
                            stored.error =
                                Some("This exchange was cleared or evicted by retention.".into());
                        }
                    }
                    stored.detail_query = None;
                }
                Ok(Err(err)) => {
                    stored.error = Some(err.to_string());
                    stored.detail_query = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    stored.error = Some("Detail reader stopped. Select the exchange again.".into());
                    stored.detail_query = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        stored.poll_window();
        let mut node_changed = false;
        if self.site_map_ui.selected_node.is_none()
            && let Some(node) = stored.window.nodes.first()
        {
            self.site_map_ui.selected_node = Some(node.id.clone());
            node_changed = true;
        }
        egui::Panel::left(TARGET_TREE_ID)
            .resizable(true)
            .default_size(340.0)
            .min_size(210.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| {
                let total = stored.view.as_ref().map_or(0, |v| v.tree_total);
                egui::ScrollArea::both()
                    .id_salt("stored-url-tree")
                    .auto_shrink([false, false])
                    .show_rows(ui, 21.0, total, |ui, range| {
                        stored.wanted_tree = range.start.saturating_sub(PAGE / 4);
                        for ordinal in range {
                            let Some(node) = ordinal
                                .checked_sub(stored.window.tree)
                                .and_then(|i| stored.window.nodes.get(i))
                            else {
                                ui.allocate_space(egui::vec2(ui.available_width(), 21.0));
                                continue;
                            };
                            let expanded = self
                                .site_map_ui
                                .open
                                .get(&node.id.key())
                                .copied()
                                .unwrap_or(node.depth <= 1);
                            let (picked, toggled) = stored_tree_row(
                                ui,
                                node,
                                expanded,
                                self.site_map_ui.selected_node.as_ref() == Some(&node.id),
                            );
                            if toggled {
                                self.site_map_ui.open.insert(node.id.key(), !expanded);
                            }
                            if picked {
                                self.site_map_ui.selected_node = Some(node.id.clone());
                                node_changed = true;
                            }
                        }
                    });
            });
        if node_changed {
            stored.wanted_row = 0;
            stored.selected_row = None;
            stored.pending_row = Some(0);
            self.selected = None;
            self.detail = None;
        }

        // Reuse the existing Contents renderer and shared details with only one
        // page of summaries and one hydrated exchange resident in the UI.
        let mut hosts: BTreeMap<String, Vec<SiteEntry>> = BTreeMap::new();
        for entry in &stored.window.entries {
            hosts
                .entry(entry.host.clone())
                .or_default()
                .push(entry.clone());
        }
        let map = SiteMap {
            hosts: hosts
                .into_iter()
                .map(|(host, entries)| SiteHost {
                    host,
                    scheme: String::new(),
                    entries,
                })
                .collect(),
        };
        let total = stored.view.as_ref().map_or(0, |v| v.total);
        let busy =
            stored.build.is_some() || stored.query.is_some() || stored.window_query.is_some();
        let error = stored.error.clone();
        self.site_map_ui.stored = stored;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::SURFACE))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(20.0);
                    ui.weak(format!("{total} matching endpoints"));
                    if busy {
                        ui.spinner();
                    }
                });
                if let Some(error) = error {
                    ui.colored_label(theme::RED, error);
                    if ui.button("Retry site map").clicked() {
                        self.site_map_ui.stored = StoredMap::default();
                    }
                }
                {
                    egui::Panel::bottom(SITE_MAP_DETAIL_ID)
                        .resizable(true)
                        .default_size((ui.available_height() * 0.46).max(220.0))
                        .min_size(190.0)
                        .show(ui, |ui| {
                            ui.take_available_space();
                            self.detail_panel(ui);
                        });
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::SURFACE))
                    .show(ui, |ui| {
                        if let Some(action) = self.site_map_contents(ui, &map, &[], node_changed) {
                            self.apply_site_map_row_action(action);
                        }
                    });
            });
        let query = Query::current(self);
        let stored = &mut self.site_map_ui.stored;
        let controls_changed = stored.requested.as_ref().is_some_and(|old| {
            old.node != query.node
                || old.filter != query.filter
                || old.scope != query.scope
                || old.search != query.search
                || old.sort != query.sort
                || old.ascending != query.ascending
                || old.notes != query.notes
        });
        if controls_changed {
            if stored
                .requested
                .as_ref()
                .is_some_and(|old| old.filter != query.filter || old.scope != query.scope)
            {
                stored.wanted_tree = 0;
            }
            self.site_map_ui.visible_ids.clear();
            stored.window.entries.clear();
            stored.wanted_row = 0;
            stored.selected_row = None;
            stored.pending_row = Some(0);
            stored.window_query = None;
        }
        let open_changed = stored.open != self.site_map_ui.open;
        if (stored.requested.as_ref() != Some(&query) || open_changed) && stored.query.is_some() {
            stored.query_cancelled.store(true, Ordering::Relaxed);
            stored.query = None;
        }
        if (stored.requested.as_ref() != Some(&query) || open_changed)
            && stored.query.is_none()
            && let Some(index) = stored.index.clone()
        {
            let expansion_view = if stored.requested.as_ref() == Some(&query) {
                stored.view.clone()
            } else {
                None
            };
            let (tx, rx) = mpsc::channel();
            stored.query = Some(rx);
            stored.requested = Some(query.clone());
            stored.open = self.site_map_ui.open.clone();
            let open = stored.open.clone();
            let selected = self.selected;
            stored.query_cancelled = Arc::new(AtomicBool::new(false));
            let cancelled = stored.query_cancelled.clone();
            std::thread::spawn(move || {
                let result = if let Some(view) = expansion_view {
                    view.with_open(&open, &cancelled)
                } else {
                    viewport::View::build_selected(&index, &query, &open, &cancelled, selected)
                };
                let _ = tx.send(result);
            });
        }
        stored.poll_window();
        if let Some(id) = self.selected
            && self.history.get(id).is_none()
        {
            stored.select(snapshot, id);
        }
        // Also poll the commit revision after a quiet period: capture commits
        // can lag the UI event that triggered the preceding frame.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        self.site_map_windows(ui.ctx());
    }
}

fn stored_tree_row(
    ui: &mut egui::Ui,
    node: &viewport::TreeRow,
    open: bool,
    selected: bool,
) -> (bool, bool) {
    let indent = 4.0 + node.depth as f32 * 16.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(indent + 160.0), 21.0),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, egui::CornerRadius::same(2), theme::SELECTED);
    } else if response.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::same(2), theme::ACCENT_WASH);
    }
    let arrow = egui::pos2(rect.left() + indent + 6.0, rect.center().y);
    if node.branch {
        paint_chevron(painter, arrow, open, theme::TEXT_MUTED);
    }
    let icon = arrow + egui::vec2(16.0, 0.0);
    match &node.id {
        SiteNodeId::Host { .. } if node.scheme == "https" => paint_lock(painter, icon, theme::BLUE),
        SiteNodeId::Host { .. } if node.scheme == "mixed" => paint_mixed_scheme(painter, icon),
        SiteNodeId::Host { .. } => paint_globe(painter, icon, theme::GRAY),
        SiteNodeId::Directory { path, .. } if path == "/" => {
            paint_gear_icon(painter, icon, theme::GRAY)
        }
        SiteNodeId::Directory { .. } => paint_folder(painter, icon),
        SiteNodeId::Endpoint { .. } => paint_file(painter, icon),
    }
    painter.text(
        egui::pos2(rect.left() + indent + 34.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &node.label,
        egui::FontId::proportional(11.8),
        theme::TEXT,
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, selected, &node.label)
    });
    let toggled = node.branch
        && (response.double_clicked()
            || (response.clicked()
                && response
                    .interact_pointer_pos()
                    .is_some_and(|p| p.x < arrow.x + 8.0)));
    let picked = response.clicked() && !toggled;
    response
        .on_hover_text(format!("{} retained endpoints", node.count))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    (picked, toggled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectHandle, RetentionLimits};
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
    };

    #[test]
    fn pending_view_does_not_consume_previous_window() {
        let mut stored = StoredMap::default();
        let (_query_tx, query_rx) = mpsc::channel();
        stored.query = Some(query_rx);
        let (window_tx, window_rx) = mpsc::channel();
        let window = viewport::Window {
            row: 123,
            ..Default::default()
        };
        window_tx.send(Ok(window)).unwrap();
        stored.window_query = Some(window_rx);
        stored.poll_window();
        assert_eq!(stored.window.row, 0);
        assert!(stored.window_query.is_some());
    }

    #[test]
    fn detail_panel_keeps_height_without_selected_exchange() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        for _ in 0..3 {
            let mut height = 0.0;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let panel = egui::Panel::bottom(SITE_MAP_DETAIL_ID)
                        .resizable(true)
                        .default_size(400.0)
                        .min_size(190.0)
                        .show(ui, |ui| {
                            ui.take_available_space();
                            app.detail_panel(ui);
                        });
                    height = panel.response.rect.height();
                },
            )
            .drop_without_applying_deltas();
            assert!(height >= 400.0, "detail panel shrank to {height}");
        }
    }

    fn wait<T>(rx: Reply<T>) -> T {
        rx.recv_timeout(Duration::from_secs(60)).unwrap().unwrap()
    }

    fn capture(project: &ProjectHandle, first: u64, count: u64) {
        for id in first..first + count {
            for event in [
                ProxyEvent::Started {
                    id,
                    timestamp: SystemTime::UNIX_EPOCH,
                    request: RequestInfo {
                        method: "POST".into(),
                        uri: format!("https://example.test/{id:06}?a=1"),
                        host: "example.test".into(),
                        path: format!("/{id:06}?a=1"),
                        scheme: "https".into(),
                        version: "HTTP/1.1".into(),
                        headers: vec![],
                    },
                },
                ProxyEvent::ResponseHead {
                    id,
                    head: ResponseHead {
                        version: "HTTP/1.1".into(),
                        status: 200,
                        reason: "OK".into(),
                        headers: vec![("content-type".into(), "text/html".into())],
                    },
                },
                ProxyEvent::Finished {
                    id,
                    end: ExchangeEnd {
                        duration: Duration::from_millis(1),
                        request_body: CapturedBody::from_bytes(vec![0, 255, 1]),
                        result: ExchangeResult::Response {
                            body: CapturedBody::from_bytes(b"<title>Stored title</title>".to_vec()),
                        },
                    },
                },
            ] {
                let captured = project.capture(&event);
                assert!(captured.event.is_some());
            }
            if id % 100 == 0 {
                wait(project.flush());
            }
        }
        wait(project.flush());
    }

    fn query() -> Query {
        Query {
            node: Some(SiteNodeId::Host {
                host: "example.test".into(),
            }),
            filter: SiteMapFilter::SHOW_ALL,
            scope: None,
            search: String::new(),
            sort: SiteMapSort::Time,
            ascending: true,
            notes: BTreeMap::new(),
            cursor: None,
            backwards: false,
            tree_after: None,
        }
    }

    fn index(project: &ProjectHandle) -> Index {
        Index::build(
            &project.database_snapshot().unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn continuous_view_matches_repeated_slashes_with_sparse_scope_boundaries() {
        let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0)).unwrap();
        capture(&project, 1, 1);
        let original = index(&project);
        let cancel = AtomicBool::new(false);
        let base = original.query(&query(), &cancel).unwrap().entries.remove(0);
        let source = Index::empty().unwrap();
        let write = source.db.as_ref().unwrap().begin_write().unwrap();
        {
            let mut entries = write.open_table(ENTRIES).unwrap();
            write.open_table(HOST_SCHEMES).unwrap();
            for i in 0..1024 {
                let mut entry = base.clone();
                entry.id = i + 1;
                entry.path = format!("/a//b?item={i}");
                // Exclude every old commit boundary while retaining writes
                // throughout the scan.
                if i % 256 == 255 {
                    entry.host = "excluded.test".into();
                }
                let key = format!("{i:04}");
                let bytes = serde_json::to_vec(&entry).unwrap();
                entries.insert(key.as_str(), bytes.as_slice()).unwrap();
            }
        }
        write.commit().unwrap();
        let mut query = query();
        query.scope = Some(BTreeSet::from(["example.test".into()]));
        for node in [
            SiteNodeId::Directory {
                host: "example.test".into(),
                path: "/a".into(),
            },
            SiteNodeId::Endpoint {
                host: "example.test".into(),
                path: "/a/b".into(),
            },
        ] {
            query.node = Some(node);
            let view = viewport::View::build(&source, &query, &BTreeMap::new(), &cancel).unwrap();
            assert_eq!(view.total, 1020);
            let window = view.window(0, 0).unwrap();
            assert_eq!(window.entries.len(), PAGE);
            assert!(
                window
                    .entries
                    .iter()
                    .all(|entry| entry.path.starts_with("/a//b?"))
            );
        }
    }

    #[test]
    fn reopened_store_exceeds_cache_pages_tied_timestamps_and_loads_old_details_lazily() {
        let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0)).unwrap();
        capture(&project, 1, 1003);
        let path = std::env::temp_dir().join(format!(
            "perseid-site-map-test-{}.perseid",
            uuid::Uuid::new_v4()
        ));
        wait(project.save_as(path.clone(), false));
        wait(project.new_project(10, 1024));
        let opened = wait(project.open(path.clone(), 10, 1024));
        assert!(opened.http.len() <= 10);
        assert!(opened.http.iter().all(|item| item.id > 1));
        let index = index(&project);
        let cancel = AtomicBool::new(false);
        let mut query = query();
        let first = index.query(&query, &cancel).unwrap();
        assert_eq!(first.total, 1003);
        assert_eq!(first.entries.len(), PAGE);
        assert_eq!(first.entries[0].id, 1);
        assert_eq!(first.entries[0].title, "Stored title");
        assert_eq!(first.entries[0].params, 1);
        assert!(first.more && first.tree_more);
        let mut seen: Vec<_> = first.entries.iter().map(|e| e.id).collect();
        query.cursor = first.entries.last().cloned();
        let second = index.query(&query, &cancel).unwrap();
        seen.extend(second.entries.iter().map(|e| e.id));
        query.cursor = second.entries.last().cloned();
        let third = index.query(&query, &cancel).unwrap();
        seen.extend(third.entries.iter().map(|e| e.id));
        assert!(!third.more);
        assert_eq!(seen, (1..=1003).collect::<Vec<_>>());
        // The production view supports direct ordinal jumps with bounded reads.
        query.cursor = None;
        let view = viewport::View::build(&index, &query, &BTreeMap::new(), &cancel).unwrap();
        assert_eq!((view.total, view.tree_total), (1003, 1005));
        let end = view.window(1000, 1000).unwrap();
        assert_eq!(
            end.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![1001, 1002, 1003]
        );
        assert_eq!(end.nodes.last().unwrap().label, "001003");
        let first_window = view.window(0, 0).unwrap();
        assert_eq!(first_window.entries.len(), PAGE);
        assert_eq!(first_window.nodes.len(), PAGE);
        assert_eq!(first_window.nodes[0].label, "example.test");
        assert_eq!(first_window.nodes[1].label, "/");
        let mut open = BTreeMap::new();
        open.insert("host:example.test".into(), false);
        let collapsed = view.with_open(&open, &cancel).unwrap();
        assert_eq!(collapsed.tree_total, 1);
        assert_eq!(collapsed.total, 1003);
        let expanded = collapsed.with_open(&BTreeMap::new(), &cancel).unwrap();
        assert_eq!(expanded.tree_total, view.tree_total);
        assert_eq!(
            expanded
                .window(1000, 1000)
                .unwrap()
                .entries
                .iter()
                .map(|e| e.id)
                .collect::<Vec<_>>(),
            vec![1001, 1002, 1003]
        );
        assert_eq!(expanded.with_open(&open, &cancel).unwrap().tree_total, 1);
        assert!(expanded.with_open(&open, &AtomicBool::new(true)).is_err());
        query.cursor = third.entries.first().cloned();
        query.backwards = true;
        let previous = index.query(&query, &cancel).unwrap();
        assert_eq!(
            previous.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            (501..=1000).collect::<Vec<_>>()
        );
        query.tree_after = first.children.last().map(|n| n.label.clone());
        let next_nodes = index.query(&query, &cancel).unwrap();
        assert_eq!(next_nodes.children[0].label, "000501");
        // Search is global, not restricted to the first displayed page.
        query.cursor = None;
        query.backwards = false;
        query.search = "001003".into();
        let searched = index.query(&query, &cancel).unwrap();
        assert_eq!(searched.total, 1);
        assert_eq!(searched.entries[0].id, 1003);
        assert!(!searched.more);
        let snapshot = project.database_snapshot().unwrap();
        let mut stored = StoredMap::default();
        stored.select(snapshot, 1);
        assert!(stored.detail.is_none());
        let (_, rx) = stored.detail_query.take().unwrap();
        let item = wait(rx).unwrap();
        assert_eq!(item.request_body.data(), &[0, 255, 1]);
        assert_eq!(
            item.response_body().unwrap().data(),
            b"<title>Stored title</title>"
        );
        drop(index);
        project.shutdown();
        drop(project);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn filters_scope_notes_search_and_all_sorts_match_existing_semantics() {
        let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0)).unwrap();
        capture(&project, 1, 12);
        let index = index(&project);
        let cancel = AtomicBool::new(false);
        let mut query = query();
        query.notes.insert(1, "Important old note".into());
        query.search = "important".into();
        assert_eq!(index.query(&query, &cancel).unwrap().entries[0].id, 1);
        let searched = viewport::View::build(&index, &query, &BTreeMap::new(), &cancel).unwrap();
        assert_eq!(searched.total, 1);
        assert_eq!(searched.window(0, 0).unwrap().entries[0].id, 1);
        query.search = "stored title".into();
        assert_eq!(index.query(&query, &cancel).unwrap().total, 12);
        query.search.clear();
        query.scope = Some(BTreeSet::from(["elsewhere.test".into()]));
        let empty = index.query(&query, &cancel).unwrap();
        assert!(empty.entries.is_empty() && empty.children.is_empty());
        let empty_view = viewport::View::build(&index, &query, &BTreeMap::new(), &cancel).unwrap();
        assert_eq!((empty_view.total, empty_view.tree_total), (0, 0));
        query.scope = Some(BTreeSet::new());
        query.filter.parameterized_only = true;
        for sort in [
            SiteMapSort::Host,
            SiteMapSort::Method,
            SiteMapSort::Url,
            SiteMapSort::Params,
            SiteMapSort::Status,
            SiteMapSort::Length,
            SiteMapSort::Mime,
            SiteMapSort::Title,
            SiteMapSort::Notes,
            SiteMapSort::Time,
        ] {
            query.sort = sort;
            for ascending in [true, false] {
                query.ascending = ascending;
                let page = index.query(&query, &cancel).unwrap();
                let view =
                    viewport::View::build(&index, &query, &BTreeMap::new(), &cancel).unwrap();
                let window = view.window(0, 0).unwrap();
                assert_eq!(
                    window.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
                    page.entries.iter().map(|e| e.id).collect::<Vec<_>>()
                );
                assert_eq!(page.entries.len(), 12);
                assert!(
                    page.entries
                        .windows(2)
                        .all(|w| !query.compare(&w[0], &w[1]).is_gt())
                );
                let mut expected: Vec<_> = page.entries.iter().map(|e| query.row(e)).collect();
                sort_site_rows(&mut expected, sort, ascending);
                assert_eq!(
                    expected.iter().map(|r| r.entry.id).collect::<Vec<_>>(),
                    page.entries.iter().map(|e| e.id).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn disk_sort_keys_match_column_comparators_for_prefixes_ties_and_pre_epoch_times() {
        let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0)).unwrap();
        capture(&project, 1, 1);
        let source = index(&project);
        let mut query = query();
        let base = source
            .query(&query, &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        let entries: Vec<_> = ["", "A", "a", "aa", "é", "中", "\0"]
            .into_iter()
            .enumerate()
            .map(|(i, value)| {
                let mut entry = base.clone();
                entry.id = i as u64;
                entry.host = format!("{value}.test");
                entry.path = format!("/{value}");
                entry.method = value.into();
                entry.title = value.into();
                entry.mime = value.into();
                entry.params = i;
                entry.status = if i == 0 { None } else { Some(200 + i as u16) };
                entry.length = i as u64 * 257;
                entry.timestamp = if i % 2 == 0 {
                    SystemTime::UNIX_EPOCH - Duration::from_nanos(i as u64)
                } else {
                    SystemTime::UNIX_EPOCH + Duration::from_nanos(i as u64)
                };
                query.notes.insert(entry.id, value.into());
                entry
            })
            .collect();
        for sort in [
            SiteMapSort::Host,
            SiteMapSort::Method,
            SiteMapSort::Url,
            SiteMapSort::Params,
            SiteMapSort::Status,
            SiteMapSort::Length,
            SiteMapSort::Mime,
            SiteMapSort::Title,
            SiteMapSort::Notes,
            SiteMapSort::Time,
        ] {
            query.sort = sort;
            for ascending in [true, false] {
                query.ascending = ascending;
                for left in &entries {
                    for right in &entries {
                        assert_eq!(
                            viewport::sort_key(&query, left)
                                .cmp(&viewport::sort_key(&query, right)),
                            query.compare(left, right),
                            "{sort:?} ascending={ascending}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn grouping_uses_latest_id_preserves_counts_schemes_and_filter_metadata() {
        let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0)).unwrap();
        for (id, scheme, timestamp, path, method, status, mime) in [
            (1, "http", 9000, "/a?x=1", "GET", 200, "text/html"),
            (2, "https", 1, "/a?x=1", "GET", 404, "text/html"),
            (3, "https", 1, "/a?x=2", "GET", 200, "text/html"),
            (4, "https", 1, "/a", "POST", 200, "text/html"),
            (5, "https", 1, "/assets/a.css", "GET", 200, "text/css"),
            (6, "https", 1, "", "CONNECT", 200, ""),
        ] {
            for event in [
                ProxyEvent::Started {
                    id,
                    timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp),
                    request: RequestInfo {
                        method: method.into(),
                        uri: format!("{scheme}://example.test{path}"),
                        scheme: scheme.into(),
                        host: "example.test".into(),
                        path: path.into(),
                        version: "HTTP/1.1".into(),
                        headers: vec![],
                    },
                },
                ProxyEvent::ResponseHead {
                    id,
                    head: ResponseHead {
                        version: "HTTP/1.1".into(),
                        status,
                        reason: String::new(),
                        headers: vec![("content-type".into(), mime.into())],
                    },
                },
            ] {
                let captured = project.capture(&event);
                assert_eq!(
                    captured.event.is_none(),
                    matches!(event, ProxyEvent::Started { ref request, .. } if request.method == "CONNECT")
                );
            }
        }
        wait(project.flush());
        let index = index(&project);
        let cancel = AtomicBool::new(false);
        let mut query = query();
        let page = index.query(&query, &cancel).unwrap();
        assert_eq!(page.total, 4);
        let latest = page.entries.iter().find(|e| e.path == "/a?x=1").unwrap();
        assert_eq!((latest.id, latest.count, latest.status), (2, 2, Some(404)));
        assert_eq!(
            page.children.iter().find(|n| n.label == "a").unwrap().count,
            3
        );
        query.node = None;
        assert_eq!(
            index.query(&query, &cancel).unwrap().children[0].scheme,
            "mixed"
        );
        query.node = Some(SiteNodeId::Host {
            host: "example.test".into(),
        });
        query.filter = SiteMapFilter::default();
        let page = index.query(&query, &cancel).unwrap();
        assert_eq!(page.total, 2);
        assert!(page.children.iter().all(|n| n.label != "assets"));
        query.filter.hide_empty_folders = false;
        let page = index.query(&query, &cancel).unwrap();
        assert_eq!(
            page.children
                .iter()
                .find(|n| n.label == "assets")
                .unwrap()
                .count,
            0
        );
        query.filter = SiteMapFilter::SHOW_ALL;
        query.node = Some(SiteNodeId::Endpoint {
            host: "example.test".into(),
            path: "/a".into(),
        });
        assert_eq!(index.query(&query, &cancel).unwrap().total, 3);
        let mut open = BTreeMap::new();
        open.insert("dir:example.test:/assets".into(), true);
        let view = viewport::View::build(&index, &query, &open, &cancel).unwrap();
        let window = view.window(0, 0).unwrap();
        assert_eq!(view.total, 3);
        assert_eq!(window.nodes[0].scheme, "mixed");
        let asset = window.nodes.iter().find(|n| n.label == "a.css").unwrap();
        assert_eq!(asset.depth, 3);
        assert!(!asset.branch);
        query.filter = SiteMapFilter::default();
        let filtered = viewport::View::build(&index, &query, &open, &cancel).unwrap();
        assert!(
            !filtered
                .window(0, 0)
                .unwrap()
                .nodes
                .iter()
                .any(|n| n.label == "assets")
        );
    }

    fn frame(app: &mut PerseidApp, ctx: &egui::Context) {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| app.stored_site_map_view(ui),
        )
        .drop_without_applying_deltas();
    }

    fn settle(app: &mut PerseidApp, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            frame(app, ctx);
            let stored = &app.site_map_ui.stored;
            assert!(stored.error.is_none(), "{:?}", stored.error);
            if stored.index.is_some()
                && stored.build.is_none()
                && stored.query.is_none()
                && stored.window_query.is_none()
                && stored.detail_query.is_none()
                && stored.pending_row.is_none()
                && (app.site_map_ui.selected_node.is_some() || stored.window.nodes.is_empty())
            {
                break;
            }
            assert!(Instant::now() < deadline, "site-map worker timed out");
            std::thread::yield_now();
        }
    }

    fn click(app: &mut PerseidApp, ctx: &egui::Context, label: &str) {
        click_tree_or_control(app, ctx, label, false);
    }

    fn click_tree_or_control(app: &mut PerseidApp, ctx: &egui::Context, label: &str, arrow: bool) {
        fn find(shape: &egui::Shape, label: &str, arrow: bool) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(if arrow {
                    text.pos + egui::vec2(-28.0, text.galley.size().y * 0.5)
                } else {
                    text.pos + text.galley.size() * 0.5
                }),
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label, arrow)),
                _ => None,
            }
        }
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| app.stored_site_map_view(ui),
        );
        let pos = output
            .shapes
            .iter()
            .find_map(|s| find(&s.shape, label, arrow))
            .unwrap_or_else(|| panic!("missing control {label}"));
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400.0, 900.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| app.stored_site_map_view(ui),
            )
            .drop_without_applying_deltas();
        }
        settle(app, ctx);
    }

    #[test]
    fn continuous_contents_keyboard_navigation_crosses_window_boundaries() {
        let mut app = PerseidApp::test_app();
        capture(app.project.handle().unwrap(), 1, 1003);
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(1));
        app.site_map_ui.stored.navigate(499);
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(500));
        app.move_site_map_selection(1);
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(501));
        app.select_site_map_edge(false);
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(1003));
        assert!(app.site_map_ui.stored.window.entries.len() <= PAGE);
        app.select_site_map_edge(true);
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(1));
        assert!(app.site_map_ui.stored.item(1).is_some());
        click(&mut app, &ctx, "000001");
        assert_eq!(app.site_map_ui.stored.total(), 1);
    }

    #[test]
    fn continuous_view_rejects_stale_queries_and_has_no_paging_controls() {
        let mut app = PerseidApp::test_app();
        capture(app.project.handle().unwrap(), 1, 1003);
        let ctx = egui::Context::default();
        settle(&mut app, &ctx);
        app.select_site_map_edge(false);
        settle(&mut app, &ctx);
        app.site_map_ui.search = "000002".into();
        frame(&mut app, &ctx);
        app.site_map_ui.search = "000003".into();
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.total(), 1);
        assert_eq!(app.selected, Some(3));
        app.site_map_ui.search.clear();
        app.site_map_ui.sort_ascending = false;
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(3));
        assert_eq!(app.site_map_ui.stored.selected_row, Some(1000));
        click_tree_or_control(&mut app, &ctx, "example.test", true);
        assert_eq!(app.site_map_ui.stored.view.as_ref().unwrap().tree_total, 1);
        click_tree_or_control(&mut app, &ctx, "example.test", true);
        assert_eq!(
            app.site_map_ui.stored.view.as_ref().unwrap().tree_total,
            1005
        );
        let output = ctx.run_ui(egui::RawInput::default(), |ui| app.stored_site_map_view(ui));
        fn labels(shape: &egui::Shape, text: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => text.push(t.galley.text().into()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        labels(shape, text);
                    }
                }
                _ => {}
            }
        }
        let mut text = Vec::new();
        for shape in &output.shapes {
            labels(&shape.shape, &mut text);
        }
        for removed in [
            "Next contents",
            "Previous contents",
            "First contents",
            "More nodes",
            "First nodes",
            "↑ Parent",
        ] {
            assert!(!text.iter().any(|s| s == removed));
        }
        output.drop_without_applying_deltas();
    }

    #[test]
    fn wheel_scrolling_loads_distant_contents_and_tree_without_changing_selection() {
        let mut app = PerseidApp::test_app();
        capture(app.project.handle().unwrap(), 1, 1003);
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        settle(&mut app, &ctx);
        for pos in [egui::pos2(800.0, 150.0), egui::pos2(100.0, 150.0)] {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400.0, 900.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Wheel,
                            delta: egui::vec2(0.0, -30_000.0),
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| app.stored_site_map_view(ui),
            )
            .drop_without_applying_deltas();
            settle(&mut app, &ctx);
        }
        assert!(app.site_map_ui.stored.window.row > 500);
        assert!(app.site_map_ui.stored.window.tree > 500);
        assert_eq!(app.selected, Some(1));
        assert!(app.site_map_ui.stored.item(1).is_some());
        assert!(app.site_map_ui.stored.window.entries.len() <= PAGE);
        assert!(app.site_map_ui.stored.window.nodes.len() <= PAGE);
    }

    #[test]
    fn temporary_index_is_private_disposable_and_cancelled_queries_are_bounded() {
        let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0)).unwrap();
        capture(&project, 1, 2);
        let index = index(&project);
        let directory = index.directory.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let page = index.query(&query(), &AtomicBool::new(true)).unwrap();
        assert!(page.entries.is_empty() && page.children.is_empty());
        assert!(
            viewport::View::build(&index, &query(), &BTreeMap::new(), &AtomicBool::new(true))
                .is_err()
        );
        drop(index);
        assert!(!directory.exists());
        let (tx, rx) = mpsc::channel::<Result<Page>>();
        let mut reply = Some(rx);
        assert!(receive(&mut reply).is_none());
        drop(tx);
        assert!(matches!(
            receive(&mut reply),
            Some(Err(ProjectError::ServiceUnavailable))
        ));
        assert!(reply.is_none());
    }

    #[test]
    fn old_navigation_notes_details_tools_pause_resume_retention_and_clear() {
        let mut app = PerseidApp::test_app();
        app.main_tab = MainTab::Target;
        capture(app.project.handle().unwrap(), 1, 1003);
        let ctx = egui::Context::default();
        settle(&mut app, &ctx);
        app.site_map_ui.selected_node = query().node;
        app.site_map_ui.sort = SiteMapSort::Time;
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.window.entries.len(), PAGE);
        app.select_site_map_edge(true);
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(1));
        assert!(app.history.get(1).is_none());
        assert!(app.site_map_ui.stored.item(1).is_some());
        app.ensure_detail(1);
        assert!(app.detail.is_some());
        app.send_selected_to_repeater();
        assert_eq!(app.repeater_tabs.len(), 1);
        assert!(app.history.get(1).is_none());
        app.main_tab = MainTab::Target;
        app.site_map_ui.paused = true;
        capture(app.project.handle().unwrap(), 1004, 1);
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.total(), 1003);
        app.site_map_ui.paused = false;
        app.site_map_ui.stored.refreshed = None;
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.total(), 1004);
        app.site_map_ui.paused = true;
        wait(
            app.project
                .handle()
                .unwrap()
                .set_retention(RetentionLimits::new(3, 0)),
        );
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.total(), 3);
        assert!(app.site_map_ui.stored.item(1).is_none());
        wait(app.project.handle().unwrap().clear_http());
        settle(&mut app, &ctx);
        assert!(app.site_map_ui.stored.window.entries.is_empty());
    }

    #[test]
    fn project_switch_rejects_pending_build_page_and_detail_even_when_ids_are_reused() {
        let mut app = PerseidApp::test_app();
        capture(app.project.handle().unwrap(), 1, 2);
        let ctx = egui::Context::default();
        settle(&mut app, &ctx);
        let generation = app.site_map_ui.stored.generation;
        let (tx, rx) = mpsc::channel();
        app.site_map_ui.stored.query = Some(rx);
        tx.send(viewport::View::build(
            &index(app.project.handle().unwrap()),
            &query(),
            &BTreeMap::new(),
            &AtomicBool::new(false),
        ))
        .unwrap();
        app.site_map_ui.stored.select(
            app.project.handle().unwrap().database_snapshot().unwrap(),
            1,
        );
        let old_cancelled = app.site_map_ui.stored.cancelled.clone();
        wait(app.project.handle().unwrap().new_project(10, 1024));
        settle(&mut app, &ctx);
        assert!(old_cancelled.load(Ordering::Relaxed));
        assert_ne!(app.site_map_ui.stored.generation, generation);
        assert!(app.site_map_ui.stored.window.entries.is_empty());
        assert!(app.site_map_ui.stored.detail.is_none());
        assert!(app.selected.is_none());
        capture(app.project.handle().unwrap(), 1, 1);
        app.site_map_ui.stored.refreshed = None;
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.window.nodes[0].count, 1);
    }

    #[test]
    #[ignore = "manual large synthetic project smoke check"]
    fn large_synthetic_project() {
        let mut app = PerseidApp::test_app();
        let project = app.project.handle().unwrap();
        wait(project.set_retention(RetentionLimits::new(0, 0)));
        capture(project, 1, 25_003);
        let path = std::env::temp_dir().join(format!(
            "perseid-large-site-map-{}.perseid",
            uuid::Uuid::new_v4()
        ));
        wait(project.save_as(path.clone(), false));
        wait(project.new_project(10, 1024));
        let opened = wait(project.open(path.clone(), 10, 1024));
        app.history.load(opened.http);
        let start = Instant::now();
        let index = index(app.project.handle().unwrap());
        let built = start.elapsed();
        let view_start = Instant::now();
        let view =
            viewport::View::build(&index, &query(), &BTreeMap::new(), &AtomicBool::new(false))
                .unwrap();
        let view_built = view_start.elapsed();
        let window_start = Instant::now();
        let window = view.window(24_500, 24_500).unwrap();
        eprintln!(
            "25,003 retained endpoints: index {built:?}, view {view_built:?}, distant window {:?}, rows {}, tree nodes {}",
            window_start.elapsed(),
            window.entries.len(),
            window.nodes.len()
        );
        assert_eq!(view.total, 25_003);
        assert_eq!(window.entries.len(), PAGE);
        assert_eq!(window.nodes.len(), PAGE);
        let status = app.project.status().unwrap();
        app.site_map_ui.stored.reset(status.generation);
        app.site_map_ui.stored.revision = (
            status.last_committed_sequence,
            status.retention_evictions,
            status.http_count,
        );
        app.site_map_ui.stored.index = Some(Arc::new(index));
        let ctx = egui::Context::default();
        settle(&mut app, &ctx);
        app.site_map_ui.stored.navigate(500);
        settle(&mut app, &ctx);
        assert_eq!(app.selected, Some(501));
        app.select_site_map_edge(true);
        settle(&mut app, &ctx);
        click(&mut app, &ctx, "000001");
        assert_eq!(app.selected, Some(1));
        assert!(app.site_map_ui.stored.item(1).is_some());
        app.send_selected_to_repeater();
        assert_eq!(app.repeater_tabs.len(), 1);
        assert!(app.history.len() <= 10 && app.history.get(1).is_none());
        eprintln!(
            "Reopened 25,003-record project: continuous Contents navigation, tree clicks, old details, and Repeater send passed; live cache {} records.",
            app.history.len()
        );
        drop(app);
        std::fs::remove_file(path).unwrap();
    }
}
