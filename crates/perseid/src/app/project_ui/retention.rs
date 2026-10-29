//! Reconcile the bounded live caches after committed storage evictions.

use super::*;

type PrunedIds = std::result::Result<(Vec<u64>, Vec<u64>), ProjectError>;

pub(super) struct RetentionSweep {
    generation: u64,
    evictions: u64,
    receiver: Receiver<PrunedIds>,
}

impl PerseidApp {
    pub(super) fn poll_retention_cache(&mut self) {
        let Some(status) = self.project.status() else {
            return;
        };
        if let Some(sweep) = self.project.retention_sweep.take()
            && sweep.generation == status.generation
        {
            match sweep.receiver.try_recv() {
                Ok(Ok((http, ws))) => {
                    self.history.remove_stored(&http);
                    self.websocket_history.remove_stored(&ws);
                    if self.selected.is_some_and(|id| http.contains(&id)) {
                        self.selected = None;
                        self.detail = None;
                    }
                    if self.websocket_selected.is_some_and(|id| ws.contains(&id)) {
                        self.websocket_selected = None;
                        self.websocket_detail = None;
                    }
                    self.project.retention_seen = (sweep.generation, sweep.evictions);
                    self.invalidate_views();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.project.retention_sweep = Some(sweep);
                }
                result => {
                    // Do not retry a broken store on every frame; a later
                    // eviction or project switch permits a fresh attempt.
                    self.project.retention_seen = (sweep.generation, sweep.evictions);
                    self.project
                        .notify_error(format!("Could not refresh retained history: {result:?}"));
                }
            }
        }
        if self.project.retention_sweep.is_none()
            && status.retention_evictions > 0
            && self.project.retention_seen != (status.generation, status.retention_evictions)
            && let Some(snapshot) = self
                .project
                .handle()
                .and_then(|handle| handle.database_snapshot())
        {
            let http = self.history.iter().map(|item| item.id).collect();
            let ws = self.websocket_history.iter().map(|item| item.id).collect();
            let (sender, receiver) = std::sync::mpsc::channel();
            self.project.retention_sweep = Some(RetentionSweep {
                generation: status.generation,
                evictions: status.retention_evictions,
                receiver,
            });
            std::thread::spawn(move || {
                let _ = sender.send(snapshot.pruned_ids(http, ws));
            });
        }
    }
}
