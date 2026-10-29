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
    fn build(snapshot: &ProjectSnapshot, cancelled: &AtomicBool) -> Result<Self> {
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
    cursor: Option<SiteEntry>,
    backwards: bool,
    tree_after: Option<String>,
}

impl Query {
    fn current(app: &PerseidApp, stored: &StoredMap) -> Self {
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
            cursor: stored.cursor.clone(),
            backwards: stored.backwards,
            tree_after: stored.tree_after.clone(),
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
    query: Option<Reply<Page>>,
    requested: Option<Query>,
    page: Page,
    cursor: Option<SiteEntry>,
    backwards: bool,
    tree_after: Option<String>,
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
        let current = Query::current(self, &stored);
        if let Some(result) = receive(&mut stored.query)
            && stored.requested.as_ref() == Some(&current)
        {
            match result {
                Ok(page) => stored.page = page,
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

        let mut node_changed = false;
        egui::Panel::left(TARGET_TREE_ID)
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                ui.weak("Retained endpoints · paged URL tree");
                if let Some(node) = &self.site_map_ui.selected_node {
                    ui.label(match node {
                        SiteNodeId::Host { host } => host.clone(),
                        SiteNodeId::Directory { host, path }
                        | SiteNodeId::Endpoint { host, path } => format!("{host}{path}"),
                    });
                    if ui.button("↑ Parent").clicked() {
                        self.site_map_ui.selected_node = parent_node(node);
                        stored.tree_after = None;
                        node_changed = true;
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            stored.query.is_none() && stored.tree_after.is_some(),
                            egui::Button::new("First nodes"),
                        )
                        .clicked()
                    {
                        stored.tree_after = None;
                    }
                    if ui
                        .add_enabled(
                            stored.query.is_none() && stored.page.tree_more,
                            egui::Button::new("More nodes"),
                        )
                        .clicked()
                    {
                        stored.tree_after = stored.page.children.last().map(|n| n.label.clone());
                    }
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for node in &stored.page.children {
                        let label = format!(
                            "{} {} ({})",
                            if node.is_directory() { "▸" } else { "·" },
                            node.label,
                            node.count
                        );
                        if ui
                            .selectable_label(false, label)
                            .on_hover_text(&node.scheme)
                            .clicked()
                        {
                            self.site_map_ui.selected_node = Some(node.id.clone());
                            stored.tree_after = None;
                            node_changed = true;
                        }
                    }
                });
            });
        if node_changed {
            stored.cursor = None;
            stored.backwards = false;
            stored.page = Page::default();
        }

        // Reuse the existing Contents renderer and shared details with only one
        // page of summaries and one hydrated exchange resident in the UI.
        let mut hosts: BTreeMap<String, Vec<SiteEntry>> = BTreeMap::new();
        for entry in &stored.page.entries {
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
        let page_more = stored.page.more;
        let first = stored.page.entries.first().cloned();
        let last = stored.page.entries.last().cloned();
        let total = stored.page.total;
        let busy = stored.build.is_some() || stored.query.is_some();
        let error = stored.error.clone();
        self.site_map_ui.stored = stored;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                let stored = &mut self.site_map_ui.stored;
                if ui
                    .add_enabled(
                        !busy && stored.cursor.is_some(),
                        egui::Button::new("First contents"),
                    )
                    .clicked()
                {
                    stored.cursor = None;
                    stored.backwards = false;
                }
                if ui
                    .add_enabled(
                        !busy && stored.cursor.is_some() && (!stored.backwards || page_more),
                        egui::Button::new("Previous contents"),
                    )
                    .clicked()
                {
                    stored.cursor = first;
                    stored.backwards = true;
                }
                if ui
                    .add_enabled(
                        !busy && (page_more || self.site_map_ui.stored.backwards),
                        egui::Button::new("Next contents"),
                    )
                    .clicked()
                {
                    self.site_map_ui.stored.cursor = last;
                    self.site_map_ui.stored.backwards = false;
                }
                ui.label(format!(
                    "{} / {total} endpoints",
                    map.hosts.iter().map(|h| h.entries.len()).sum::<usize>()
                ));
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
            egui::Panel::bottom(SITE_MAP_DETAIL_ID)
                .resizable(true)
                .default_size(260.0)
                .show(ui, |ui| self.detail_panel(ui));
            egui::CentralPanel::default().show(ui, |ui| {
                if self.site_map_ui.selected_node.is_none() {
                    ui.label("Select a host to browse all its retained endpoints.");
                } else if let Some(action) = self.site_map_contents(ui, &map, &[], node_changed) {
                    self.apply_site_map_row_action(action);
                }
            });
        });
        let query = Query::current(self, &self.site_map_ui.stored);
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
        let mut query = query;
        if controls_changed {
            if stored
                .requested
                .as_ref()
                .is_some_and(|old| old.filter != query.filter || old.scope != query.scope)
            {
                stored.tree_after = None;
                query.tree_after = None;
                stored.page.children.clear();
            }
            stored.cursor = None;
            stored.backwards = false;
            query.cursor = None;
            query.backwards = false;
            stored.page.entries.clear();
            self.site_map_ui.visible_ids.clear();
        }
        if stored.requested.as_ref() != Some(&query)
            && stored.query.is_none()
            && let Some(index) = stored.index.clone()
        {
            let (tx, rx) = mpsc::channel();
            stored.query = Some(rx);
            stored.requested = Some(query.clone());
            stored.query_cancelled = Arc::new(AtomicBool::new(false));
            let cancelled = stored.query_cancelled.clone();
            std::thread::spawn(move || {
                let _ = tx.send(index.query(&query, &cancelled));
            });
        }
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

fn parent_node(node: &SiteNodeId) -> Option<SiteNodeId> {
    match node {
        SiteNodeId::Host { .. } => None,
        SiteNodeId::Directory { host, path } | SiteNodeId::Endpoint { host, path } => {
            let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or_default();
            if parent.is_empty() {
                Some(SiteNodeId::Host { host: host.clone() })
            } else {
                Some(SiteNodeId::Directory {
                    host: host.clone(),
                    path: parent.into(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectHandle, RetentionLimits};
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
    };

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
        query.search = "stored title".into();
        assert_eq!(index.query(&query, &cancel).unwrap().total, 12);
        query.search.clear();
        query.scope = Some(BTreeSet::from(["elsewhere.test".into()]));
        let empty = index.query(&query, &cancel).unwrap();
        assert!(empty.entries.is_empty() && empty.children.is_empty());
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
                && stored.detail_query.is_none()
            {
                break;
            }
            assert!(Instant::now() < deadline, "site-map worker timed out");
            std::thread::yield_now();
        }
    }

    fn click(app: &mut PerseidApp, ctx: &egui::Context, label: &str) {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
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
            .find_map(|s| find(&s.shape, label))
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
    fn paged_tree_and_contents_controls_navigate_old_rows() {
        let mut app = PerseidApp::test_app();
        capture(app.project.handle().unwrap(), 1, 1003);
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        settle(&mut app, &ctx);
        click(&mut app, &ctx, "▸ example.test (1003)");
        assert_eq!(app.site_map_ui.stored.page.entries[0].id, 1);
        click(&mut app, &ctx, "Next contents");
        assert_eq!(app.site_map_ui.stored.page.entries[0].id, 501);
        click(&mut app, &ctx, "Next contents");
        assert_eq!(app.site_map_ui.stored.page.entries[0].id, 1001);
        click(&mut app, &ctx, "Previous contents");
        assert_eq!(app.site_map_ui.stored.page.entries[0].id, 501);
        click(&mut app, &ctx, "First contents");
        assert_eq!(app.site_map_ui.stored.page.entries[0].id, 1);
        click(&mut app, &ctx, "More nodes");
        assert_eq!(app.site_map_ui.stored.page.children[0].label, "000501");
        click(&mut app, &ctx, "First nodes");
        click(&mut app, &ctx, "· 000001 (1)");
        assert_eq!(app.site_map_ui.stored.page.entries.len(), 1);
        assert_eq!(app.site_map_ui.stored.page.entries[0].id, 1);
        assert!(app.site_map_ui.stored.item(1).is_some());
        click(&mut app, &ctx, "↑ Parent");
        assert_eq!(app.site_map_ui.stored.page.total, 1003);
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
        assert_eq!(app.site_map_ui.stored.page.entries.len(), PAGE);
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
        assert_eq!(app.site_map_ui.stored.page.total, 1003);
        app.site_map_ui.paused = false;
        app.site_map_ui.stored.refreshed = None;
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.page.total, 1004);
        app.site_map_ui.paused = true;
        wait(
            app.project
                .handle()
                .unwrap()
                .set_retention(RetentionLimits::new(3, 0)),
        );
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.page.total, 3);
        assert!(app.site_map_ui.stored.item(1).is_none());
        wait(app.project.handle().unwrap().clear_http());
        settle(&mut app, &ctx);
        assert!(app.site_map_ui.stored.page.entries.is_empty());
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
        tx.send(Ok(Page {
            entries: vec![
                index(app.project.handle().unwrap())
                    .query(&query(), &AtomicBool::new(false))
                    .unwrap()
                    .entries
                    .remove(0),
            ],
            ..Default::default()
        }))
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
        assert!(app.site_map_ui.stored.page.entries.is_empty());
        assert!(app.site_map_ui.stored.detail.is_none());
        assert!(app.selected.is_none());
        capture(app.project.handle().unwrap(), 1, 1);
        app.site_map_ui.stored.refreshed = None;
        settle(&mut app, &ctx);
        assert_eq!(app.site_map_ui.stored.page.children[0].count, 1);
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
        let page = index.query(&query(), &AtomicBool::new(false)).unwrap();
        eprintln!(
            "25,003 retained endpoints: index {built:?}, first query {:?}, rows {}, child nodes {}",
            start.elapsed() - built,
            page.entries.len(),
            page.children.len()
        );
        assert_eq!(page.total, 25_003);
        assert_eq!(page.entries.len(), PAGE);
        assert_eq!(page.children.len(), PAGE);
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
        click(&mut app, &ctx, "▸ example.test (25003)");
        click(&mut app, &ctx, "Next contents");
        assert_eq!(app.site_map_ui.stored.page.entries[0].id, 501);
        click(&mut app, &ctx, "First contents");
        click(&mut app, &ctx, "· 000001 (1)");
        assert_eq!(app.selected, Some(1));
        assert!(app.site_map_ui.stored.item(1).is_some());
        app.send_selected_to_repeater();
        assert_eq!(app.repeater_tabs.len(), 1);
        assert!(app.history.len() <= 10 && app.history.get(1).is_none());
        eprintln!(
            "Reopened 25,003-record project: headless tree/Contents clicks, paging, old details, and Repeater send passed; live cache {} records.",
            app.history.len()
        );
        drop(app);
        std::fs::remove_file(path).unwrap();
    }
}
