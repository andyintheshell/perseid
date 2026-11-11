//! Disposable disk-backed, ordinal-addressable views. Scrolling reads a bounded
//! range rather than rescanning history or retaining all decoded summaries.
use super::*;

const SORTED: TableDefinition<&[u8], &[u8]> = TableDefinition::new("sorted");
const ROWS: TableDefinition<u64, &[u8]> = TableDefinition::new("rows");
const NODES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("nodes");
const TREE: TableDefinition<u64, &[u8]> = TableDefinition::new("tree");

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct TreeRow {
    pub id: SiteNodeId,
    pub label: String,
    pub scheme: String,
    pub depth: usize,
    pub count: usize,
    pub branch: bool,
}

pub(super) struct View {
    index: Arc<Index>,
    // Expansion-only updates share the ordered Contents and node index.
    tree_index: Option<Arc<Index>>,
    pub total: usize,
    pub tree_total: usize,
    pub selected: Option<(u64, usize)>,
}

// NUL-escaped UTF-8 preserves Rust string ordering, including prefix strings.
fn text(key: &mut Vec<u8>, value: &str) {
    for byte in value.bytes() {
        key.extend_from_slice(&[1, byte]);
    }
    key.push(0);
}

pub(super) fn sort_key(query: &Query, entry: &SiteEntry) -> Vec<u8> {
    let mut key = Vec::new();
    match query.sort {
        SiteMapSort::Host => {
            text(&mut key, &entry.host.to_ascii_lowercase());
            text(&mut key, &entry.path);
        }
        SiteMapSort::Method => text(&mut key, &entry.method),
        SiteMapSort::Url => text(&mut key, &entry.path),
        SiteMapSort::Params => key.extend_from_slice(&(entry.params as u64).to_be_bytes()),
        SiteMapSort::Status => {
            key.extend_from_slice(&entry.status.unwrap_or(u16::MAX).to_be_bytes())
        }
        SiteMapSort::Length => key.extend_from_slice(&entry.length.to_be_bytes()),
        SiteMapSort::Mime => text(&mut key, &entry.mime_label().to_ascii_lowercase()),
        SiteMapSort::Title => text(&mut key, &entry.title.to_ascii_lowercase()),
        SiteMapSort::Notes => text(&mut key, &query.row(entry).notes.to_ascii_lowercase()),
        SiteMapSort::Time => {
            let nanos = match entry.timestamp.duration_since(SystemTime::UNIX_EPOCH) {
                Ok(d) => d.as_nanos() as i128,
                Err(e) => -(e.duration().as_nanos() as i128),
            };
            key.extend_from_slice(&((nanos as u128) ^ (1 << 127)).to_be_bytes());
            key.extend_from_slice(&entry.id.to_be_bytes());
        }
    }
    if !query.ascending {
        for byte in &mut key {
            *byte = !*byte;
        }
    }
    text(&mut key, &entry.path);
    text(&mut key, &entry.host);
    text(&mut key, &entry.method);
    key
}

fn node_key(id: &SiteNodeId) -> Vec<u8> {
    let mut key = Vec::new();
    text(&mut key, id.host());
    if let SiteNodeId::Directory { path, .. } | SiteNodeId::Endpoint { path, .. } = id {
        // Root is a real branch, as in the ordinary URL tree.
        text(&mut key, "");
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            text(&mut key, segment);
        }
    }
    key
}

fn ancestors(entry: &SiteEntry) -> Vec<TreeRow> {
    let host = entry.host.clone();
    let mut nodes = vec![
        TreeRow {
            id: SiteNodeId::Host { host: host.clone() },
            label: host.clone(),
            scheme: String::new(),
            depth: 0,
            count: 0,
            branch: true,
        },
        TreeRow {
            id: SiteNodeId::Directory {
                host: host.clone(),
                path: "/".into(),
            },
            label: "/".into(),
            scheme: String::new(),
            depth: 1,
            count: 0,
            branch: true,
        },
    ];
    let base = entry.base_path();
    let segments: Vec<_> = base.split('/').filter(|s| !s.is_empty()).collect();
    let mut path = String::new();
    for (i, segment) in segments.iter().enumerate() {
        path.push('/');
        path.push_str(segment);
        let branch = i + 1 < segments.len() || base.ends_with('/');
        nodes.push(TreeRow {
            id: if branch {
                SiteNodeId::Directory {
                    host: host.clone(),
                    path: path.clone(),
                }
            } else {
                SiteNodeId::Endpoint {
                    host: host.clone(),
                    path: path.clone(),
                }
            },
            label: (*segment).into(),
            scheme: String::new(),
            depth: i + 2,
            count: 0,
            branch,
        });
    }
    nodes
}

impl View {
    #[cfg(test)]
    pub fn build(
        source: &Index,
        query: &Query,
        open: &BTreeMap<String, bool>,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        Self::build_selected(source, query, open, cancel, None)
    }

    pub fn build_selected(
        source: &Index,
        query: &Query,
        open: &BTreeMap<String, bool>,
        cancel: &AtomicBool,
        selected_id: Option<u64>,
    ) -> Result<Self> {
        let index = Index::empty()?;
        let read = source.db.as_ref().unwrap().begin_read()?;
        let entries = read.open_table(ENTRIES)?;
        let schemes = read.open_table(HOST_SCHEMES)?;
        let db = index.db.as_ref().unwrap();
        let mut write = db.begin_write()?;
        write.set_durability(redb::Durability::None)?;
        write.open_table(SORTED)?;
        write.open_table(NODES)?;
        for (i, value) in entries.iter()?.enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(ProjectError::ServiceUnavailable);
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
            if allowed || !query.filter.hide_empty_folders {
                let mut table = write.open_table(NODES)?;
                for mut node in ancestors(&entry) {
                    if !allowed && !node.branch {
                        continue;
                    }
                    let key = node_key(&node.id);
                    if let Some(old) = table.get(key.as_slice())? {
                        let old: TreeRow = serde_json::from_slice(old.value())?;
                        node.count = old.count;
                        if old.branch {
                            node.branch = true;
                            node.id = old.id;
                        }
                    }
                    node.count += usize::from(allowed);
                    if node.depth == 0 {
                        let set = schemes
                            .get(entry.host.as_str())?
                            .map(|s| serde_json::from_slice::<BTreeSet<String>>(s.value()))
                            .transpose()?
                            .unwrap_or_default();
                        node.scheme = if set.len() == 1 {
                            set.first().unwrap().clone()
                        } else {
                            "mixed".into()
                        };
                    }
                    let bytes = serde_json::to_vec(&node)?;
                    table.insert(key.as_slice(), bytes.as_slice())?;
                }
            }
            if allowed
                && query.node.as_ref().is_some_and(|n| n.contains(&entry))
                && (query.search.is_empty() || site_row_matches(&query.row(&entry), &query.search))
            {
                let key = sort_key(query, &entry);
                write
                    .open_table(SORTED)?
                    .insert(key.as_slice(), value.value())?;
            }
            if i % 256 == 255 {
                write.commit()?;
                write = db.begin_write()?;
                write.set_durability(redb::Durability::None)?;
            }
        }
        write.commit()?;
        let read = db.begin_read()?;
        let mut write = db.begin_write()?;
        write.set_durability(redb::Durability::None)?;
        let mut total = 0;
        let mut selected = None;
        write.open_table(ROWS)?;
        {
            for value in read.open_table(SORTED)?.iter()? {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ProjectError::ServiceUnavailable);
                }
                let (_, value) = value?;
                if let Some(id) = selected_id {
                    let entry: SiteEntry = serde_json::from_slice(value.value())?;
                    if entry.id == id {
                        selected = Some((id, total));
                    }
                }
                write
                    .open_table(ROWS)?
                    .insert(total as u64, value.value())?;
                total += 1;
                if total % 256 == 0 {
                    write.commit()?;
                    write = db.begin_write()?;
                    write.set_durability(redb::Durability::None)?;
                }
            }
        }
        let mut tree_total = 0;
        write.open_table(TREE)?;
        {
            let mut visible_ancestors: Vec<bool> = Vec::new();
            for value in read.open_table(NODES)?.iter()? {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ProjectError::ServiceUnavailable);
                }
                let (_, value) = value?;
                let node: TreeRow = serde_json::from_slice(value.value())?;
                visible_ancestors.truncate(node.depth);
                let visible = visible_ancestors.iter().all(|v| *v);
                let expanded = open.get(&node.id.key()).copied().unwrap_or(node.depth <= 1);
                visible_ancestors.push(visible && expanded);
                if visible {
                    write
                        .open_table(TREE)?
                        .insert(tree_total as u64, value.value())?;
                    tree_total += 1;
                    if tree_total % 256 == 0 {
                        write.commit()?;
                        write = db.begin_write()?;
                        write.set_durability(redb::Durability::None)?;
                    }
                }
            }
        }
        write.commit()?;
        Ok(Self {
            index: Arc::new(index),
            tree_index: None,
            total,
            tree_total,
            selected,
        })
    }

    pub fn with_open(&self, open: &BTreeMap<String, bool>, cancel: &AtomicBool) -> Result<Self> {
        let tree_index = Index::empty()?;
        let db = tree_index.db.as_ref().unwrap();
        let read = self.index.db.as_ref().unwrap().begin_read()?;
        let mut write = db.begin_write()?;
        write.set_durability(redb::Durability::None)?;
        write.open_table(TREE)?;
        let mut tree_total = 0;
        let mut visible_ancestors = Vec::new();
        for value in read.open_table(NODES)?.iter()? {
            if cancel.load(Ordering::Relaxed) {
                return Err(ProjectError::ServiceUnavailable);
            }
            let (_, value) = value?;
            let node: TreeRow = serde_json::from_slice(value.value())?;
            visible_ancestors.truncate(node.depth);
            let visible = visible_ancestors.iter().all(|v| *v);
            visible_ancestors
                .push(visible && open.get(&node.id.key()).copied().unwrap_or(node.depth <= 1));
            if visible {
                write
                    .open_table(TREE)?
                    .insert(tree_total as u64, value.value())?;
                tree_total += 1;
                if tree_total % 256 == 0 {
                    write.commit()?;
                    write = db.begin_write()?;
                    write.set_durability(redb::Durability::None)?;
                }
            }
        }
        write.commit()?;
        Ok(Self {
            index: self.index.clone(),
            tree_index: Some(Arc::new(tree_index)),
            total: self.total,
            tree_total,
            selected: self.selected,
        })
    }

    pub fn window(&self, row: usize, tree: usize) -> Result<Window> {
        let read = self.index.db.as_ref().unwrap().begin_read()?;
        let mut entries = Vec::new();
        for value in read
            .open_table(ROWS)?
            .range(row as u64..(row + PAGE) as u64)?
        {
            entries.push(serde_json::from_slice(value?.1.value())?);
        }
        let mut nodes = Vec::new();
        let tree_read = self
            .tree_index
            .as_ref()
            .unwrap_or(&self.index)
            .db
            .as_ref()
            .unwrap()
            .begin_read()?;
        for value in tree_read
            .open_table(TREE)?
            .range(tree as u64..(tree + PAGE) as u64)?
        {
            nodes.push(serde_json::from_slice(value?.1.value())?);
        }
        Ok(Window {
            row,
            tree,
            entries,
            nodes,
        })
    }
}

#[derive(Default)]
pub(super) struct Window {
    pub row: usize,
    pub tree: usize,
    pub entries: Vec<SiteEntry>,
    pub nodes: Vec<TreeRow>,
}
