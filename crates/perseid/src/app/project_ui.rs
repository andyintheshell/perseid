//! Project file commands and the workspace state stored with a project.
//!
//! Perseid starts with a temporary in-memory project. `File > Save Project As`
//! moves it into a `.perseid` file; `Open` switches to an existing one. The
//! dialogs take a path directly instead of depending on a platform file picker.
//!
//! Workbench state (scope, match/replace rules, Repeater tabs, Intruder
//! configuration, Decoder panels and Comparer items) is saved with the project
//! and restored on open. Captured traffic and workbench state share one writer
//! queue, so a save always includes captures applied before it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Align2, RichText};

use crate::project::{
    CommitReceipt, OpenedProject, ProjectError, ProjectHandle, ProjectMode, ProjectStatus,
    RuleDirectionRecord, RulePartRecord, RuleWorkspace, ScopeWorkspace, WorkspaceState,
};
use crate::proxy::{MatchReplaceRule, RuleDirection, RulePart};

use super::*;

/// How often the unchanged-workstation check runs. Edits are still flushed on
/// Save and on exit; this only bounds how much a crash can lose.
const WORKSPACE_SYNC_INTERVAL: Duration = Duration::from_secs(1);
/// How long a command result stays visible in the status bar.
const NOTICE_LIFETIME: Duration = Duration::from_secs(8);
const PROJECT_EXTENSION: &str = "perseid";
const TEMPORARY_LABEL: &str = "Untitled (temporary)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectDialogKind {
    New,
    Open,
    SaveAs,
    SaveCopy,
}

impl ProjectDialogKind {
    fn title(self) -> &'static str {
        match self {
            Self::New => "New Project",
            Self::Open => "Open Project",
            Self::SaveAs => "Save Project As",
            Self::SaveCopy => "Save Project Copy",
        }
    }

    fn submit_label(self) -> &'static str {
        match self {
            Self::New => "New Project",
            Self::Open => "Open",
            Self::SaveAs => "Save",
            Self::SaveCopy => "Save Copy",
        }
    }

    fn needs_path(self) -> bool {
        !matches!(self, Self::New)
    }
}

struct ProjectDialog {
    kind: ProjectDialogKind,
    path: String,
    error: Option<String>,
}

struct ProjectNotice {
    text: String,
    error: bool,
    at: Instant,
}

enum PendingProject {
    Save {
        copy: bool,
        path: PathBuf,
        receiver: Receiver<std::result::Result<(), ProjectError>>,
    },
    Open {
        path: PathBuf,
        receiver: Receiver<std::result::Result<OpenedProject, ProjectError>>,
        restart_proxy: bool,
    },
    New {
        receiver: Receiver<std::result::Result<OpenedProject, ProjectError>>,
        restart_proxy: bool,
    },
    ClearHttp {
        receiver: Receiver<std::result::Result<CommitReceipt, ProjectError>>,
    },
    ClearWs {
        receiver: Receiver<std::result::Result<CommitReceipt, ProjectError>>,
    },
}

impl PendingProject {
    fn is_file_operation(&self) -> bool {
        matches!(
            self,
            Self::Save { .. } | Self::Open { .. } | Self::New { .. }
        )
    }
}

enum ProjectCompletion {
    Saved {
        copy: bool,
        path: PathBuf,
        result: std::result::Result<(), ProjectError>,
    },
    Opened {
        path: PathBuf,
        result: std::result::Result<OpenedProject, ProjectError>,
        restart_proxy: bool,
    },
    NewProject {
        result: std::result::Result<OpenedProject, ProjectError>,
        restart_proxy: bool,
    },
    ClearedHttp(std::result::Result<CommitReceipt, ProjectError>),
    ClearedWs(std::result::Result<CommitReceipt, ProjectError>),
}

enum ProjectDialogAction {
    Submit(ProjectDialogKind, Option<PathBuf>),
}

/// UI state for the current project.
pub(super) struct ProjectUi {
    handle: Option<ProjectHandle>,
    dialog: Option<ProjectDialog>,
    pending: Vec<PendingProject>,
    notice: Option<ProjectNotice>,
    last_workspace: Option<WorkspaceState>,
    last_workspace_check: Instant,
    ui_limit: usize,
    ui_bytes: usize,
    /// Documentation screenshots show the File menu open without input.
    #[cfg(feature = "screenshots")]
    pub(super) file_menu_forced_open: bool,
}

impl ProjectUi {
    pub(super) fn new(ui_limit: usize, ui_bytes: usize) -> Self {
        let handle = match ProjectHandle::temporary() {
            Ok(handle) => Some(handle),
            Err(err) => {
                log::error!("could not start the temporary project: {err}");
                None
            }
        };
        let mut ui = Self {
            handle,
            dialog: None,
            pending: Vec::new(),
            notice: None,
            last_workspace: None,
            last_workspace_check: Instant::now(),
            ui_limit,
            ui_bytes,
            #[cfg(feature = "screenshots")]
            file_menu_forced_open: false,
        };
        if ui.handle.is_none() {
            ui.notify_error("Persistence is unavailable for this session.");
        }
        ui
    }

    pub(super) fn handle(&self) -> Option<&ProjectHandle> {
        self.handle.as_ref()
    }

    pub(super) fn status(&self) -> Option<ProjectStatus> {
        self.handle.as_ref().map(ProjectHandle::status)
    }

    pub(super) fn is_busy(&self) -> bool {
        self.pending.iter().any(PendingProject::is_file_operation)
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(ProjectNotice {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    fn notify_error(&mut self, text: impl Into<String>) {
        self.notice = Some(ProjectNotice {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    pub(super) fn clear_http_history(&mut self) {
        let Some(handle) = self.handle() else {
            return;
        };
        let receiver = handle.clear_http();
        self.pending.push(PendingProject::ClearHttp { receiver });
    }

    pub(super) fn clear_ws_history(&mut self) {
        let Some(handle) = self.handle() else {
            return;
        };
        let receiver = handle.clear_ws();
        self.pending.push(PendingProject::ClearWs { receiver });
    }

    fn notice(&self) -> Option<(&str, bool)> {
        let notice = self.notice.as_ref()?;
        if notice.at.elapsed() > NOTICE_LIFETIME {
            return None;
        }
        Some((&notice.text, notice.error))
    }

    fn open_dialog(&mut self, kind: ProjectDialogKind, current: Option<&Path>) {
        let path = if kind.needs_path() {
            default_project_path(current, kind == ProjectDialogKind::SaveCopy)
        } else {
            String::new()
        };
        self.dialog = Some(ProjectDialog {
            kind,
            path,
            error: None,
        });
    }

    fn dialog(&mut self, ctx: &egui::Context) -> Option<ProjectDialogAction> {
        let dialog = self.dialog.as_ref()?;
        let kind = dialog.kind;
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        let response = egui::Window::new(kind.title())
            .id(egui::Id::unique("perseid.project.dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                let dialog = self.dialog.as_mut().expect("dialog is still open");
                ui.set_min_width(420.0);
                match kind {
                    ProjectDialogKind::New => {
                        ui.label(
                            "Discard the current project and start with an empty temporary project?",
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Nothing is deleted from disk. Save the current project first if you want to keep it.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }
                    ProjectDialogKind::Open => {
                        ui.label("Path to an existing .perseid project:");
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .desired_width(f32::INFINITY)
                                .hint_text("/path/to/project.perseid"),
                        );
                        submit |= edit.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    }
                    ProjectDialogKind::SaveAs | ProjectDialogKind::SaveCopy => {
                        ui.label(if kind == ProjectDialogKind::SaveAs {
                            "Save the current project to a new file:"
                        } else {
                            "Write a copy without switching the open project:"
                        });
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .desired_width(f32::INFINITY)
                                .hint_text("/path/to/project.perseid"),
                        );
                        submit |= edit.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new("The destination must not already exist.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                }
                if let Some(error) = &dialog.error {
                    ui.add_space(3.0);
                    ui.label(RichText::new(error).small().color(theme::RED));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(kind.submit_label()).clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        let _ = response;

        if submit {
            let dialog = self.dialog.as_mut()?;
            if dialog.kind.needs_path() {
                let trimmed = dialog.path.trim();
                if trimmed.is_empty() {
                    dialog.error = Some("Enter a project file path.".to_string());
                    return None;
                }
                let path = normalize_project_path(trimmed);
                let kind = dialog.kind;
                self.dialog = None;
                return Some(ProjectDialogAction::Submit(kind, Some(path)));
            }
            let kind = dialog.kind;
            self.dialog = None;
            return Some(ProjectDialogAction::Submit(kind, None));
        }
        if !open || cancel {
            self.dialog = None;
        }
        None
    }

    fn poll(&mut self) -> Vec<ProjectCompletion> {
        let mut completed = Vec::new();
        let mut keep = Vec::with_capacity(self.pending.len());
        for pending in self.pending.drain(..) {
            match pending {
                PendingProject::Save {
                    copy,
                    path,
                    receiver,
                } => match receiver.try_recv() {
                    Ok(result) => completed.push(ProjectCompletion::Saved { copy, path, result }),
                    Err(TryRecvError::Empty) => {
                        keep.push(PendingProject::Save {
                            copy,
                            path,
                            receiver,
                        });
                    }
                    Err(TryRecvError::Disconnected) => {
                        completed.push(ProjectCompletion::Saved {
                            copy,
                            path,
                            result: Err(ProjectError::ServiceUnavailable),
                        });
                    }
                },
                PendingProject::Open {
                    path,
                    receiver,
                    restart_proxy,
                } => match receiver.try_recv() {
                    Ok(result) => completed.push(ProjectCompletion::Opened {
                        path,
                        result,
                        restart_proxy,
                    }),
                    Err(TryRecvError::Empty) => {
                        keep.push(PendingProject::Open {
                            path,
                            receiver,
                            restart_proxy,
                        });
                    }
                    Err(TryRecvError::Disconnected) => {
                        completed.push(ProjectCompletion::Opened {
                            path,
                            result: Err(ProjectError::ServiceUnavailable),
                            restart_proxy,
                        });
                    }
                },
                PendingProject::New {
                    receiver,
                    restart_proxy,
                } => match receiver.try_recv() {
                    Ok(result) => completed.push(ProjectCompletion::NewProject {
                        result,
                        restart_proxy,
                    }),
                    Err(TryRecvError::Empty) => {
                        keep.push(PendingProject::New {
                            receiver,
                            restart_proxy,
                        });
                    }
                    Err(TryRecvError::Disconnected) => {
                        completed.push(ProjectCompletion::NewProject {
                            result: Err(ProjectError::ServiceUnavailable),
                            restart_proxy,
                        });
                    }
                },
                PendingProject::ClearHttp { receiver } => match receiver.try_recv() {
                    Ok(result) => completed.push(ProjectCompletion::ClearedHttp(result)),
                    Err(TryRecvError::Empty) => keep.push(PendingProject::ClearHttp { receiver }),
                    Err(TryRecvError::Disconnected) => completed.push(
                        ProjectCompletion::ClearedHttp(Err(ProjectError::ServiceUnavailable)),
                    ),
                },
                PendingProject::ClearWs { receiver } => match receiver.try_recv() {
                    Ok(result) => completed.push(ProjectCompletion::ClearedWs(result)),
                    Err(TryRecvError::Empty) => keep.push(PendingProject::ClearWs { receiver }),
                    Err(TryRecvError::Disconnected) => completed.push(
                        ProjectCompletion::ClearedWs(Err(ProjectError::ServiceUnavailable)),
                    ),
                },
            }
        }
        self.pending = keep;
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.at.elapsed() > NOTICE_LIFETIME)
        {
            self.notice = None;
        }
        completed
    }
}

fn normalize_project_path(input: &str) -> PathBuf {
    let path = PathBuf::from(input);
    if path.extension().is_none() {
        path.with_extension(PROJECT_EXTENSION)
    } else {
        path
    }
}

fn default_project_path(current: Option<&Path>, copy: bool) -> String {
    if let Some(current) = current.filter(|_| !copy) {
        return current.display().to_string();
    }
    let fallback = if copy {
        "perseid-project-copy.perseid"
    } else {
        "perseid-project.perseid"
    };
    if let Some(current) = current
        && let Some(stem) = current.file_stem().and_then(|stem| stem.to_str())
    {
        let name = format!("{stem}-copy.perseid");
        if let Some(parent) = current.parent() {
            return parent.join(name).display().to_string();
        }
    }
    std::env::current_dir()
        .unwrap_or_default()
        .join(fallback)
        .display()
        .to_string()
}

fn rule_workspace(rule: &MatchReplaceRule) -> RuleWorkspace {
    RuleWorkspace {
        enabled: rule.enabled,
        direction: match rule.direction {
            RuleDirection::Request => RuleDirectionRecord::Request,
            RuleDirection::Response => RuleDirectionRecord::Response,
        },
        part: match rule.part {
            RulePart::Head => RulePartRecord::Head,
            RulePart::Body => RulePartRecord::Body,
            RulePart::All => RulePartRecord::All,
        },
        pattern: rule.pattern.clone(),
        replacement: rule.replacement.clone(),
        regex: rule.regex,
    }
}

fn rule_from_workspace(rule: &RuleWorkspace) -> MatchReplaceRule {
    MatchReplaceRule {
        enabled: rule.enabled,
        direction: match rule.direction {
            RuleDirectionRecord::Request => RuleDirection::Request,
            RuleDirectionRecord::Response => RuleDirection::Response,
        },
        part: match rule.part {
            RulePartRecord::Head => RulePart::Head,
            RulePartRecord::Body => RulePart::Body,
            RulePartRecord::All => RulePart::All,
        },
        pattern: rule.pattern.clone(),
        replacement: rule.replacement.clone(),
        regex: rule.regex,
    }
}

impl PerseidApp {
    /// The File menu: project lifecycle commands live above Quit.
    pub(super) fn project_menu(&mut self, ui: &mut egui::Ui) {
        let response = ui.menu_button("File", |ui| {
            if ui.button("New Project").clicked() {
                self.project.open_dialog(ProjectDialogKind::New, None);
                ui.close();
            }
            if ui.button("Open Project…").clicked() {
                let current = self.current_project_path();
                self.project
                    .open_dialog(ProjectDialogKind::Open, current.as_deref());
                ui.close();
            }
            ui.separator();
            if ui.button("Save Project As…").clicked() {
                let current = self.current_project_path();
                self.project
                    .open_dialog(ProjectDialogKind::SaveAs, current.as_deref());
                ui.close();
            }
            if ui.button("Save Project Copy…").clicked() {
                let current = self.current_project_path();
                self.project
                    .open_dialog(ProjectDialogKind::SaveCopy, current.as_deref());
                ui.close();
            }
        });
        #[cfg(feature = "screenshots")]
        if self.project.file_menu_forced_open {
            egui::Popup::open_id(ui.ctx(), response.response.id.with("popup"));
        }
        let _ = response;
    }

    fn current_project_path(&self) -> Option<PathBuf> {
        self.project
            .status()
            .and_then(|status| status.mode.path().map(Path::to_path_buf))
    }

    fn notify_project_busy(&mut self) {
        self.project
            .notify_error("Wait for the current project operation to finish.");
    }

    fn begin_save_project(&mut self, path: PathBuf, copy: bool) {
        if self.project.is_busy() {
            self.notify_project_busy();
            return;
        }
        // Queue the latest workbench state before the save command. Both use
        // the same writer mailbox, so the snapshot includes it.
        self.sync_workspace(true);
        let Some(handle) = self.project.handle() else {
            self.project
                .notify_error("Persistence is unavailable for this session.");
            return;
        };
        let receiver = if copy {
            handle.save_copy(path.clone())
        } else {
            handle.save_as(path.clone())
        };
        self.project.pending.push(PendingProject::Save {
            copy,
            path,
            receiver,
        });
    }

    fn stop_proxy_for_project_switch(&mut self) -> bool {
        let running = self.proxy.take().is_some();
        if running {
            self.clear_intercept_queue();
            self.proxy_error = None;
        }
        running
    }

    fn begin_open_project(&mut self, path: PathBuf) {
        if self.project.is_busy() {
            self.notify_project_busy();
            return;
        }
        self.sync_workspace(true);
        let restart_proxy = self.stop_proxy_for_project_switch();
        let Some(handle) = self.project.handle() else {
            self.project
                .notify_error("Persistence is unavailable for this session.");
            return;
        };
        let receiver = handle.open(path.clone(), self.project.ui_limit, self.project.ui_bytes);
        self.project.notify(format!("Opening {}…", path.display()));
        self.project.pending.push(PendingProject::Open {
            path,
            receiver,
            restart_proxy,
        });
    }

    fn begin_new_project(&mut self) {
        if self.project.is_busy() {
            self.notify_project_busy();
            return;
        }
        self.sync_workspace(true);
        let restart_proxy = self.stop_proxy_for_project_switch();
        let Some(handle) = self.project.handle() else {
            self.project
                .notify_error("Persistence is unavailable for this session.");
            return;
        };
        let receiver = handle.new_project(self.project.ui_limit, self.project.ui_bytes);
        self.project.pending.push(PendingProject::New {
            receiver,
            restart_proxy,
        });
    }

    pub(super) fn poll_project(&mut self) {
        for completion in self.project.poll() {
            match completion {
                ProjectCompletion::Saved { copy, path, result } => match result {
                    Ok(()) => {
                        self.project.last_workspace = Some(self.workspace_snapshot());
                        self.project.notify(if copy {
                            format!("Saved a copy to {}", path.display())
                        } else {
                            format!("Saved project to {}", path.display())
                        });
                    }
                    Err(err) => {
                        self.project
                            .notify_error(format!("Could not save the project: {err}"));
                    }
                },
                ProjectCompletion::Opened {
                    path,
                    result,
                    restart_proxy,
                } => {
                    match result {
                        Ok(opened) => self.apply_opened_project(opened, Some(&path)),
                        Err(err) => {
                            self.project
                                .notify_error(format!("Could not open the project: {err}"));
                        }
                    }
                    if restart_proxy {
                        let notify = self.notify.clone();
                        self.start_proxy(notify);
                    }
                }
                ProjectCompletion::NewProject {
                    result,
                    restart_proxy,
                } => {
                    match result {
                        Ok(opened) => self.apply_opened_project(opened, None),
                        Err(err) => {
                            self.project
                                .notify_error(format!("Could not create a project: {err}"));
                        }
                    }
                    if restart_proxy {
                        let notify = self.notify.clone();
                        self.start_proxy(notify);
                    }
                }
                ProjectCompletion::ClearedHttp(result) => {
                    if let Err(err) = result {
                        self.project
                            .notify_error(format!("Could not clear the project history: {err}"));
                    }
                }
                ProjectCompletion::ClearedWs(result) => {
                    if let Err(err) = result {
                        self.project.notify_error(format!(
                            "Could not clear the stored WebSocket history: {err}"
                        ));
                    }
                }
            }
        }
    }

    fn apply_opened_project(&mut self, opened: OpenedProject, path: Option<&Path>) {
        let http_total = opened.http_total;
        let ws_total = opened.ws_total;
        let http_loaded = opened.http.len() as u64;
        let ws_loaded = opened.ws.len() as u64;

        self.history.load(opened.http);
        self.websocket_history.load(opened.ws);
        self.selected = None;
        self.detail = None;
        self.websocket_selected = None;
        self.websocket_detail = None;
        self.scroll_to_selected = false;
        self.websocket_scroll_to_selected = false;
        self.inspector.clear_notes();
        self.search.clear_history();
        self.invalidate_views();
        self.apply_workspace(&opened.workspace);
        self.project.last_workspace = Some(self.workspace_snapshot());
        self.project.last_workspace_check = Instant::now();

        let truncated = http_loaded < http_total || ws_loaded < ws_total;
        if truncated {
            self.project.notify(format!(
                "Loaded the newest {} of {http_total} exchanges and {ws_loaded} of {ws_total} WebSocket messages; older records remain in the project but are not shown yet.",
                http_loaded
            ));
        } else if let Some(path) = path {
            self.project.notify(format!("Opened {}", path.display()));
        } else {
            self.project.notify("Started a new temporary project.");
        }
    }

    fn workspace_snapshot(&self) -> WorkspaceState {
        WorkspaceState {
            version: 1,
            scope: ScopeWorkspace {
                enabled: self.scope_enabled,
                hosts: self.scope.iter().cloned().collect(),
            },
            match_replace: self.match_replace.iter().map(rule_workspace).collect(),
            repeater: self.repeater_workspace(),
            intruder: self.intruder_workspace(),
            decoder: self.decoder_workspace(),
            comparer: self.comparer_workspace(),
        }
    }

    fn apply_workspace(&mut self, workspace: &WorkspaceState) {
        self.scope_enabled = workspace.scope.enabled;
        self.scope = workspace.scope.hosts.iter().cloned().collect();
        self.scope_input.clear();
        self.match_replace = workspace
            .match_replace
            .iter()
            .map(rule_from_workspace)
            .collect();
        self.rules_error = None;
        self.apply_proxy_settings();
        self.apply_repeater_workspace(&workspace.repeater);
        self.apply_intruder_workspace(&workspace.intruder);
        self.apply_decoder_workspace(&workspace.decoder);
        self.apply_comparer_workspace(&workspace.comparer);
        self.invalidate_views();
        self.site_map = None;
    }

    /// Persist the workbench state when it changed. `force` bypasses the
    /// periodic check but still skips an identical snapshot.
    pub(super) fn sync_workspace(&mut self, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.project.last_workspace_check) < WORKSPACE_SYNC_INTERVAL
        {
            return;
        }
        self.project.last_workspace_check = now;
        let workspace = self.workspace_snapshot();
        if self.project.last_workspace.as_ref() == Some(&workspace) {
            return;
        }
        let Some(handle) = self.project.handle() else {
            return;
        };
        if handle.set_workspace(workspace.clone()) {
            self.project.last_workspace = Some(workspace);
        }
    }

    /// Stop the writer and make the final workspace durable on exit.
    pub(super) fn shutdown_project(&mut self) {
        self.sync_workspace(true);
        if let Some(handle) = self.project.handle() {
            let receiver = handle.flush();
            let _ = receiver.recv_timeout(Duration::from_secs(10));
        }
    }

    /// Poll project commands and draw their dialogs. Called once per frame.
    pub(super) fn project_windows(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.project.dialog(ctx) {
            match action {
                ProjectDialogAction::Submit(kind, path) => match (kind, path) {
                    (ProjectDialogKind::New, _) => self.begin_new_project(),
                    (ProjectDialogKind::Open, Some(path)) => self.begin_open_project(path),
                    (ProjectDialogKind::SaveAs, Some(path)) => {
                        self.begin_save_project(path, false);
                    }
                    (ProjectDialogKind::SaveCopy, Some(path)) => {
                        self.begin_save_project(path, true);
                    }
                    (_, None) => {}
                },
            }
        }
        self.sync_workspace(false);
    }

    /// Project identity, save state, and the most recent command result.
    pub(super) fn project_status_widget(&mut self, ui: &mut egui::Ui) {
        let status = self.project.status();
        let busy = self.project.is_busy();
        let notice = self
            .project
            .notice()
            .map(|(text, error)| (text.to_string(), error));

        ui.separator();
        match status.as_ref().map(|status| &status.mode) {
            Some(ProjectMode::Persistent { path }) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                ui.label(RichText::new(name).small().color(theme::TEXT_MUTED))
                    .on_hover_text(format!("Project: {}", path.display()));
            }
            Some(ProjectMode::Temporary) => {
                ui.label(
                    RichText::new(TEMPORARY_LABEL)
                        .small()
                        .color(theme::TEXT_MUTED),
                )
                .on_hover_text(
                    "Captures live in memory only. Use File > Save Project As… to keep them.",
                );
            }
            None => {
                ui.colored_label(theme::RED, "project unavailable");
            }
        }

        if busy {
            ui.colored_label(theme::AMBER, "project busy…");
        }

        if let Some(status) = &status {
            if let Some(error) = &status.last_error {
                ui.colored_label(theme::RED, "project error")
                    .on_hover_text(error);
            }
            let loss = status
                .capture_loss_count
                .saturating_add(status.event_loss_count);
            if loss > 0 {
                ui.colored_label(
                    theme::AMBER,
                    format!("{loss} capture records incomplete"),
                )
                .on_hover_text(
                    "Some capture events or terminal responses were lost. The affected rows are marked in the project.",
                );
            }
        }

        if let Some((text, error)) = notice {
            ui.label(RichText::new(text).small().color(if error {
                theme::RED
            } else {
                theme::GREEN
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_paths_get_the_extension() {
        assert_eq!(
            normalize_project_path("/tmp/demo"),
            PathBuf::from("/tmp/demo.perseid")
        );
        assert_eq!(
            normalize_project_path("/tmp/demo.perseid"),
            PathBuf::from("/tmp/demo.perseid")
        );
        assert_eq!(
            normalize_project_path("/tmp/demo.data"),
            PathBuf::from("/tmp/demo.data")
        );
    }

    #[test]
    fn copy_default_uses_the_source_name() {
        let source = Path::new("/tmp/session.perseid");
        let path = default_project_path(Some(source), true);
        // Compare paths, not strings: Windows renders the separator differently.
        assert_eq!(
            PathBuf::from(path),
            source.with_file_name("session-copy.perseid")
        );
    }
}
