//! In-app file browser used by the project open/save dialogs.
//!
//! The browser deliberately stays inside egui instead of depending on a
//! platform file picker. It lists folders and `.perseid` files, supports direct
//! folder navigation, and keeps the project-file extension consistent with the
//! rest of the project subsystem.

use std::fs;
use std::path::{Path, PathBuf};

use egui::{Key, RichText, TextEdit};

use super::*;

const PROJECT_EXTENSION: &str = "perseid";
const LIST_HEIGHT: f32 = 250.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileDialogMode {
    Open,
    Save,
}

impl FileDialogMode {
    fn file_label(self) -> &'static str {
        match self {
            Self::Open => "Project file:",
            Self::Save => "File name:",
        }
    }
}

#[derive(Clone, Debug)]
struct FileEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

#[derive(Clone, Debug)]
pub(super) struct FileDialog {
    mode: FileDialogMode,
    directory: PathBuf,
    directory_input: String,
    file_name: String,
    entries: Vec<FileEntry>,
    selected: Option<PathBuf>,
    error: Option<String>,
    show_hidden: bool,
    new_folder: Option<String>,
    home: Option<PathBuf>,
}

pub(super) enum FileDialogAction {
    None,
    Submit,
    Cancel,
}

impl FileDialog {
    pub(super) fn new(
        directory: PathBuf,
        file_name: impl Into<String>,
        mode: FileDialogMode,
    ) -> Self {
        let home = home_directory();
        let mut dialog = Self {
            mode,
            directory: PathBuf::new(),
            directory_input: String::new(),
            file_name: file_name.into(),
            entries: Vec::new(),
            selected: None,
            error: None,
            show_hidden: false,
            new_folder: None,
            home,
        };
        dialog.navigate_to(directory);
        dialog
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(super) fn file_name(&self) -> &str {
        &self.file_name
    }

    #[cfg(test)]
    pub(super) fn set_file_name(&mut self, name: impl Into<String>) {
        self.file_name = name.into();
    }

    #[cfg(test)]
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Point the browser at a full file path. The parent folder is listed and
    /// the file is selected when it exists.
    pub(super) fn set_path(&mut self, path: &Path) {
        let path = if path.is_relative() {
            std::env::current_dir().unwrap_or_default().join(path)
        } else {
            path.to_path_buf()
        };
        let (directory, file_name) = if path.is_dir() {
            (path.clone(), None)
        } else {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.directory.clone());
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            (parent, name)
        };
        self.navigate_to(directory);
        if let Some(name) = file_name {
            self.selected = Some(path);
            self.file_name = name;
        }
    }

    fn navigate_to(&mut self, directory: PathBuf) {
        match read_entries(&directory, self.show_hidden) {
            Ok(entries) => {
                self.directory = directory;
                self.directory_input = self.directory.display().to_string();
                self.entries = entries;
                self.selected = None;
                self.error = None;
            }
            Err(err) => {
                self.error = Some(err);
            }
        }
    }

    fn navigate_input(&mut self) {
        let input = self.directory_input.trim();
        if input.is_empty() {
            return;
        }
        let mut path = PathBuf::from(expand_tilde(input));
        if path.is_relative() {
            path = self.directory.join(path);
        }
        if path.is_file() {
            self.set_path(&path);
        } else {
            self.navigate_to(path);
        }
    }

    fn reload(&mut self) {
        match read_entries(&self.directory, self.show_hidden) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(err) => {
                self.entries.clear();
                self.error = Some(err);
            }
        }
    }

    fn create_folder(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.error = Some("Enter a folder name.".to_string());
            return;
        }
        if name == "." || name == ".." || name.contains(['/', '\\']) {
            self.error = Some("Folder names cannot contain path separators.".to_string());
            return;
        }
        let path = self.directory.join(name);
        match fs::create_dir(&path) {
            Ok(()) => self.navigate_to(path),
            Err(err) => {
                self.error = Some(format!("Could not create {}: {err}", path.display()));
            }
        }
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui, submit_label: &str) -> FileDialogAction {
        let mut action = FileDialogAction::None;
        let mut navigate = None;
        let mut refresh = false;

        ui.horizontal(|ui| {
            let parent = self.directory.parent().map(Path::to_path_buf);
            if ui
                .add_enabled(parent.is_some(), egui::Button::new("Up"))
                .clicked()
                && let Some(parent) = parent
            {
                navigate = Some(parent);
            }
            if ui
                .add_enabled(self.home.is_some(), egui::Button::new("Home"))
                .clicked()
                && let Some(home) = self.home.clone()
            {
                navigate = Some(home);
            }
            if ui.button("Refresh").clicked() {
                refresh = true;
            }
            if ui.checkbox(&mut self.show_hidden, "Show hidden").changed() {
                refresh = true;
            }
            if ui.button("New folder").clicked() {
                self.new_folder = Some(String::new());
            }
        });

        ui.add_space(2.0);
        let mut go = false;
        ui.horizontal(|ui| {
            ui.label("Folder:");
            let response = ui.add(
                TextEdit::singleline(&mut self.directory_input)
                    .desired_width(f32::INFINITY)
                    .hint_text("/path/to/projects"),
            );
            go = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            if ui.button("Go").clicked() {
                go = true;
            }
        });
        if go {
            self.navigate_input();
        }
        if let Some(directory) = navigate {
            self.navigate_to(directory);
        }
        if refresh {
            self.reload();
        }

        ui.add_space(4.0);
        if self.new_folder.is_some() {
            let mut name = self.new_folder.clone().unwrap_or_default();
            let mut create = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                ui.label("New folder:");
                let response = ui.add(
                    TextEdit::singleline(&mut name)
                        .desired_width(220.0)
                        .hint_text("folder-name"),
                );
                create = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                if ui.button("Create").clicked() {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if cancel {
                self.new_folder = None;
            } else if create {
                self.new_folder = None;
                self.create_folder(&name);
            } else {
                self.new_folder = Some(name);
            }
        }

        let mut open_directory = None;
        let mut choose_file = None;
        let mut submit_file = false;
        egui::ScrollArea::vertical()
            .max_height(LIST_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.set_min_height(LIST_HEIGHT);
                if self.entries.is_empty() {
                    ui.label(
                        RichText::new("No project files in this folder.").color(theme::TEXT_MUTED),
                    );
                }
                for entry in &self.entries {
                    let label = if entry.is_dir {
                        RichText::new(format!("{}/", entry.name)).color(theme::BLUE)
                    } else {
                        RichText::new(&entry.name)
                    };
                    let selected = self.selected.as_deref() == Some(entry.path.as_path());
                    let response = ui
                        .selectable_label(selected, label)
                        .on_hover_text(entry.path.display().to_string());
                    if response.clicked() {
                        if entry.is_dir {
                            open_directory = Some(entry.path.clone());
                        } else {
                            choose_file = Some(entry.path.clone());
                        }
                    }
                    if response.double_clicked() && !entry.is_dir {
                        choose_file = Some(entry.path.clone());
                        submit_file = true;
                    }
                }
            });
        if let Some(directory) = open_directory {
            self.navigate_to(directory);
        }
        if let Some(path) = choose_file {
            self.selected = Some(path.clone());
            if let Some(name) = path.file_name() {
                self.file_name = name.to_string_lossy().into_owned();
            }
            self.error = None;
        }
        if submit_file {
            action = FileDialogAction::Submit;
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(self.mode.file_label());
            let response = ui.add(
                TextEdit::singleline(&mut self.file_name)
                    .desired_width(f32::INFINITY)
                    .hint_text("project.perseid"),
            );
            if response.changed() {
                self.selected = None;
                self.error = None;
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                action = FileDialogAction::Submit;
            }
        });
        ui.label(
            RichText::new("Project files use the .perseid extension; it is added when omitted.")
                .small()
                .color(theme::TEXT_MUTED),
        );

        if let Some(error) = &self.error {
            ui.add_space(2.0);
            ui.label(RichText::new(error).small().color(theme::RED));
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(submit_label).clicked() {
                action = FileDialogAction::Submit;
            }
            if ui.button("Cancel").clicked() {
                action = FileDialogAction::Cancel;
            }
        });

        action
    }
}

fn read_entries(directory: &Path, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
    let read = fs::read_dir(directory)
        .map_err(|err| format!("Could not open {}: {err}", directory.display()))?;
    let mut entries = Vec::new();
    for entry in read {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                log::warn!("could not read an entry in {}: {err}", directory.display());
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => {
                log::warn!("could not inspect {}: {err}", path.display());
                continue;
            }
        };
        let is_dir = metadata.is_dir();
        if !is_dir && !is_project_file(&path) {
            continue;
        }
        entries.push(FileEntry { name, path, is_dir });
    }
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(entries)
}

fn is_project_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PROJECT_EXTENSION))
}

fn expand_tilde(input: &str) -> String {
    let input = input.trim();
    if input == "~" {
        return home_directory()
            .map(|home| home.display().to_string())
            .unwrap_or_else(|| input.to_string());
    }
    if let Some(rest) = input.strip_prefix("~/")
        && let Some(home) = home_directory()
    {
        return home.join(rest).display().to_string();
    }
    input.to_string()
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "perseid-file-dialog-{label}-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("create temp directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn names(dialog: &FileDialog) -> Vec<String> {
        dialog
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    }

    #[test]
    fn entries_put_folders_first_and_only_list_projects() {
        let root = TempDir::new("listing");
        fs::create_dir(root.path.join("z-folder")).expect("folder");
        fs::create_dir(root.path.join("a-folder")).expect("folder");
        fs::write(root.path.join("session.perseid"), b"project").expect("project");
        fs::write(root.path.join("notes.txt"), b"text").expect("text");
        fs::write(root.path.join(".hidden.perseid"), b"project").expect("hidden");

        let dialog = FileDialog::new(root.path.clone(), "", FileDialogMode::Open);

        assert_eq!(names(&dialog), ["a-folder", "z-folder", "session.perseid"]);
    }

    #[test]
    fn hidden_entries_can_be_shown() {
        let root = TempDir::new("hidden");
        fs::write(root.path.join(".hidden.perseid"), b"project").expect("hidden");

        let mut dialog = FileDialog::new(root.path.clone(), "", FileDialogMode::Open);
        assert!(names(&dialog).is_empty());

        dialog.show_hidden = true;
        dialog.reload();
        assert_eq!(names(&dialog), [".hidden.perseid"]);
    }

    #[test]
    fn navigating_to_a_missing_folder_reports_an_error_and_keeps_the_listing() {
        let root = TempDir::new("missing");
        fs::write(root.path.join("session.perseid"), b"project").expect("project");
        let mut dialog = FileDialog::new(root.path.clone(), "", FileDialogMode::Open);

        dialog.navigate_to(root.path.join("missing"));

        assert_eq!(dialog.directory(), root.path);
        assert_eq!(names(&dialog), ["session.perseid"]);
        assert!(
            dialog
                .error()
                .is_some_and(|error| error.contains("Could not open"))
        );
    }

    #[test]
    fn setting_a_file_path_navigates_to_its_parent_and_selects_it() {
        let root = TempDir::new("select");
        let file = root.path.join("session.perseid");
        fs::write(&file, b"project").expect("project");
        let mut dialog = FileDialog::new(
            std::env::temp_dir(),
            "fallback.perseid",
            FileDialogMode::Save,
        );

        dialog.set_path(&file);

        assert_eq!(dialog.directory(), root.path);
        assert_eq!(dialog.file_name(), "session.perseid");
        assert_eq!(dialog.selected.as_deref(), Some(file.as_path()));
    }

    #[test]
    fn folder_names_with_separators_are_rejected() {
        let root = TempDir::new("folder-name");
        let mut dialog = FileDialog::new(root.path.clone(), "", FileDialogMode::Save);

        dialog.create_folder("../escape");

        assert!(dialog.new_folder.is_none());
        assert!(
            dialog
                .error()
                .is_some_and(|error| error.contains("path separators"))
        );
        assert!(!root.path.join("escape").exists());
    }

    #[test]
    fn creating_a_folder_enters_it() {
        let root = TempDir::new("create-folder");
        let mut dialog = FileDialog::new(root.path.clone(), "", FileDialogMode::Save);

        dialog.create_folder("assessment");

        assert_eq!(dialog.directory(), root.path.join("assessment"));
        assert!(dialog.directory().is_dir());
    }
}
