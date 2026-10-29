//! Project file commands and the workspace state stored with a project.
//!
//! Perseid starts with a temporary in-memory project. `File > Save Project As`
//! moves it into a `.perseid` file; `Open` switches to an existing one. The
//! dialogs use an in-app folder browser rather than a platform file picker.
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
    ProjectStatus, ProxySectionRecord, RetentionLimits, RuleDirectionRecord, RulePartRecord,
    RuleWorkspace, ScopeWorkspace, TargetSectionRecord, TlsWorkspace, UiWorkspace, WorkspaceState,
};
use crate::proxy::{MatchReplaceRule, RuleDirection, RulePart};

use super::file_dialog::{FileDialog, FileDialogAction, FileDialogMode};
use super::*;

/// How often the unchanged-workstation check runs. Edits are still flushed on
/// Save and on exit; this only bounds how much a crash can lose.
const WORKSPACE_SYNC_INTERVAL: Duration = Duration::from_secs(1);
/// How long a command result stays visible in the status bar.
const NOTICE_LIFETIME: Duration = Duration::from_secs(8);
const PROJECT_EXTENSION: &str = "perseid";
const TEMPORARY_LABEL: &str = "Untitled (temporary)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProjectDialogKind {
    Open,
    SaveAs,
    SaveCopy,
}

impl ProjectDialogKind {
    fn title(self) -> &'static str {
        match self {
            Self::Open => "Open Project",
            Self::SaveAs => "Save Project As",
            Self::SaveCopy => "Save Project Copy",
        }
    }

    fn submit_label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::SaveAs => "Save",
            Self::SaveCopy => "Save Copy",
        }
    }

    fn file_mode(self) -> FileDialogMode {
        match self {
            Self::Open => FileDialogMode::Open,
            Self::SaveAs | Self::SaveCopy => FileDialogMode::Save,
        }
    }
}

/// A project switch that may need to be postponed until the current temporary
/// project is saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ProjectSwitch {
    New,
    Open(PathBuf),
}

struct ProjectSwitchPrompt {
    request: ProjectSwitch,
}

enum ProjectSwitchAction {
    Save,
    Discard,
    Cancel,
}

struct ProjectDialog {
    kind: ProjectDialogKind,
    browser: FileDialog,
    error: Option<String>,
    overwrite: Option<PathBuf>,
}

struct ProjectNotice {
    text: String,
    error: bool,
    at: Instant,
}

enum PendingProject {
    PrepareOpen {
        path: PathBuf,
        receiver: Receiver<std::result::Result<crate::project::PreparedProject, ProjectError>>,
    },
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
            Self::Save { .. } | Self::PrepareOpen { .. } | Self::Open { .. } | Self::New { .. }
        )
    }
}

enum ProjectCompletion {
    Prepared {
        path: PathBuf,
        result: std::result::Result<crate::project::PreparedProject, ProjectError>,
    },
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
    Submit {
        kind: ProjectDialogKind,
        path: PathBuf,
        overwrite: bool,
    },
}

/// UI state for the current project.
mod retention;
use retention::RetentionSweep;

pub(super) struct ProjectUi {
    retention_seen: (u64, u64),
    retention_sweep: Option<RetentionSweep>,
    pub(super) history_browser: super::history_browser::HistoryBrowser,
    storage_limits: Option<RetentionLimits>,
    retention_pending: Option<Receiver<std::result::Result<CommitReceipt, ProjectError>>>,
    handle: Option<ProjectHandle>,
    dialog: Option<ProjectDialog>,
    dialog_icon: Option<egui::TextureHandle>,
    switch_prompt: Option<ProjectSwitchPrompt>,
    switch_after_save: Option<ProjectSwitch>,
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
    pub(super) fn new(ui_limit: usize, ui_bytes: usize, retention: RetentionLimits) -> Self {
        let handle = match ProjectHandle::temporary_with_retention(retention) {
            Ok(handle) => Some(handle),
            Err(err) => {
                log::error!("could not start the temporary project: {err}");
                None
            }
        };
        let mut ui = Self {
            retention_seen: (0, 0),
            retention_sweep: None,
            history_browser: Default::default(),
            storage_limits: None,
            retention_pending: None,
            handle,
            dialog: None,
            dialog_icon: None,
            switch_prompt: None,
            switch_after_save: None,
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

    fn dialog_icon(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if self.dialog_icon.is_none() {
            self.dialog_icon = branding::mark_texture(ctx);
        }
        self.dialog_icon.clone()
    }

    pub(super) fn is_busy(&self) -> bool {
        self.retention_pending.is_some()
            || self.pending.iter().any(PendingProject::is_file_operation)
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
        self.history_browser = Default::default();
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

    pub(super) fn open_dialog(&mut self, kind: ProjectDialogKind, current: Option<&Path>) {
        let (directory, file_name) = default_project_location(kind, current);
        let mut dialog = ProjectDialog {
            kind,
            browser: FileDialog::new(directory, file_name, kind.file_mode()),
            error: None,
            overwrite: None,
        };
        let candidate = dialog.browser.directory().join(dialog.browser.file_name());
        if candidate.is_file() {
            dialog.browser.set_path(&candidate);
        }
        self.dialog = Some(dialog);
    }

    /// Reopen a save dialog for a destination that appeared while a save was
    /// in flight. The overwrite prompt is shown immediately.
    fn reopen_save_dialog(&mut self, kind: ProjectDialogKind, path: &Path) {
        self.open_dialog(kind, Some(path));
        let dialog = self.dialog.as_mut().expect("dialog was just opened");
        dialog.browser.set_path(path);
        dialog.overwrite = Some(path.to_path_buf());
    }

    /// Ask for Save/Discard/Cancel before replacing a project with unsaved
    /// temporary content.
    fn ask_before_switch(&mut self, request: ProjectSwitch) {
        self.switch_prompt = Some(ProjectSwitchPrompt { request });
    }

    fn take_switch_request(&mut self) -> Option<ProjectSwitch> {
        self.switch_prompt.take().map(|prompt| prompt.request)
    }

    fn cancel_switch(&mut self) {
        self.switch_prompt = None;
    }

    fn take_switch_after_save(&mut self) -> Option<ProjectSwitch> {
        self.switch_after_save.take()
    }

    fn clear_switch_after_save(&mut self) {
        self.switch_after_save = None;
    }

    /// The unsaved-work prompt chose Save: remember the pending switch and let
    /// the Save As dialog continue it after a successful save.
    fn begin_switch_save(&mut self) {
        let Some(request) = self.take_switch_request() else {
            return;
        };
        self.switch_after_save = Some(request);
        let current = self
            .status()
            .and_then(|status| status.mode.path().map(Path::to_path_buf));
        self.open_dialog(ProjectDialogKind::SaveAs, current.as_deref());
    }

    fn close_dialog(&mut self) {
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.kind == ProjectDialogKind::SaveAs)
        {
            // Cancelling the Save As also cancels the switch it was meant to
            // enable. The current project stays open either way.
            self.switch_after_save = None;
        }
        self.dialog = None;
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
        let directory = std::env::temp_dir().join("perseid-screenshot-assessments");
        let _ = std::fs::create_dir_all(&directory);
        let _ = std::fs::create_dir_all(directory.join("internal-lab"));
        for name in ["demo-shop.perseid", "api-review.perseid"] {
            let path = directory.join(name);
            if !path.exists() {
                let _ = std::fs::write(path, b"");
            }
        }
        self.open_dialog(
            ProjectDialogKind::SaveAs,
            Some(&directory.join("demo-shop.perseid")),
        );
    }

    fn dialog(&mut self, ctx: &egui::Context) -> Option<ProjectDialogAction> {
        let kind = self.dialog.as_ref()?.kind;
        let icon = self.dialog_icon(ctx);
        let overwrite_already_open = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.overwrite.is_some());
        let mut action = FileDialogAction::None;

        egui::Modal::new(egui::Id::unique("perseid.project.dialog"))
            .backdrop_color(if overwrite_already_open {
                egui::Color32::TRANSPARENT
            } else {
                egui::Color32::from_black_alpha(42)
            })
            .frame(super::file_dialog::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(super::file_dialog::MODAL_WIDTH);
                if super::file_dialog::title_bar(ui, kind.title(), icon.as_ref()) {
                    action = FileDialogAction::Cancel;
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 16,
                        top: 12,
                        bottom: 14,
                    })
                    .show(ui, |ui| {
                        ui.set_width(super::file_dialog::MODAL_WIDTH - 32.0);
                        let dialog = self.dialog.as_mut().expect("dialog is still open");
                        let mut error = dialog.error.take();
                        action = dialog.browser.ui(ui, kind.submit_label(), &mut error);
                        dialog.error = error;
                    });
            });

        let escape =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let overwrite_open = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.overwrite.is_some());
        if escape {
            if overwrite_open {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.overwrite = None;
                }
            } else {
                self.close_dialog();
                return None;
            }
        }

        if let FileDialogAction::Cancel = action {
            self.close_dialog();
            return None;
        }

        if !overwrite_open && matches!(action, FileDialogAction::Submit) {
            match self.prepare_submission(kind) {
                Ok(path) if kind == ProjectDialogKind::Open => {
                    self.dialog = None;
                    return Some(ProjectDialogAction::Submit {
                        kind,
                        path,
                        overwrite: false,
                    });
                }
                Ok(path) if path.is_file() => {
                    if let Some(dialog) = self.dialog.as_mut() {
                        dialog.overwrite = Some(path);
                        dialog.error = None;
                    }
                }
                Ok(path) => {
                    self.dialog = None;
                    return Some(ProjectDialogAction::Submit {
                        kind,
                        path,
                        overwrite: false,
                    });
                }
                Err(error) => {
                    if let Some(dialog) = self.dialog.as_mut() {
                        dialog.error = Some(error);
                    }
                }
            }
        }

        if let Some(path) = self
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.overwrite.clone())
        {
            let mut choice = None;
            let mut closed = false;
            egui::Modal::new(egui::Id::unique("perseid.project.overwrite"))
                .backdrop_color(egui::Color32::from_black_alpha(70))
                .frame(super::file_dialog::modal_frame())
                .show(ctx, |ui| {
                    ui.set_width(460.0);
                    if super::file_dialog::title_bar(ui, "Overwrite File?", icon.as_ref()) {
                        closed = true;
                    }
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 16,
                            right: 16,
                            top: 14,
                            bottom: 14,
                        })
                        .show(ui, |ui| {
                            ui.set_width(460.0 - 32.0);
                            ui.label("A file already exists at:");
                            ui.label(
                                RichText::new(path.display().to_string())
                                    .color(theme::TEXT)
                                    .strong(),
                            );
                            ui.add_space(6.0);
                            ui.label("Replacing it cannot be undone.");
                            ui.add_space(16.0);
                            egui::Sides::new().height(30.0).show(
                                ui,
                                |_ui| {},
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [104.0, 30.0],
                                            egui::Button::new("Keep Existing"),
                                        )
                                        .clicked()
                                    {
                                        choice = Some(false);
                                    }
                                    if ui
                                        .add_sized([88.0, 30.0], egui::Button::new("Overwrite"))
                                        .clicked()
                                    {
                                        choice = Some(true);
                                    }
                                },
                            );
                        });
                });
            if closed && choice.is_none() {
                choice = Some(false);
            }
            match choice {
                Some(true) => {
                    self.dialog = None;
                    return Some(ProjectDialogAction::Submit {
                        kind,
                        path,
                        overwrite: true,
                    });
                }
                Some(false) => {
                    if let Some(dialog) = self.dialog.as_mut() {
                        dialog.overwrite = None;
                    }
                }
                None => {}
            }
        }

        None
    }

    fn prepare_submission(&mut self, kind: ProjectDialogKind) -> Result<PathBuf, String> {
        let Some(dialog) = self.dialog.as_mut() else {
            return Err("The project dialog is no longer open.".to_string());
        };
        let input = dialog.browser.file_name().trim().to_string();
        if input.is_empty() {
            return Err(match kind {
                ProjectDialogKind::Open => "Choose a project file to open.".to_string(),
                ProjectDialogKind::SaveAs | ProjectDialogKind::SaveCopy => {
                    "Enter a project file name.".to_string()
                }
            });
        }
        let path = project_path_in(dialog.browser.directory(), &input);
        if path.is_dir() {
            return Err("The destination is a folder. Choose a file name.".to_string());
        }
        match kind {
            ProjectDialogKind::Open => {
                if !path.is_file() {
                    return Err(format!("{} does not exist.", path.display()));
                }
            }
            ProjectDialogKind::SaveAs | ProjectDialogKind::SaveCopy => {
                if let Some(parent) = path.parent()
                    && !parent.is_dir()
                {
                    return Err(format!("The folder {} does not exist.", parent.display()));
                }
            }
        }
        dialog.error = None;
        Ok(path)
    }

    /// Ask whether to save the current temporary project before replacing it.
    fn switch_prompt(&mut self, ctx: &egui::Context) -> Option<ProjectSwitchAction> {
        let prompt = self.switch_prompt.as_ref()?;
        let destination = match &prompt.request {
            ProjectSwitch::New => {
                "Starting a new project will replace the current temporary project.".to_string()
            }
            ProjectSwitch::Open(path) => format!(
                "Opening {} will replace the current temporary project.",
                path.display()
            ),
        };
        let mut open = true;
        let mut action = None;
        egui::Window::new("Unsaved Project")
            .id(egui::Id::unique("perseid.project.unsaved"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.label("The current temporary project has not been saved.");
                ui.add_space(4.0);
                ui.label(RichText::new(destination).small().color(theme::TEXT_MUTED));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save…").clicked() {
                        action = Some(ProjectSwitchAction::Save);
                    }
                    if ui.button("Discard").clicked() {
                        action = Some(ProjectSwitchAction::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(ProjectSwitchAction::Cancel);
                    }
                });
            });
        if action.is_some() {
            return action;
        }
        if !open {
            self.switch_prompt = None;
            return Some(ProjectSwitchAction::Cancel);
        }
        None
    }

    fn poll(&mut self) -> Vec<ProjectCompletion> {
        let mut completed = Vec::new();
        let mut keep = Vec::with_capacity(self.pending.len());
        for pending in self.pending.drain(..) {
            match pending {
                PendingProject::PrepareOpen { path, receiver } => match receiver.try_recv() {
                    Ok(result) => completed.push(ProjectCompletion::Prepared { path, result }),
                    Err(TryRecvError::Empty) => {
                        keep.push(PendingProject::PrepareOpen { path, receiver })
                    }
                    Err(TryRecvError::Disconnected) => {
                        completed.push(ProjectCompletion::Prepared {
                            path,
                            result: Err(ProjectError::ServiceUnavailable),
                        })
                    }
                },
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

#[cfg(test)]
fn normalize_project_path(input: &str) -> PathBuf {
    normalize_project_path_buf(PathBuf::from(input))
}

fn normalize_project_path_buf(path: PathBuf) -> PathBuf {
    if path.extension().is_none() {
        path.with_extension(PROJECT_EXTENSION)
    } else {
        path
    }
}

/// Resolve a file name typed into the dialog against the folder it lists.
fn project_path_in(directory: &Path, input: &str) -> PathBuf {
    let mut path = PathBuf::from(input.trim());
    if path.is_relative() {
        path = directory.join(path);
    }
    normalize_project_path_buf(path)
}

fn default_project_location(kind: ProjectDialogKind, current: Option<&Path>) -> (PathBuf, String) {
    let cwd = || std::env::current_dir().unwrap_or_default();
    let current_directory = current
        .and_then(Path::parent)
        .filter(|parent| parent.is_dir())
        .map(Path::to_path_buf);
    match kind {
        ProjectDialogKind::Open => (current_directory.unwrap_or_else(cwd), String::new()),
        ProjectDialogKind::SaveAs => {
            let name = current
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "perseid-project.perseid".to_string());
            (current_directory.unwrap_or_else(cwd), name)
        }
        ProjectDialogKind::SaveCopy => {
            let name = current
                .and_then(Path::file_stem)
                .and_then(|stem| stem.to_str())
                .map(|stem| format!("{stem}-copy.perseid"))
                .unwrap_or_else(|| "perseid-project-copy.perseid".to_string());
            (current_directory.unwrap_or_else(cwd), name)
        }
    }
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
        TargetSection::Issues => TargetSectionRecord::Issues,
    }
}

fn target_section_from_record(section: TargetSectionRecord) -> TargetSection {
    match section {
        TargetSectionRecord::SiteMap => TargetSection::SiteMap,
        TargetSectionRecord::Scope => TargetSection::Scope,
        TargetSectionRecord::Issues => TargetSection::Issues,
    }
}

impl PerseidApp {
    pub(super) fn open_project_storage(&mut self) {
        if let Some(status) = self.project.status() {
            self.project.storage_limits = Some(status.retention);
        }
    }

    fn project_storage_window(&mut self, ctx: &egui::Context) {
        let Some(mut limits) = self.project.storage_limits else {
            return;
        };
        let Some(status) = self.project.status() else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        let saved = matches!(status.mode, ProjectMode::Persistent { .. });
        egui::Window::new("Project storage")
            .open(&mut open).collapsible(false).resizable(false).default_width(480.0)
            .show(ctx, |ui| {
                ui.label(format!("{} HTTP exchanges, {} WebSocket messages, {} retained body bytes", status.http_count, status.ws_count, status.retained_body_bytes));
                if !saved { ui.label("Save this project first to configure saved-project retention. Temporary retention is configured by startup options."); }
                ui.add_enabled_ui(saved && !self.project.is_busy(), |ui| {
                    ui.horizontal(|ui| { ui.label("Maximum records (HTTP + WebSocket)"); ui.add(egui::DragValue::new(&mut limits.max_records).range(0..=u64::MAX)); });
                    ui.horizontal(|ui| { ui.label("Maximum retained body bytes"); ui.add(egui::DragValue::new(&mut limits.max_body_bytes).range(0..=u64::MAX)); });
                    ui.label("Zero means unlimited. Apply removes oldest eligible records immediately and cannot be undone. Save a copy before lowering limits.");
                    ui.weak("In-flight HTTP exchanges and the newest record of each stream are kept even when limits are exceeded. Limits exclude headers and workbench data, and do not cap physical file size.");
                    if ui.button("Apply retention limits").clicked() { apply = true; }
                });
                ui.separator();
                ui.weak("Clear history removes captures, not the project file or tool snapshots. To remove a project: switch away or quit, then delete its .perseid file in your file manager. Projects are not encrypted. Deletion is not secure erasure.");
            });
        if apply && let Some(handle) = self.project.handle() {
            self.project.retention_pending = Some(handle.set_retention(limits));
            open = false;
        }
        self.project.storage_limits = open.then_some(limits);
        if self.project.retention_pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// The File menu: project lifecycle commands live above Quit.
    #[cfg(any(not(target_os = "macos"), test))]
    pub(super) fn project_menu(&mut self, ui: &mut egui::Ui) {
        let response = ui.menu_button("File", |ui| {
            if ui.button("New Project").clicked() {
                self.request_project_switch(ProjectSwitch::New);
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
            // otherwise leak into every later screenshot. The forced state is
            // only for the screenshot runner; normal builds of the feature
            // still need the menu to open and close through user input.
            if self.screenshots.is_some() {
                let popup_id = response.response.id.with("popup");
                if self.project.file_menu_forced_open {
                    egui::Popup::open_id(ui.ctx(), popup_id);
                } else {
                    egui::Popup::close_id(ui.ctx(), popup_id);
                }
            }
        }
        let _ = response;
    }

    pub(super) fn current_project_path(&self) -> Option<PathBuf> {
        self.project
            .status()
            .and_then(|status| status.mode.path().map(Path::to_path_buf))
    }

    fn notify_project_busy(&mut self) {
        self.project
            .notify_error("Wait for the current project operation to finish.");
    }

    /// A temporary project is worth a confirmation when it holds captured
    /// records or non-default workbench state. The committed counters lag the
    /// capture queue by a write batch, so the live UI histories count too.
    fn has_unsaved_temporary_work(&self) -> bool {
        let Some(status) = self.project.status() else {
            return false;
        };
        if !matches!(status.mode, ProjectMode::Temporary) {
            return false;
        }
        status.http_count.saturating_add(status.ws_count) > 0
            || self.history.len() > 0
            || self.websocket_history.len() > 0
            || self.workspace_snapshot() != self.project_baseline_workspace
    }

    pub(super) fn request_project_switch(&mut self, request: ProjectSwitch) {
        if self.project.is_busy() {
            self.notify_project_busy();
            return;
        }
        if self.has_unsaved_temporary_work() {
            self.project.ask_before_switch(request);
        } else {
            self.perform_project_switch(request);
        }
    }

    fn perform_project_switch(&mut self, request: ProjectSwitch) {
        match request {
            ProjectSwitch::New => self.begin_new_project(),
            ProjectSwitch::Open(path) => self.begin_open_project(path),
        }
    }

    fn begin_save_project(&mut self, path: PathBuf, copy: bool, overwrite: bool) {
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
            handle.save_copy(path.clone(), overwrite)
        } else {
            handle.save_as(path.clone(), overwrite)
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
        let Some(handle) = self.project.handle() else {
            self.project
                .notify_error("Persistence is unavailable for this session.");
            return;
        };
        let receiver =
            handle.prepare_open(path.clone(), self.project.ui_limit, self.project.ui_bytes);
        self.project.notify(format!("Opening {}…", path.display()));
        self.project
            .pending
            .push(PendingProject::PrepareOpen { path, receiver });
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
        self.poll_retention_cache();
        if let Some(receiver) = &self.project.retention_pending {
            match receiver.try_recv() {
                Ok(Ok(_)) => {
                    self.project.retention_pending = None;
                    self.project.history_browser = Default::default();
                    self.project.notify("Saved retention limits. Oldest eligible records were removed; use Browse stored history to see retained records.");
                }
                Ok(Err(err)) => {
                    self.project.retention_pending = None;
                    self.project
                        .notify_error(format!("Could not set retention: {err}"));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.project.retention_pending = None;
                    self.project
                        .notify_error("Project writer stopped before saving retention limits.");
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        for completion in self.project.poll() {
            match completion {
                ProjectCompletion::Prepared { path, result } => match result {
                    Ok(prepared) => {
                        self.sync_workspace(true);
                        let restart_proxy = self.stop_proxy_for_project_switch();
                        if let Some(handle) = self.project.handle() {
                            let receiver = handle.install_prepared(prepared);
                            self.project.pending.push(PendingProject::Open {
                                path,
                                receiver,
                                restart_proxy,
                            });
                        }
                    }
                    Err(err) => self
                        .project
                        .notify_error(format!("Could not open the project: {err}")),
                },
                ProjectCompletion::Saved { copy, path, result } => match result {
                    Ok(()) => {
                        self.project.notify(if copy {
                            format!("Saved a copy to {}", path.display())
                        } else {
                            format!("Saved project to {}", path.display())
                        });
                        // The save command persisted the workspace queued when
                        // it started. Flush edits made while it was pending so
                        // they are not marked saved and then discarded by a
                        // switch that follows the save.
                        self.sync_workspace(true);
                        if copy {
                            self.project.clear_switch_after_save();
                        } else if let Some(request) = self.project.take_switch_after_save() {
                            self.perform_project_switch(request);
                        }
                    }
                    Err(err) => {
                        if let ProjectError::DestinationExists(existing) = &err {
                            // The destination appeared after the dialog checked
                            // it. Keep the save available and ask explicitly.
                            let kind = if copy {
                                ProjectDialogKind::SaveCopy
                            } else {
                                ProjectDialogKind::SaveAs
                            };
                            self.project.reopen_save_dialog(kind, existing);
                            self.project.notify_error(format!(
                                "A file already exists at {}.",
                                existing.display()
                            ));
                        } else {
                            self.project.clear_switch_after_save();
                            self.project
                                .notify_error(format!("Could not save the project: {err}"));
                        }
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
        self.project.retention_sweep = None;
        self.project.retention_seen = (0, 0);
        self.project.history_browser = Default::default();
        self.project.storage_limits = None;
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
        let baseline = self.workspace_snapshot();
        self.project.last_workspace = Some(baseline.clone());
        self.project_baseline_workspace = baseline;
        self.project.last_workspace_check = Instant::now();

        let truncated = http_loaded < http_total || ws_loaded < ws_total;
        if truncated {
            self.project.notify(format!(
                "Loaded the newest {} of {http_total} exchanges and {ws_loaded} of {ws_total} WebSocket messages; use Browse stored history or Search for older records.",
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
            history_filter: self.history_filter.to_workspace(),
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
        self.history_filter = HistoryFilter::from_workspace(&workspace.history_filter);
        self.history_filter_draft = None;
        self.title_cache.borrow_mut().clear();
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
        // Site-map filters and node selection are view-local and can refer to
        // hosts or exchanges that are absent from the project just opened.
        self.site_map_ui = target::SiteMapUi::new();
    }

    pub(super) fn workspace_snapshot(&self) -> WorkspaceState {
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
            .apply_workspace(&workspace.search, &self.history, &self.websocket_history);
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
        if self.project.is_busy() || self.project.retention_sweep.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.stored_history_window(ctx);
        self.project_storage_window(ctx);
        if let Some(action) = self.project.switch_prompt(ctx) {
            match action {
                ProjectSwitchAction::Save => self.project.begin_switch_save(),
                ProjectSwitchAction::Discard => {
                    if let Some(request) = self.project.take_switch_request() {
                        self.perform_project_switch(request);
                    }
                }
                ProjectSwitchAction::Cancel => self.project.cancel_switch(),
            }
        }
        if let Some(action) = self.project.dialog(ctx) {
            match action {
                ProjectDialogAction::Submit {
                    kind: ProjectDialogKind::Open,
                    path,
                    ..
                } => {
                    self.request_project_switch(ProjectSwitch::Open(path));
                }
                ProjectDialogAction::Submit {
                    kind: ProjectDialogKind::SaveAs,
                    path,
                    overwrite,
                } => {
                    self.begin_save_project(path, false, overwrite);
                }
                ProjectDialogAction::Submit {
                    kind: ProjectDialogKind::SaveCopy,
                    path,
                    overwrite,
                } => {
                    self.begin_save_project(path, true, overwrite);
                }
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
            if status.retention_evictions > 0 {
                ui.colored_label(
                    theme::AMBER,
                    format!("{} old records evicted", status.retention_evictions),
                )
                .on_hover_text(
                    "Project retention removed the oldest captured records to stay within the configured logical limits. This does not securely erase disk bytes.",
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
pub(super) mod tests {
    use super::*;
    use crate::project::{
        BodyWorkspace, CapturePartRecord, CompareModeRecord, ComparerWorkspace, DecoderCodecRecord,
        DecoderColorTagRecord, DecoderOperationRecord, DecoderPanelWorkspace, DecoderSpanWorkspace,
        DecoderViewRecord, HistoryFilterWorkspace, InspectorNoteWorkspace, InspectorWorkspace,
        IntruderAttackRecord, IntruderPlanWorkspace, IntruderResultOutcomeWorkspace,
        IntruderResultWorkspace, IntruderWorkspace, MessagePaneRecord, MessageViewRecord,
        MessageViewsWorkspace, RepeaterResponseWorkspace, RepeaterWorkspace, ResultViewRecord,
        SearchWorkspace, SequencerResultTabRecord, SequencerSectionRecord,
        SequencerSettingsWorkspace, SideTabRecord, TextSelectionWorkspace, TlsWorkspace,
        UiWorkspace,
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
                history_filter: HistoryFilterWorkspace {
                    show_css: false,
                    show_images: false,
                    search: "login".to_string(),
                    search_regex: true,
                    ..HistoryFilterWorkspace::default()
                },
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
                result_items: Vec::new(),
                selected_item: None,
                limit_reached: false,
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
        assert_eq!(
            app.history_filter.to_workspace(),
            workspace.ui.history_filter
        );
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
    fn target_issues_section_round_trips_through_the_workspace() {
        let mut app = PerseidApp::test_app();
        app.target_section = TargetSection::Issues;

        let workspace = app.workspace_snapshot();
        assert_eq!(workspace.ui.target_section, TargetSectionRecord::Issues);

        app.target_section = TargetSection::SiteMap;
        app.apply_workspace(&workspace);
        assert_eq!(app.target_section, TargetSection::Issues);
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
        let directory =
            std::env::temp_dir().join(format!("perseid-copy-default-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create temporary directory");
        let source = directory.join("session.perseid");

        let (parent, name) = default_project_location(ProjectDialogKind::SaveCopy, Some(&source));

        assert_eq!(parent, directory);
        assert_eq!(name, "session-copy.perseid");
        let _ = std::fs::remove_dir_all(&directory);
    }

    use crate::history::{HistoryItem, Phase};
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo,
        WebSocketConnectionInfo, WebSocketDirection, WebSocketOpcode,
    };
    use crate::websocket_history::WebSocketMessageItem;
    use std::time::SystemTime;

    fn screen_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))
    }

    fn find_text_center(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + egui::vec2(text.galley.size().x / 2.0, text.galley.size().y / 2.0))
            }
            egui::Shape::Vec(shapes) => shapes
                .iter()
                .find_map(|shape| find_text_center(shape, label)),
            _ => None,
        }
    }

    /// Click the first widget whose label matches. Returns false when the
    /// label is not rendered.
    pub(in crate::app) fn click_window_text(
        ctx: &egui::Context,
        label: &str,
        mut run: impl FnMut(&egui::Context),
    ) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            ..Default::default()
        };
        // Windows lay out their contents over several passes.
        let mut output = ctx.run_pass(input.clone(), |ctx| run(ctx));
        output.textures_delta.clear();
        let mut output = ctx.run_pass(input.clone(), |ctx| run(ctx));
        output.textures_delta.clear();
        let Some(pos) = output
            .shapes
            .iter()
            .find_map(|clipped| find_text_center(&clipped.shape, label))
        else {
            return false;
        };
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_pass(input, |ctx| run(ctx));
        output.textures_delta.clear();
        true
    }

    /// Click the first widget whose label matches inside a root Ui. Returns
    /// false when the label is not rendered.
    fn click_ui_text(ctx: &egui::Context, label: &str, mut run: impl FnMut(&mut egui::Ui)) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            ..Default::default()
        };
        // A menu popup only appears in the pass after the button click.
        let mut output = ctx.run_ui(input.clone(), |ui| run(ui));
        output.textures_delta.clear();
        let mut output = ctx.run_ui(input.clone(), |ui| run(ui));
        output.textures_delta.clear();
        let Some(pos) = output
            .shapes
            .iter()
            .find_map(|clipped| find_text_center(&clipped.shape, label))
        else {
            return false;
        };
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| run(ui));
        output.textures_delta.clear();
        true
    }

    fn frame(mut run: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input.clone(), |ui| run(ui));
        output.textures_delta.clear();
        let mut output = ctx.run_ui(input, |ui| run(ui));
        output.textures_delta.clear();
        output
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    fn wait_for<T>(receiver: std::sync::mpsc::Receiver<std::result::Result<T, ProjectError>>) -> T {
        receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("project writer dropped the reply channel")
            .expect("project command failed")
    }

    /// Run the project poll loop until no command is pending. Commands finish
    /// on the writer thread, so a short bounded wait keeps the test honest.
    fn poll_until_idle(app: &mut PerseidApp) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !app.project.pending.is_empty() {
            app.poll_project();
            if app.project.pending.is_empty() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the project command did not complete"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn history_item() -> HistoryItem {
        HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::UNIX_EPOCH,
            request: RequestInfo {
                method: "GET".to_string(),
                uri: "http://example.test/".to_string(),
                scheme: "http".to_string(),
                host: "example.test".to_string(),
                path: "/".to_string(),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
            request_body: CapturedBody::default(),
            response: None,
            phase: Phase::InFlight,
        }
    }

    fn websocket_item() -> WebSocketMessageItem {
        WebSocketMessageItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::UNIX_EPOCH,
            connection: WebSocketConnectionInfo {
                id: 1,
                url: "wss://example.test/socket".to_string(),
                host: "example.test".to_string(),
                tls: true,
                listener_port: 8443,
                protocol: None,
            },
            direction: WebSocketDirection::ToServer,
            opcode: WebSocketOpcode::Text,
            payload: CapturedBody::from_bytes(b"hello".to_vec()),
        }
    }

    fn started_event(wire_id: u64) -> ProxyEvent {
        ProxyEvent::Started {
            id: wire_id,
            timestamp: SystemTime::UNIX_EPOCH,
            request: RequestInfo {
                method: "GET".to_string(),
                uri: "http://example.test/".to_string(),
                scheme: "http".to_string(),
                host: "example.test".to_string(),
                path: "/".to_string(),
                version: "HTTP/1.1".to_string(),
                headers: Vec::new(),
            },
        }
    }

    fn finished_event(wire_id: u64) -> ProxyEvent {
        ProxyEvent::Finished {
            id: wire_id,
            end: ExchangeEnd {
                duration: Duration::from_millis(1),
                request_body: CapturedBody::default(),
                result: ExchangeResult::Response {
                    body: CapturedBody::default(),
                },
            },
        }
    }

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("perseid-{label}-{}.perseid", uuid::Uuid::new_v4()))
    }

    #[test]
    fn live_history_counts_as_unsaved_temporary_work() {
        // Committed counters lag the writer batch, so the prompt must also
        // consider rows that are still only in the live UI history.
        let mut app = PerseidApp::test_app();
        assert!(!app.has_unsaved_temporary_work());
        app.history.load(vec![history_item()]);
        assert!(app.has_unsaved_temporary_work());
        app.request_project_switch(ProjectSwitch::New);
        assert!(app.project.switch_prompt.is_some());

        let mut app = PerseidApp::test_app();
        app.websocket_history.load(vec![websocket_item()]);
        assert!(app.has_unsaved_temporary_work());
    }

    #[test]
    fn saved_projects_and_missing_handles_are_never_unsaved() {
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;
        assert!(app.has_unsaved_temporary_work());

        app.project.handle = None;
        assert!(!app.has_unsaved_temporary_work());

        // A persistent project never prompts, even with non-default state.
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;
        let file = temp_path("ui-persistent");
        wait_for(
            app.project
                .handle()
                .expect("handle")
                .save_as(file.clone(), false),
        );
        assert!(!app.has_unsaved_temporary_work());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_busy_project_refuses_a_switch_with_a_notice() {
        let mut app = PerseidApp::test_app();
        let (_sender, receiver) = std::sync::mpsc::channel();
        app.project.pending.push(PendingProject::Save {
            copy: false,
            path: PathBuf::from("/tmp/demo.perseid"),
            receiver,
        });

        app.request_project_switch(ProjectSwitch::New);

        assert!(app.project.switch_prompt.is_none());
        assert_eq!(app.project.pending.len(), 1);
        let (text, error) = app.project.notice().expect("busy notice");
        assert!(error);
        assert!(text.contains("Wait for the current project operation to finish"));
    }

    #[test]
    fn the_file_menu_new_project_queues_a_switch() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();

        assert!(click_ui_text(&ctx, "File", |ui| app.project_menu(ui)));
        assert!(click_ui_text(&ctx, "New Project", |ui| app.project_menu(ui)));
        assert!(matches!(
            app.project.pending.as_slice(),
            [PendingProject::New { .. }]
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_file_menu_opens_each_project_dialog_and_respects_unsaved_work() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        for (action, expected) in [
            ("open", ProjectDialogKind::Open),
            ("save_as", ProjectDialogKind::SaveAs),
            ("save_copy", ProjectDialogKind::SaveCopy),
        ] {
            app.handle_native_menu_action(&ctx, action);
            assert_eq!(app.project.dialog.as_ref().unwrap().kind, expected);
        }
        app.project.dialog = None;
        app.scope_enabled = true;
        app.handle_native_menu_action(&ctx, "new");
        assert!(app.project.switch_prompt.is_some());
        assert!(app.project.pending.is_empty());
        app.shutdown_project();
    }

    #[test]
    fn switch_prompt_buttons_cancel_discard_and_save() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;

        app.request_project_switch(ProjectSwitch::New);
        assert!(click_window_text(&ctx, "Cancel", |ctx| app.project_windows(ctx)));
        assert!(app.project.switch_prompt.is_none());
        assert!(app.project.pending.is_empty());
        assert!(app.scope_enabled);

        app.request_project_switch(ProjectSwitch::New);
        assert!(click_window_text(&ctx, "Discard", |ctx| app.project_windows(ctx)));
        assert!(app.project.switch_prompt.is_none());
        assert!(matches!(
            app.project.pending.as_slice(),
            [PendingProject::New { .. }]
        ));
        app.project.pending.clear();

        app.request_project_switch(ProjectSwitch::New);
        assert!(click_window_text(&ctx, "Save…", |ctx| app.project_windows(ctx)));
        assert!(app.project.switch_prompt.is_none());
        assert_eq!(app.project.switch_after_save, Some(ProjectSwitch::New));
        assert_eq!(
            app.project.dialog.as_ref().expect("save dialog").kind,
            ProjectDialogKind::SaveAs
        );

        // Cancelling the Save As also cancels the switch it was meant to enable.
        assert!(click_window_text(&ctx, "Cancel", |ctx| app.project_windows(ctx)));
        assert!(app.project.dialog.is_none());
        assert!(app.project.switch_after_save.is_none());
        assert!(app.project.pending.is_empty());
        app.shutdown_project();
    }

    #[test]
    fn open_dialog_submits_a_normalized_path_and_queues_the_switch() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        let source = temp_path("ui-open");
        std::fs::write(&source, b"project").expect("write project placeholder");

        app.project.open_dialog(ProjectDialogKind::Open, None);
        app.project
            .dialog
            .as_mut()
            .expect("dialog")
            .browser
            .set_path(&source);
        assert!(click_window_text(&ctx, "Open", |ctx| app.project_windows(ctx)));

        assert!(app.project.dialog.is_none());
        match app.project.pending.first() {
            Some(PendingProject::PrepareOpen { path, .. }) => {
                assert_eq!(path, &source);
            }
            _ => panic!("expected a pending open"),
        }
        let _ = std::fs::remove_file(&source);
        app.shutdown_project();
    }

    #[test]
    fn a_dialog_with_an_empty_file_name_reports_an_error_and_stays_open() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        app.project.open_dialog(ProjectDialogKind::Open, None);
        app.project
            .dialog
            .as_mut()
            .expect("dialog")
            .browser
            .set_file_name("   ");

        assert!(click_window_text(&ctx, "Open", |ctx| app.project_windows(ctx)));

        let dialog = app.project.dialog.as_ref().expect("dialog stays open");
        assert_eq!(
            dialog.error.as_deref(),
            Some("Choose a project file to open.")
        );
        assert!(app.project.pending.is_empty());
    }

    #[test]
    fn open_dialog_rejects_a_missing_file() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        app.project.open_dialog(ProjectDialogKind::Open, None);
        let dialog = app.project.dialog.as_mut().expect("dialog");
        dialog.browser.set_path(&temp_path("ui-missing"));

        assert!(click_window_text(&ctx, "Open", |ctx| app.project_windows(ctx)));

        let dialog = app.project.dialog.as_ref().expect("dialog stays open");
        assert!(
            dialog
                .error
                .as_deref()
                .is_some_and(|text| text.contains("does not exist"))
        );
        assert!(app.project.pending.is_empty());
    }

    #[test]
    fn save_dialogs_queue_and_complete_their_saves() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        let save_path = temp_path("ui-save");
        let copy_path = temp_path("ui-copy");

        app.project.open_dialog(ProjectDialogKind::SaveAs, None);
        app.project
            .dialog
            .as_mut()
            .expect("dialog")
            .browser
            .set_path(&save_path);
        assert!(click_window_text(&ctx, "Save", |ctx| app.project_windows(ctx)));
        assert!(app.project.dialog.is_none());
        match app.project.pending.first() {
            Some(PendingProject::Save { copy, path, .. }) => {
                assert!(!copy);
                assert_eq!(path, &save_path);
            }
            _ => panic!("expected a pending save"),
        }
        poll_until_idle(&mut app);
        assert!(app.project.pending.is_empty());
        let (text, error) = app.project.notice().expect("save notice");
        assert!(!error, "save notice was an error: {text}");
        assert!(
            text.contains("Saved project to"),
            "unexpected notice: {text}"
        );
        assert!(save_path.exists());

        app.project.open_dialog(ProjectDialogKind::SaveCopy, None);
        app.project
            .dialog
            .as_mut()
            .expect("dialog")
            .browser
            .set_path(&copy_path);
        assert!(click_window_text(&ctx, "Save Copy", |ctx| app.project_windows(ctx)));
        match app.project.pending.first() {
            Some(PendingProject::Save { copy: true, .. }) => {}
            _ => panic!("expected a pending save copy"),
        }
        poll_until_idle(&mut app);
        let (text, error) = app.project.notice().expect("copy notice");
        assert!(!error, "copy notice was an error: {text}");
        assert!(text.contains("Saved a copy to"));
        assert_eq!(
            app.project.status().expect("status").mode.path(),
            Some(save_path.as_path()),
            "saving a copy leaves the open project unchanged"
        );

        let _ = std::fs::remove_file(&save_path);
        let _ = std::fs::remove_file(&copy_path);
    }

    #[test]
    fn saving_over_an_existing_file_asks_before_overwriting() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        let save_path = temp_path("ui-overwrite");
        std::fs::write(&save_path, b"old contents").expect("write existing file");

        app.project.open_dialog(ProjectDialogKind::SaveAs, None);
        app.project
            .dialog
            .as_mut()
            .expect("dialog")
            .browser
            .set_path(&save_path);
        assert!(click_window_text(&ctx, "Save", |ctx| app.project_windows(ctx)));

        assert!(app.project.pending.is_empty(), "save must wait for consent");
        assert_eq!(
            app.project
                .dialog
                .as_ref()
                .expect("dialog stays open")
                .overwrite
                .as_deref(),
            Some(save_path.as_path())
        );
        assert_eq!(
            std::fs::read(&save_path).expect("unmodified file"),
            b"old contents"
        );

        assert!(click_window_text(&ctx, "Overwrite", |ctx| app.project_windows(ctx)));
        assert!(app.project.dialog.is_none());
        assert!(matches!(
            app.project.pending.first(),
            Some(PendingProject::Save { copy: false, path, .. }) if path == &save_path
        ));
        poll_until_idle(&mut app);
        assert!(
            std::fs::metadata(&save_path).expect("saved file").len() > b"old contents".len() as u64,
            "the overwrite must replace the old file with a project"
        );

        let _ = std::fs::remove_file(&save_path);
    }

    #[test]
    fn keeping_an_existing_file_leaves_the_save_dialog_open() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        let save_path = temp_path("ui-keep-existing");
        std::fs::write(&save_path, b"old contents").expect("write existing file");

        app.project.open_dialog(ProjectDialogKind::SaveAs, None);
        app.project
            .dialog
            .as_mut()
            .expect("dialog")
            .browser
            .set_path(&save_path);
        assert!(click_window_text(&ctx, "Save", |ctx| app.project_windows(ctx)));
        assert!(click_window_text(&ctx, "Keep Existing", |ctx| {
            app.project_windows(ctx)
        }));

        let dialog = app.project.dialog.as_ref().expect("dialog stays open");
        assert!(dialog.overwrite.is_none());
        assert!(app.project.pending.is_empty());
        assert_eq!(
            std::fs::read(&save_path).expect("unmodified file"),
            b"old contents"
        );

        let _ = std::fs::remove_file(&save_path);
    }

    #[test]
    fn clicking_a_folder_in_the_dialog_navigates_into_it() {
        let ctx = egui::Context::default();
        let root = std::env::temp_dir().join(format!("perseid-ui-folder-{}", uuid::Uuid::new_v4()));
        let child = root.join("assessment");
        std::fs::create_dir_all(&child).expect("create folders");

        let mut app = PerseidApp::test_app();
        app.project.open_dialog(ProjectDialogKind::Open, None);
        app.project
            .dialog
            .as_mut()
            .expect("dialog")
            .browser
            .set_path(&root);

        assert!(click_window_text(&ctx, "assessment/", |ctx| {
            app.project_windows(ctx)
        }));
        assert_eq!(
            app.project
                .dialog
                .as_ref()
                .expect("dialog")
                .browser
                .directory(),
            child
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_destination_that_appears_during_a_save_reopens_with_the_overwrite_prompt() {
        let mut app = PerseidApp::test_app();
        let path = temp_path("ui-save-race");
        std::fs::write(&path, b"appeared later").expect("write existing file");
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Err(ProjectError::DestinationExists(path.clone())))
            .expect("queue save result");
        app.project.pending.push(PendingProject::Save {
            copy: false,
            path: path.clone(),
            receiver,
        });

        app.poll_project();

        let dialog = app.project.dialog.as_ref().expect("save dialog reopened");
        assert_eq!(dialog.kind, ProjectDialogKind::SaveAs);
        assert_eq!(dialog.overwrite.as_deref(), Some(path.as_path()));
        assert_eq!(
            dialog.browser.file_name(),
            path.file_name().expect("file name").to_string_lossy()
        );
        assert!(app.project.notice().expect("notice").1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_completed_save_flushes_edits_made_while_it_was_pending() {
        let mut app = PerseidApp::test_app();
        // The writer holds a workspace with an enabled scope.
        app.scope_enabled = true;
        app.sync_workspace(true);

        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Ok(())).expect("queue save result");
        app.project.pending.push(PendingProject::Save {
            copy: false,
            path: PathBuf::from("/tmp/demo.perseid"),
            receiver,
        });
        // The edit arrives after the save was queued but before it completed.
        app.scope_enabled = false;
        app.poll_project();
        assert!(app.project.pending.is_empty());

        // The edit must have been queued behind the save: a copy taken now
        // contains it even though the save itself did not.
        let copy = temp_path("ui-flush");
        wait_for(
            app.project
                .handle()
                .expect("handle")
                .save_copy(copy.clone(), false),
        );
        let reopened = ProjectHandle::temporary().expect("temporary project");
        let opened = wait_for(reopened.open(copy.clone(), 10, 1024));
        assert!(
            !opened.workspace.scope.enabled,
            "workbench edits made while the save was pending must be flushed"
        );
        let _ = std::fs::remove_file(&copy);
    }

    #[test]
    fn applying_an_opened_project_resets_selection_and_reports_status() {
        let mut app = PerseidApp::test_app();
        app.selected = Some(3);
        app.websocket_selected = Some(4);

        app.apply_opened_project(
            OpenedProject {
                http: Vec::new(),
                ws: Vec::new(),
                workspace: WorkspaceState::default(),
                http_total: 5,
                ws_total: 0,
            },
            Some(Path::new("/tmp/demo.perseid")),
        );
        assert!(app.selected.is_none());
        assert!(app.websocket_selected.is_none());
        let (text, error) = app.project.notice().expect("notice");
        assert!(!error);
        assert!(text.contains("Loaded the newest 0 of 5 exchanges"));

        app.apply_opened_project(
            OpenedProject {
                http: Vec::new(),
                ws: Vec::new(),
                workspace: WorkspaceState::default(),
                http_total: 0,
                ws_total: 0,
            },
            Some(Path::new("/tmp/demo.perseid")),
        );
        let (text, error) = app.project.notice().expect("notice");
        assert!(!error);
        assert!(text.contains("Opened /tmp/demo.perseid"));

        app.apply_opened_project(
            OpenedProject {
                http: Vec::new(),
                ws: Vec::new(),
                workspace: WorkspaceState::default(),
                http_total: 0,
                ws_total: 0,
            },
            None,
        );
        let (text, error) = app.project.notice().expect("notice");
        assert!(!error);
        assert!(text.contains("Started a new temporary project"));
    }

    #[test]
    fn failed_opens_keep_the_same_running_listener_workspace_and_project() {
        let mut app = PerseidApp::test_app();
        app.scope.insert("keep.example".into());
        app.start_proxy(app.notify.clone());
        let address = app.proxy.as_ref().unwrap().local_addr();
        let generation = app.project.status().unwrap().generation;
        let locked = temp_path("ui-locked");
        let corrupt = temp_path("ui-corrupt");
        let missing = temp_path("ui-missing");
        let holder = ProjectHandle::temporary().unwrap();
        wait_for(holder.save_as(locked.clone(), false));
        std::fs::write(&corrupt, b"not a redb file").unwrap();
        for path in [&missing, &locked, &corrupt] {
            app.begin_open_project(path.clone());
            poll_until_idle(&mut app);
            assert_eq!(app.project.status().unwrap().generation, generation);
            let proxy = app.proxy.as_ref().unwrap();
            assert!(proxy.is_running());
            assert_eq!(
                proxy.local_addr(),
                address,
                "failed open restarted the listener"
            );
            assert!(std::net::TcpStream::connect(address).is_ok());
            assert!(app.scope.contains("keep.example"));
            assert!(app.project.notice().unwrap().1);
            app.project.handle().unwrap().capture(&started_event(10));
            app.project.handle().unwrap().capture(&finished_event(10));
            wait_for(app.project.handle().unwrap().flush());
        }
        assert_eq!(app.project.status().unwrap().http_count, 3);
        assert!(!missing.exists());
        app.shutdown_project();
        drop(holder);
        std::fs::remove_file(locked).unwrap();
        std::fs::remove_file(corrupt).unwrap();
    }

    #[test]
    fn validated_open_switches_workspace_and_restarts_a_running_proxy() {
        let path = temp_path("ui-validated-open");
        let source = ProjectHandle::temporary().unwrap();
        source.capture(&started_event(1));
        source.capture(&finished_event(1));
        wait_for(source.save_copy(path.clone(), false));
        let mut app = PerseidApp::test_app();
        app.scope.insert("old.example".into());
        app.start_proxy(app.notify.clone());
        let generation = app.project.status().unwrap().generation;
        app.begin_open_project(path.clone());
        poll_until_idle(&mut app);
        assert!(app.project.status().unwrap().generation > generation);
        assert_eq!(app.current_project_path(), Some(path.clone()));
        assert!(app.scope.is_empty());
        assert_eq!(app.history.len(), 1);
        assert!(app.proxy.as_ref().unwrap().is_running());
        app.shutdown_project();
        drop(app);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn storage_window_applies_saved_limits_and_reports_retained_counts() {
        let ctx = egui::Context::default();
        let path = temp_path("ui-retention");
        let mut app = PerseidApp::test_app();
        let handle = app.project.handle().unwrap();
        wait_for(handle.save_as(path.clone(), false));
        for id in 1..=3 {
            for event in [started_event(id), finished_event(id)] {
                app.history.apply(handle.capture(&event).event.unwrap());
            }
        }
        wait_for(handle.flush());
        app.selected = Some(1);
        app.history.apply(started_event(99)); // not yet committed
        app.open_project_storage();
        app.project.storage_limits = Some(RetentionLimits::new(1, 0));
        assert!(click_window_text(&ctx, "Apply retention limits", |ctx| app
            .project_windows(ctx)));
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.project.retention_pending.is_some()
            || app.project.retention_sweep.is_some()
            || app.project.retention_seen.1 != 2
        {
            app.poll_project();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let status = app.project.status().unwrap();
        assert_eq!(status.retention, RetentionLimits::new(1, 0));
        assert_eq!(status.http_count, 1);
        assert_eq!(status.retention_evictions, 2);
        assert!(app.history.get(1).is_none());
        assert!(app.history.get(2).is_none());
        assert!(app.history.get(3).is_some());
        assert!(app.history.get(99).is_some());
        assert!(app.selected.is_none());
        assert_eq!(app.history.in_flight(), 1);
        app.shutdown_project();
        drop(app);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn project_status_widget_reports_lost_captures_and_evictions() {
        let mut app = PerseidApp::test_app();
        // A tiny record budget makes the third exchange evict the second.
        app.project = ProjectUi::new(10, 1024, RetentionLimits::new(1, 0));
        {
            let handle = app.project.handle().expect("handle");
            handle.capture(&started_event(1));
            // The exchange disappears before its completion arrives; the
            // writer counts the orphaned Finished event as a loss.
            wait_for(handle.clear_http());
            handle.capture(&finished_event(1));
            handle.capture(&started_event(2));
            handle.capture(&finished_event(2));
            handle.capture(&started_event(3));
            handle.capture(&finished_event(3));
            wait_for(handle.flush());
        }
        let status = app.project.status().expect("status");
        assert_eq!(status.event_loss_count, 1);
        assert_eq!(status.retention_evictions, 1);

        let output = frame(|ui| app.project_status_widget(ui));
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        assert!(
            texts
                .iter()
                .any(|text| text.contains("capture records incomplete")),
            "the status widget must report lost captures: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text.contains("old records evicted")),
            "the status widget must report retention evictions: {texts:?}"
        );
    }

    #[test]
    fn shutdown_project_flushes_the_writer() {
        let mut app = PerseidApp::test_app();
        let file = temp_path("ui-shutdown");
        wait_for(
            app.project
                .handle()
                .expect("handle")
                .save_as(file.clone(), false),
        );

        // A workbench change made just before shutdown must reach the file,
        // not only the in-memory project that is about to be dropped.
        app.scope_enabled = true;
        app.shutdown_project();
        drop(app); // like on_exit: the writer stops and releases the file

        let reopened = ProjectHandle::temporary().expect("temporary project");
        let opened = wait_for(reopened.open(file.clone(), 10, 1024));
        assert!(
            opened.workspace.scope.enabled,
            "shutdown must flush the pending workspace"
        );
        let _ = std::fs::remove_file(&file);
    }
}

#[cfg(test)]
mod switch_tests {
    use super::*;

    #[test]
    fn an_empty_temporary_project_switches_without_a_prompt() {
        let mut app = PerseidApp::test_app();
        assert!(!app.has_unsaved_temporary_work());

        app.request_project_switch(ProjectSwitch::New);

        assert!(app.project.switch_prompt.is_none());
        assert!(matches!(
            app.project.pending.as_slice(),
            [PendingProject::New { .. }]
        ));
    }

    #[test]
    fn unsaved_work_asks_before_a_switch_and_cancel_keeps_the_project() {
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;
        assert!(app.has_unsaved_temporary_work());

        app.request_project_switch(ProjectSwitch::New);
        assert!(app.project.switch_prompt.is_some());
        assert!(app.project.pending.is_empty());

        app.project.cancel_switch();
        assert!(app.project.switch_prompt.is_none());
        assert!(app.project.pending.is_empty());
        assert_eq!(
            app.project.status().expect("project status").mode,
            ProjectMode::Temporary
        );
        assert!(app.scope_enabled);
    }

    #[test]
    fn discarding_unsaved_work_performs_the_pending_switch() {
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;
        app.request_project_switch(ProjectSwitch::New);

        let request = app
            .project
            .take_switch_request()
            .expect("pending project switch");
        app.perform_project_switch(request);

        assert!(matches!(
            app.project.pending.as_slice(),
            [PendingProject::New { .. }]
        ));
    }

    #[test]
    fn cancelling_save_as_after_the_prompt_keeps_the_current_project() {
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;
        app.request_project_switch(ProjectSwitch::New);

        app.project.begin_switch_save();
        assert_eq!(app.project.switch_after_save, Some(ProjectSwitch::New));
        assert_eq!(
            app.project.dialog.as_ref().expect("save dialog").kind,
            ProjectDialogKind::SaveAs
        );

        app.project.close_dialog();
        assert!(app.project.switch_after_save.is_none());
        assert!(app.project.dialog.is_none());
        assert!(app.project.pending.is_empty());
        assert_eq!(
            app.project.status().expect("project status").mode,
            ProjectMode::Temporary
        );
    }

    #[test]
    fn a_failed_save_cancels_the_pending_switch_and_keeps_the_project_open() {
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(sender);
        app.project.pending.push(PendingProject::Save {
            copy: false,
            path: PathBuf::from("/nonexistent/demo.perseid"),
            receiver,
        });
        app.project.switch_after_save = Some(ProjectSwitch::New);

        app.poll_project();

        assert!(app.project.switch_after_save.is_none());
        assert!(app.project.pending.is_empty());
        assert_eq!(
            app.project.status().expect("project status").mode,
            ProjectMode::Temporary
        );
        assert!(
            app.project.notice().expect("save error notice").1,
            "the failed save must be reported as an error"
        );
    }

    #[test]
    fn a_successful_save_continues_the_pending_switch() {
        let mut app = PerseidApp::test_app();
        app.scope_enabled = true;
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Ok(())).expect("queue save result");
        app.project.pending.push(PendingProject::Save {
            copy: false,
            path: PathBuf::from("/tmp/demo.perseid"),
            receiver,
        });
        app.project.switch_after_save = Some(ProjectSwitch::New);

        app.poll_project();

        assert!(app.project.switch_after_save.is_none());
        assert!(matches!(
            app.project.pending.as_slice(),
            [PendingProject::New { .. }]
        ));
    }
}
