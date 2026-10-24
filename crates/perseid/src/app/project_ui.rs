//! Project file commands and the workspace state stored with a project.
//!
//! Perseid starts with a temporary in-memory project. `File > Save Project As`
//! moves it into a `.perseid` file; `Open` switches to an existing one. The
//! dialogs take a path directly instead of depending on a platform file picker.
//!
//! Workbench state (scope, match/replace rules, UI selections, Inspector,
//! Repeater, Intruder, Decoder, Comparer, Search, Sequencer, and TLS settings)
//! is saved with the project and restored on open. Captured traffic and
//! workbench state share one writer queue, so a save always includes captures
//! applied before it. The local certificate authority is never part of the
//! workspace.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Align2, RichText};

use crate::project::{
    CommitReceipt, MainTabRecord, OpenedProject, ProjectError, ProjectHandle, ProjectMode,
    ProjectStatus, ProxySectionRecord, RuleDirectionRecord, RulePartRecord, RuleWorkspace,
    ScopeWorkspace, TargetSectionRecord, TlsWorkspace, UiWorkspace, WorkspaceState,
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

    /// Documentation screenshots reset transient project UI between scenes.
    #[cfg(feature = "screenshots")]
    pub(super) fn reset_for_screenshot(&mut self) {
        self.dialog = None;
        self.file_menu_forced_open = false;
    }

    /// Documentation screenshots: show the Save As dialog with a demo path.
    #[cfg(feature = "screenshots")]
    pub(super) fn show_save_dialog_for_screenshot(&mut self) {
        self.open_dialog(
            ProjectDialogKind::SaveAs,
            Some(Path::new("/home/user/assessments/demo-shop.perseid")),
        );
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

fn main_tab_record(tab: MainTab) -> MainTabRecord {
    match tab {
        MainTab::Target => MainTabRecord::Target,
        MainTab::Proxy => MainTabRecord::Proxy,
        MainTab::Intruder => MainTabRecord::Intruder,
        MainTab::Repeater => MainTabRecord::Repeater,
        MainTab::Sequencer => MainTabRecord::Sequencer,
        MainTab::Decoder => MainTabRecord::Decoder,
        MainTab::Comparer => MainTabRecord::Comparer,
    }
}

fn main_tab_from_record(tab: MainTabRecord) -> MainTab {
    match tab {
        MainTabRecord::Target => MainTab::Target,
        MainTabRecord::Proxy => MainTab::Proxy,
        MainTabRecord::Intruder => MainTab::Intruder,
        MainTabRecord::Repeater => MainTab::Repeater,
        MainTabRecord::Sequencer => MainTab::Sequencer,
        MainTabRecord::Decoder => MainTab::Decoder,
        MainTabRecord::Comparer => MainTab::Comparer,
    }
}

fn proxy_section_record(section: ProxySection) -> ProxySectionRecord {
    match section {
        ProxySection::Intercept => ProxySectionRecord::Intercept,
        ProxySection::HttpHistory => ProxySectionRecord::HttpHistory,
        ProxySection::WebSocketsHistory => ProxySectionRecord::WebSocketsHistory,
        ProxySection::MatchReplace => ProxySectionRecord::MatchReplace,
        ProxySection::Tls => ProxySectionRecord::Tls,
    }
}

fn proxy_section_from_record(section: ProxySectionRecord) -> ProxySection {
    match section {
        ProxySectionRecord::Intercept => ProxySection::Intercept,
        ProxySectionRecord::HttpHistory => ProxySection::HttpHistory,
        ProxySectionRecord::WebSocketsHistory => ProxySection::WebSocketsHistory,
        ProxySectionRecord::MatchReplace => ProxySection::MatchReplace,
        ProxySectionRecord::Tls => ProxySection::Tls,
    }
}

fn target_section_record(section: TargetSection) -> TargetSectionRecord {
    match section {
        TargetSection::SiteMap => TargetSectionRecord::SiteMap,
        TargetSection::Scope => TargetSectionRecord::Scope,
    }
}

fn target_section_from_record(section: TargetSectionRecord) -> TargetSection {
    match section {
        TargetSectionRecord::SiteMap => TargetSection::SiteMap,
        TargetSectionRecord::Scope => TargetSection::Scope,
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
        {
            // Keep the menu open only for its own scene. Popup state would
            // otherwise leak into every later screenshot.
            let popup_id = response.response.id.with("popup");
            if self.project.file_menu_forced_open {
                egui::Popup::open_id(ui.ctx(), popup_id);
            } else {
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
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

    fn ui_workspace(&self) -> UiWorkspace {
        UiWorkspace {
            main_tab: main_tab_record(self.main_tab),
            proxy_section: proxy_section_record(self.proxy_section),
            target_section: target_section_record(self.target_section),
            request_view: self.request_view.into(),
            response_view: self.response_view.into(),
            websocket_view: self.websocket_view.into(),
            http_filter: self.filter.clone(),
            websocket_filter: self.websocket_filter.clone(),
            selected_http_id: self.selected,
            selected_websocket_id: self.websocket_selected,
            repeater_active: self.repeater_active,
        }
    }

    fn apply_ui_workspace(&mut self, workspace: &UiWorkspace) {
        self.main_tab = main_tab_from_record(workspace.main_tab);
        self.proxy_section = proxy_section_from_record(workspace.proxy_section);
        self.target_section = target_section_from_record(workspace.target_section);
        self.request_view = workspace.request_view.into();
        self.response_view = workspace.response_view.into();
        self.websocket_view = workspace.websocket_view.into();
        self.filter = workspace.http_filter.clone();
        self.websocket_filter = workspace.websocket_filter.clone();
        self.selected = workspace
            .selected_http_id
            .filter(|id| self.history.get(*id).is_some());
        self.websocket_selected = workspace
            .selected_websocket_id
            .filter(|id| self.websocket_history.get(*id).is_some());
        self.detail = None;
        self.websocket_detail = None;
        self.scroll_to_selected = false;
        self.websocket_scroll_to_selected = false;
    }

    fn workspace_snapshot(&self) -> WorkspaceState {
        WorkspaceState {
            version: WorkspaceState::CURRENT_VERSION,
            scope: ScopeWorkspace {
                enabled: self.scope_enabled,
                enforce: self.scope_enforce,
                hosts: self.scope.iter().cloned().collect(),
            },
            match_replace: self.match_replace.iter().map(rule_workspace).collect(),
            ui: self.ui_workspace(),
            inspector: self.inspector.workspace(),
            repeater: self.repeater_workspace(),
            intruder: self.intruder_workspace(),
            decoder: self.decoder_workspace(),
            comparer: self.comparer_workspace(),
            comparer_ui: self.comparer_ui_workspace(),
            search: self.search.workspace(),
            sequencer: self.sequencer.workspace(),
            tls: TlsWorkspace {
                intercept: self.tls_intercept,
                passthrough: self.tls_passthrough.clone(),
            },
        }
    }

    fn apply_workspace(&mut self, workspace: &WorkspaceState) {
        self.scope_enabled = workspace.scope.enabled;
        self.scope_enforce = workspace.scope.enforce;
        self.scope = workspace.scope.hosts.iter().cloned().collect();
        self.scope_input.clear();
        self.match_replace = workspace
            .match_replace
            .iter()
            .map(rule_from_workspace)
            .collect();
        self.rules_error = None;
        self.tls_intercept = workspace.tls.intercept;
        self.tls_passthrough = workspace.tls.passthrough.clone();
        self.tls_passthrough_input.clear();
        self.apply_ui_workspace(&workspace.ui);
        self.apply_proxy_settings();
        self.apply_repeater_workspace(&workspace.repeater, workspace.ui.repeater_active);
        self.apply_intruder_workspace(&workspace.intruder);
        self.apply_decoder_workspace(&workspace.decoder);
        self.apply_comparer_workspace(&workspace.comparer);
        self.apply_comparer_ui_workspace(&workspace.comparer_ui);
        self.inspector.apply_workspace(&workspace.inspector);
        self.search
            .apply_workspace(&workspace.search, &self.history);
        self.sequencer.apply_workspace(&workspace.sequencer);
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
    use crate::project::{
        BodyWorkspace, CapturePartRecord, CompareModeRecord, ComparerWorkspace, DecoderCodecRecord,
        DecoderColorTagRecord, DecoderOperationRecord, DecoderPanelWorkspace, DecoderSpanWorkspace,
        DecoderViewRecord, InspectorNoteWorkspace, InspectorWorkspace, IntruderAttackRecord,
        IntruderPlanWorkspace, IntruderResultOutcomeWorkspace, IntruderResultWorkspace,
        IntruderWorkspace, MessagePaneRecord, MessageViewRecord, MessageViewsWorkspace,
        RepeaterResponseWorkspace, RepeaterWorkspace, ResultViewRecord, SearchWorkspace,
        SequencerResultTabRecord, SequencerSectionRecord, SequencerSettingsWorkspace,
        SideTabRecord, TextSelectionWorkspace, TlsWorkspace, UiWorkspace,
    };

    fn sample_workspace() -> WorkspaceState {
        let response_views = MessageViewsWorkspace {
            pretty: "HTTP/1.1 200 OK\r\n\r\nbody".to_string(),
            raw: "HTTP/1.1 200 OK\n\nbody".to_string(),
            hex: "48 54 54 50".to_string(),
        };
        WorkspaceState {
            scope: ScopeWorkspace {
                enabled: true,
                enforce: true,
                hosts: vec!["example.test".to_string()],
            },
            match_replace: vec![RuleWorkspace {
                enabled: true,
                direction: RuleDirectionRecord::Response,
                part: RulePartRecord::Body,
                pattern: "before".to_string(),
                replacement: "after".to_string(),
                regex: false,
            }],
            ui: UiWorkspace {
                main_tab: MainTabRecord::Comparer,
                proxy_section: ProxySectionRecord::Tls,
                target_section: TargetSectionRecord::Scope,
                request_view: MessageViewRecord::Hex,
                response_view: MessageViewRecord::Raw,
                websocket_view: MessageViewRecord::Hex,
                http_filter: "status:200".to_string(),
                websocket_filter: "chat".to_string(),
                selected_http_id: None,
                selected_websocket_id: None,
                repeater_active: 0,
            },
            inspector: InspectorWorkspace {
                open: false,
                tab: SideTabRecord::Notes,
                expanded: vec!["request.headers".to_string()],
                notes: vec![InspectorNoteWorkspace {
                    id: 4,
                    text: "check this exchange".to_string(),
                }],
            },
            repeater: vec![RepeaterWorkspace {
                id: 2,
                title: "Login".to_string(),
                scheme: "https".to_string(),
                host: "example.test".to_string(),
                request: "GET / HTTP/1.1\r\nHost: example.test\r\n\r\n".to_string(),
                note: Some("seeded".to_string()),
                response: Some(RepeaterResponseWorkspace {
                    views: response_views,
                    status: 200,
                    reason: "OK".to_string(),
                    duration_ms: 18,
                    size: 4,
                    truncated: false,
                    body_complete: true,
                    body_error: None,
                }),
                error: None,
                response_view: MessageViewRecord::Raw,
            }],
            intruder: IntruderWorkspace {
                scheme: "https".to_string(),
                host: "example.test".to_string(),
                request: "POST /login HTTP/1.1\r\nHost: example.test\r\n\r\nuser=§a§".to_string(),
                attack: IntruderAttackRecord::ClusterBomb,
                payload_sets: vec!["admin\nroot".to_string()],
                active_set: 0,
                concurrency: 8,
                update_host: true,
                note: None,
                selection: Some(TextSelectionWorkspace { start: 0, end: 4 }),
                cursor: 5,
                results: vec![IntruderResultWorkspace {
                    attack_id: 9,
                    sequence: 2,
                    payloads: vec!["admin".to_string()],
                    duration_ms: 9,
                    outcome: IntruderResultOutcomeWorkspace::Response {
                        version: "HTTP/1.1".to_string(),
                        status: 200,
                        reason: "OK".to_string(),
                        headers: vec![("Content-Length".to_string(), "4".to_string())],
                        body: BodyWorkspace {
                            data: b"body".to_vec(),
                            total_len: 4,
                            truncated: false,
                            complete: true,
                        },
                        body_error: None,
                    },
                }],
                selected: Some(2),
                detail_pane: MessagePaneRecord::Request,
                detail_view: MessageViewRecord::Raw,
                attack_id: Some(9),
                last_plan: Some(IntruderPlanWorkspace {
                    attack: IntruderAttackRecord::ClusterBomb,
                    scheme: "https".to_string(),
                    host: "example.test".to_string(),
                    template: "POST /login HTTP/1.1\r\nHost: example.test\r\n\r\nuser=§a§"
                        .to_string(),
                    payload_sets: vec![vec!["admin".to_string(), "root".to_string()]],
                    concurrency: 8,
                    scope: Vec::new(),
                }),
                total: 2,
                received: 1,
                error: None,
                finished_note: Some("1 of 2 completed".to_string()),
            },
            decoder: vec![DecoderPanelWorkspace {
                id: 3,
                data: b"abc".to_vec(),
                spans: vec![DecoderSpanWorkspace {
                    start: 0,
                    end: 3,
                    tag: DecoderColorTagRecord::Codec(DecoderCodecRecord::Url),
                }],
                operation: Some(DecoderOperationRecord::Decode(DecoderCodecRecord::Url)),
                view: DecoderViewRecord::Hex,
                selection: Some(TextSelectionWorkspace { start: 1, end: 3 }),
            }],
            comparer: vec![b"one".to_vec(), b"two".to_vec()],
            comparer_ui: ComparerWorkspace {
                selected: [Some(0), Some(1)],
                active: 1,
                result_open: true,
                result_mode: Some(CompareModeRecord::Words),
                result_items: Some([0, 1]),
                result_views: [ResultViewRecord::Text, ResultViewRecord::Hex],
                sync_views: true,
                diff_cursor: 0,
            },
            search: SearchWorkspace {
                open: true,
                query: "needle".to_string(),
                options: crate::project::SearchOptionsWorkspace {
                    regex: true,
                    negative: true,
                    ..Default::default()
                },
                ran: false,
                dirty: false,
                results: Vec::new(),
                selected: None,
                pane: MessagePaneRecord::Request,
                request_view: MessageViewRecord::Hex,
                response_view: MessageViewRecord::Raw,
                error: None,
            },
            sequencer: crate::project::SequencerWorkspace {
                section: SequencerSectionRecord::Settings,
                tokens: vec!["deadbeef".to_string(), "cafebabe".to_string()],
                settings: SequencerSettingsWorkspace {
                    capture_regex: r"token=(\w+)".to_string(),
                    capture_part: CapturePartRecord::Both,
                    deduplicate: false,
                    ..Default::default()
                },
                result_open: true,
                result_tab: SequencerResultTabRecord::Bit,
                result_title: "Perseid Sequencer [manual load analysis #3]".to_string(),
                analysis_count: 3,
                result_source: Some("manual load".to_string()),
            },
            tls: TlsWorkspace {
                intercept: true,
                passthrough: vec!["pinned.example.test".to_string()],
            },
            ..Default::default()
        }
    }

    #[test]
    fn workspace_round_trips_through_the_app() {
        let mut app = PerseidApp::test_app();
        let workspace = sample_workspace();

        app.apply_workspace(&workspace);

        assert_eq!(app.main_tab, MainTab::Comparer);
        assert_eq!(app.proxy_section, ProxySection::Tls);
        assert_eq!(app.target_section, TargetSection::Scope);
        assert_eq!(app.request_view, MessageView::Hex);
        assert_eq!(app.filter, "status:200");
        assert!(app.scope_enabled);
        assert!(app.scope_enforce);
        assert!(app.tls_intercept);
        assert_eq!(app.tls_passthrough, ["pinned.example.test"]);
        assert_eq!(app.inspector.workspace(), workspace.inspector);
        assert_eq!(app.repeater_workspace(), workspace.repeater);
        assert_eq!(app.intruder_workspace(), workspace.intruder);
        assert_eq!(app.decoder_workspace(), workspace.decoder);
        assert_eq!(app.comparer_workspace(), workspace.comparer);
        assert_eq!(app.comparer_ui_workspace(), workspace.comparer_ui);
        assert_eq!(app.search.workspace(), workspace.search);
        assert_eq!(app.sequencer.workspace(), workspace.sequencer);
        assert_eq!(app.workspace_snapshot(), workspace);
    }

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
