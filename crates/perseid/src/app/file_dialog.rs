//! In-app file browser used by the project open/save dialogs.
//!
//! The browser deliberately stays inside egui instead of depending on a
//! platform file picker. It supports project files and general file imports,
//! with direct folder navigation.

use std::fs;
use std::path::{Path, PathBuf};

use egui::{
    Align, Align2, Color32, CornerRadius, Key, Layout, Margin, Response, RichText, Sense, Stroke,
    TextEdit, Vec2,
};

use super::*;

const PROJECT_EXTENSION: &str = "perseid";
const LIST_HEIGHT: f32 = 350.0;
const TITLE_HEIGHT: f32 = 40.0;
const ROW_HEIGHT: f32 = 30.0;
const FIELD_HEIGHT: f32 = 28.0;
const LABEL_WIDTH: f32 = 94.0;
const ICON_BUTTON: f32 = 28.0;
const GRID_TILE_WIDTH: f32 = 112.0;
const GRID_TILE_HEIGHT: f32 = 82.0;

/// Width shared by every project file dialog.
pub(super) const MODAL_WIDTH: f32 = 760.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileDialogMode {
    Open,
    Save,
}

impl FileDialogMode {
    fn folder_label(self) -> &'static str {
        match self {
            Self::Open => "Look in:",
            Self::Save => "Save in:",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileViewMode {
    List,
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogIcon {
    Up,
    Home,
    Refresh,
    NewFolder,
    Eye,
    List,
    Grid,
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
    all_files: bool,
    directory: PathBuf,
    directory_input: String,
    file_name: String,
    entries: Vec<FileEntry>,
    selected: Option<PathBuf>,
    error: Option<String>,
    show_hidden: bool,
    view: FileViewMode,
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
        Self::with_filter(directory, file_name, mode, false)
    }

    /// Open a browser that shows all regular files, for importing data.
    pub(super) fn open_any_file(directory: PathBuf) -> Self {
        Self::with_filter(directory, "", FileDialogMode::Open, true)
    }

    fn with_filter(
        directory: PathBuf,
        file_name: impl Into<String>,
        mode: FileDialogMode,
        all_files: bool,
    ) -> Self {
        let home = home_directory();
        let mut dialog = Self {
            mode,
            all_files,
            directory: PathBuf::new(),
            directory_input: String::new(),
            file_name: file_name.into(),
            entries: Vec::new(),
            selected: None,
            error: None,
            show_hidden: false,
            view: FileViewMode::List,
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

    pub(super) fn chosen_path(&self) -> Result<PathBuf, String> {
        let name = self.file_name.trim();
        if name.is_empty() {
            return Err("Enter a file name.".to_string());
        }
        if let Some(path) = &self.selected {
            return Ok(path.clone());
        }
        Ok(self.directory.join(name))
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
        match read_entries(&directory, self.show_hidden, self.all_files) {
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
        match read_entries(&self.directory, self.show_hidden, self.all_files) {
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

    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        submit_label: &str,
        external_error: &mut Option<String>,
    ) -> FileDialogAction {
        let mut action = FileDialogAction::None;
        let original_directory = self.directory.clone();
        let original_file_name = self.file_name.clone();

        let mut navigate = None;
        let mut refresh = false;
        let mut go = false;
        self.location_row(ui, &mut navigate, &mut refresh, &mut go);

        if go {
            self.navigate_input();
        }
        if let Some(directory) = navigate {
            self.navigate_to(directory);
        }
        if refresh {
            self.reload();
        }

        if self.new_folder.is_some() {
            ui.add_space(8.0);
            self.new_folder_row(ui);
        }

        ui.add_space(10.0);
        let (open_directory, choose_file, submit_file) = self.file_list(ui);
        if let Some(directory) = open_directory {
            self.navigate_to(directory);
        }
        if let Some(path) = choose_file {
            self.choose_file(path);
        }
        if submit_file {
            action = FileDialogAction::Submit;
        }

        ui.add_space(12.0);
        self.file_name_row(ui, &mut action);
        ui.add_space(8.0);
        self.file_type_row(ui);

        if self.directory != original_directory || self.file_name != original_file_name {
            *external_error = None;
        }
        if let Some(error) = external_error.as_deref().or(self.error.as_deref()) {
            ui.add_space(6.0);
            ui.label(RichText::new(error).small().color(theme::RED));
        }

        ui.add_space(10.0);
        egui::Sides::new().height(30.0).show(
            ui,
            |_ui| {},
            |ui| {
                if ui
                    .add_sized([88.0, 30.0], egui::Button::new("Cancel"))
                    .clicked()
                {
                    action = FileDialogAction::Cancel;
                }
                if ui
                    .add_sized([88.0, 30.0], egui::Button::new(submit_label))
                    .clicked()
                {
                    action = FileDialogAction::Submit;
                }
            },
        );

        action
    }

    fn location_row(
        &mut self,
        ui: &mut egui::Ui,
        navigate: &mut Option<PathBuf>,
        refresh: &mut bool,
        go: &mut bool,
    ) {
        ui.horizontal(|ui| {
            ui.add_sized(
                [LABEL_WIDTH, FIELD_HEIGHT],
                egui::Label::new(RichText::new(self.mode.folder_label()).color(theme::TEXT)),
            );

            let spacing = ui.spacing().item_spacing.x;
            let toolbar_width = 7.0 * ICON_BUTTON + 8.0 * spacing + 6.0;
            let field_width = (ui.available_width() - toolbar_width).max(180.0);
            egui::Frame::NONE
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(2))
                .inner_margin(Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.set_width(field_width - 12.0);
                    ui.horizontal(|ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_folder(ui.painter(), icon_rect.shrink(1.0), theme::BLUE);
                        let response = ui
                            .add(
                                TextEdit::singleline(&mut self.directory_input)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("/path/to/projects"),
                            )
                            .on_hover_text("Type a folder path and press Enter");
                        *go = response.lost_focus()
                            && ui.input(|input| input.key_pressed(Key::Enter));
                    });
                });

            ui.add_space(2.0);
            let parent = self.directory.parent().map(Path::to_path_buf);
            if ui
                .add_enabled_ui(parent.is_some(), |ui| {
                    icon_button(ui, DialogIcon::Up, false, "Parent folder")
                })
                .inner
                .clicked()
                && let Some(parent) = parent
            {
                *navigate = Some(parent);
            }
            if ui
                .add_enabled_ui(self.home.is_some(), |ui| {
                    icon_button(ui, DialogIcon::Home, false, "Home folder")
                })
                .inner
                .clicked()
                && let Some(home) = self.home.clone()
            {
                *navigate = Some(home);
            }
            if icon_button(ui, DialogIcon::Refresh, false, "Refresh").clicked() {
                *refresh = true;
            }
            if icon_button(ui, DialogIcon::NewFolder, false, "New folder").clicked() {
                self.new_folder = Some(String::new());
            }
            if icon_button(
                ui,
                DialogIcon::Eye,
                self.show_hidden,
                if self.show_hidden {
                    "Hide hidden files"
                } else {
                    "Show hidden files"
                },
            )
            .clicked()
            {
                self.show_hidden = !self.show_hidden;
                *refresh = true;
            }
            ui.add_space(6.0);
            if icon_button(
                ui,
                DialogIcon::List,
                self.view == FileViewMode::List,
                "List view",
            )
            .clicked()
            {
                self.view = FileViewMode::List;
            }
            if icon_button(
                ui,
                DialogIcon::Grid,
                self.view == FileViewMode::Grid,
                "Grid view",
            )
            .clicked()
            {
                self.view = FileViewMode::Grid;
            }
        });
    }

    fn new_folder_row(&mut self, ui: &mut egui::Ui) {
        let mut name = self.new_folder.clone().unwrap_or_default();
        let mut create = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add_sized(
                [LABEL_WIDTH, FIELD_HEIGHT],
                egui::Label::new(RichText::new("New folder:").color(theme::TEXT)),
            );
            let width = (ui.available_width() - 184.0).max(160.0);
            let response = ui.add_sized(
                [width, FIELD_HEIGHT],
                TextEdit::singleline(&mut name).hint_text("folder-name"),
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

    fn file_name_row(&mut self, ui: &mut egui::Ui, action: &mut FileDialogAction) {
        ui.horizontal(|ui| {
            ui.add_sized(
                [LABEL_WIDTH, FIELD_HEIGHT],
                egui::Label::new(RichText::new("File Name:").color(theme::TEXT)),
            );
            let width = ui.available_width();
            let response = ui
                .add_sized(
                    [width, FIELD_HEIGHT],
                    TextEdit::singleline(&mut self.file_name)
                        .hint_text(if self.all_files {
                            "payloads.txt"
                        } else {
                            "project.perseid"
                        })
                        .margin(egui::Margin::symmetric(6, 4)),
                )
                .on_hover_text(if self.all_files {
                    "Choose a file to load"
                } else {
                    "The .perseid extension is added when omitted"
                });
            if response.changed() {
                self.selected = None;
                self.error = None;
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                *action = FileDialogAction::Submit;
            }
        });
    }

    fn file_type_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_sized(
                [LABEL_WIDTH, FIELD_HEIGHT],
                egui::Label::new(RichText::new("Files of Type:").color(theme::TEXT)),
            );
            let width = ui.available_width();
            let file_type = if self.all_files {
                "All files (*)"
            } else {
                "Perseid projects (*.perseid)"
            };
            egui::ComboBox::from_id_salt("perseid.project.file_type")
                .selected_text(file_type)
                .width(width)
                .show_ui(ui, |ui| {
                    let _ = ui.selectable_label(true, file_type);
                });
        });
    }

    fn file_list(&self, ui: &mut egui::Ui) -> (Option<PathBuf>, Option<PathBuf>, bool) {
        let mut open_directory = None;
        let mut choose_file = None;
        let mut submit_file = false;
        egui::Frame::NONE
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(2))
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(LIST_HEIGHT)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.set_min_height(LIST_HEIGHT);
                        if self.entries.is_empty() {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(if self.all_files {
                                    "No files in this folder."
                                } else {
                                    "No project files in this folder."
                                })
                                .color(theme::TEXT_MUTED),
                            );
                            return;
                        }
                        match self.view {
                            FileViewMode::List => {
                                for entry in &self.entries {
                                    let selected =
                                        self.selected.as_deref() == Some(entry.path.as_path());
                                    let response = file_row(ui, entry, selected);
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
                            }
                            FileViewMode::Grid => {
                                ui.horizontal_wrapped(|ui| {
                                    for entry in &self.entries {
                                        let selected =
                                            self.selected.as_deref() == Some(entry.path.as_path());
                                        let response = grid_entry(ui, entry, selected);
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
                            }
                        }
                    });
            });
        (open_directory, choose_file, submit_file)
    }

    fn choose_file(&mut self, path: PathBuf) {
        self.selected = Some(path.clone());
        if let Some(name) = path.file_name() {
            self.file_name = name.to_string_lossy().into_owned();
        }
        self.error = None;
    }
}

pub(super) fn modal_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(6))
        .shadow(egui::epaint::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(48),
        })
}

/// Draw the custom title bar shared by the project dialogs. Returns `true`
/// when the user clicks the close button.
pub(super) fn title_bar(
    ui: &mut egui::Ui,
    title: &str,
    icon: Option<&egui::TextureHandle>,
) -> bool {
    let mut close = false;
    let width = ui.available_width();
    let response = ui.allocate_ui_with_layout(
        Vec2::new(width, TITLE_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(12.0);
            if let Some(icon) = icon {
                ui.add(egui::Image::from_texture(icon).fit_to_exact_size(Vec2::splat(22.0)));
            } else {
                ui.add_space(22.0);
            }
            ui.add_space(4.0);
            ui.label(RichText::new(title).size(15.0).strong().color(theme::TEXT));
            egui::Sides::new().height(TITLE_HEIGHT).show(
                ui,
                |_ui| {},
                |ui| {
                    if close_button(ui).clicked() {
                        close = true;
                    }
                    ui.add_space(2.0);
                },
            );
        },
    );
    let rect = response.response.rect;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 0.5),
            egui::pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, theme::BORDER),
    );
    close
}

fn close_button(ui: &mut egui::Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(3), theme::ACCENT_WASH);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                CornerRadius::same(3),
                Stroke::new(1.0, theme::ACCENT_DARK),
                egui::StrokeKind::Inside,
            );
        }
        let color = if response.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_MUTED
        };
        let center = rect.center();
        let arm = 5.0;
        let stroke = Stroke::new(1.4, color);
        ui.painter().line_segment(
            [center + Vec2::new(-arm, -arm), center + Vec2::new(arm, arm)],
            stroke,
        );
        ui.painter().line_segment(
            [center + Vec2::new(arm, -arm), center + Vec2::new(-arm, arm)],
            stroke,
        );
    }
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), "Close"));
    response
        .on_hover_text("Close")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn icon_button(ui: &mut egui::Ui, icon: DialogIcon, selected: bool, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ICON_BUTTON), Sense::click());
    if ui.is_rect_visible(rect) {
        let enabled = ui.is_enabled();
        let fill = if selected {
            Some(theme::ACCENT_SOFT)
        } else if response.hovered() {
            Some(theme::ACCENT_WASH)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, CornerRadius::same(3), fill);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                CornerRadius::same(3),
                Stroke::new(1.0, theme::ACCENT_DARK),
                egui::StrokeKind::Inside,
            );
        }
        let color = if !enabled {
            theme::BORDER
        } else if selected {
            theme::ACCENT_DARK
        } else {
            theme::TEXT_MUTED
        };
        paint_dialog_icon(ui.painter(), rect.shrink(7.5), icon, color);
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, tooltip)
    });
    response
        .on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn file_row(ui: &mut egui::Ui, entry: &FileEntry, selected: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(2), theme::SELECTED);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(2), theme::ACCENT_WASH);
        }
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            Vec2::splat(16.0),
        );
        if entry.is_dir {
            paint_folder(ui.painter(), icon_rect, theme::BLUE);
        } else {
            paint_file(ui.painter(), icon_rect, theme::TEXT_MUTED);
        }
        ui.painter().text(
            egui::pos2(rect.left() + 34.0, rect.center().y),
            Align2::LEFT_CENTER,
            entry_label(entry),
            egui::FontId::proportional(12.5),
            theme::TEXT,
        );
    }
    let label = entry_label(entry);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, &label)
    });
    response.on_hover_text(entry.path.display().to_string())
}

fn grid_entry(ui: &mut egui::Ui, entry: &FileEntry, selected: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(GRID_TILE_WIDTH, GRID_TILE_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter()
                .rect_filled(rect.shrink(1.0), CornerRadius::same(4), theme::SELECTED);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect.shrink(1.0), CornerRadius::same(4), theme::ACCENT_WASH);
        }
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + 26.0),
            Vec2::new(30.0, 26.0),
        );
        if entry.is_dir {
            paint_folder(ui.painter(), icon_rect, theme::BLUE);
        } else {
            paint_file(ui.painter(), icon_rect, theme::TEXT_MUTED);
        }

        let galley = ui.painter().layout(
            entry_label(entry),
            egui::FontId::proportional(11.5),
            theme::TEXT,
            rect.width() - 12.0,
        );
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 6.0, rect.top() + 46.0),
            egui::pos2(rect.right() - 6.0, rect.bottom() - 2.0),
        );
        ui.painter()
            .with_clip_rect(text_rect.intersect(ui.clip_rect()))
            .galley(text_rect.min, galley, theme::TEXT);
    }
    let label = entry_label(entry);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, &label)
    });
    response.on_hover_text(entry.path.display().to_string())
}

fn entry_label(entry: &FileEntry) -> String {
    if entry.is_dir {
        format!("{}/", entry.name)
    } else {
        entry.name.clone()
    }
}

fn paint_dialog_icon(painter: &egui::Painter, rect: egui::Rect, icon: DialogIcon, color: Color32) {
    let center = rect.center();
    let stroke = Stroke::new(1.3, color);
    match icon {
        DialogIcon::Up => {
            painter.line_segment(
                [center + Vec2::new(0.0, 5.0), center + Vec2::new(0.0, -5.0)],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-4.0, -1.0),
                    center + Vec2::new(0.0, -5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(4.0, -1.0), center + Vec2::new(0.0, -5.0)],
                stroke,
            );
        }
        DialogIcon::Home => {
            painter.add(egui::Shape::line(
                vec![
                    center + Vec2::new(-6.0, 0.0),
                    center + Vec2::new(0.0, -5.5),
                    center + Vec2::new(6.0, 0.0),
                ],
                stroke,
            ));
            painter.line_segment(
                [center + Vec2::new(-4.5, 0.0), center + Vec2::new(-4.5, 5.0)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(4.5, 0.0), center + Vec2::new(4.5, 5.0)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-4.5, 5.0), center + Vec2::new(4.5, 5.0)],
                stroke,
            );
        }
        DialogIcon::Refresh => {
            let radius = 5.0;
            let start = -std::f32::consts::FRAC_PI_2 + 0.5;
            let end = start + std::f32::consts::PI * 1.55;
            let points: Vec<_> = (0..=18)
                .map(|index| {
                    let angle = start + (end - start) * index as f32 / 18.0;
                    center + Vec2::new(angle.cos() * radius, angle.sin() * radius)
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
            let tip = center + Vec2::new(end.cos() * radius, end.sin() * radius);
            painter.line_segment([tip, tip + Vec2::new(-2.8, -1.8)], stroke);
            painter.line_segment([tip, tip + Vec2::new(0.2, -3.2)], stroke);
        }
        DialogIcon::NewFolder => {
            paint_folder(painter, rect.translate(Vec2::new(-1.0, -1.0)), color);
            let plus = rect.right_bottom() + Vec2::new(-1.0, -1.0);
            painter.line_segment(
                [plus + Vec2::new(-3.0, 0.0), plus + Vec2::new(3.0, 0.0)],
                stroke,
            );
            painter.line_segment(
                [plus + Vec2::new(0.0, -3.0), plus + Vec2::new(0.0, 3.0)],
                stroke,
            );
        }
        DialogIcon::Eye => {
            let points: Vec<_> = (0..=24)
                .map(|index| {
                    let angle = index as f32 / 24.0 * std::f32::consts::TAU;
                    center + Vec2::new(angle.cos() * 6.0, angle.sin() * 3.5)
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
            painter.circle_filled(center, 1.8, color);
        }
        DialogIcon::List => {
            for row in -1..=1 {
                let y = center.y + row as f32 * 4.0;
                painter.circle_filled(egui::pos2(center.x - 6.0, y), 1.0, color);
                painter.line_segment(
                    [egui::pos2(center.x - 3.0, y), egui::pos2(center.x + 6.0, y)],
                    stroke,
                );
            }
        }
        DialogIcon::Grid => {
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    painter.rect_filled(
                        egui::Rect::from_center_size(
                            center + Vec2::new(x * 3.3, y * 3.3),
                            Vec2::splat(4.0),
                        ),
                        CornerRadius::same(1),
                        color,
                    );
                }
            }
        }
    }
}

fn paint_folder(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let tab_height = (rect.height() * 0.24).max(2.0);
    let tab_width = rect.width() * 0.38;
    let shoulder = rect.width() * 0.46;
    let points = vec![
        egui::pos2(rect.left(), rect.bottom()),
        egui::pos2(rect.left(), rect.top() + tab_height),
        egui::pos2(rect.left() + tab_width, rect.top() + tab_height),
        egui::pos2(rect.left() + shoulder, rect.top()),
        egui::pos2(rect.right(), rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
    ];
    painter.add(egui::Shape::closed_line(points, Stroke::new(1.2, color)));
}

fn paint_file(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    painter.rect_stroke(
        rect,
        CornerRadius::same(1),
        stroke,
        egui::StrokeKind::Inside,
    );
    let fold = (rect.width() * 0.34).min(rect.height() * 0.34);
    painter.line_segment(
        [
            egui::pos2(rect.right() - fold, rect.top()),
            egui::pos2(rect.right(), rect.top() + fold),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(rect.right() - fold, rect.top()),
            egui::pos2(rect.right() - fold, rect.top() + fold),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(rect.right() - fold, rect.top() + fold),
            egui::pos2(rect.right(), rect.top() + fold),
        ],
        stroke,
    );
}

fn read_entries(
    directory: &Path,
    show_hidden: bool,
    all_files: bool,
) -> Result<Vec<FileEntry>, String> {
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
        if !is_dir && !(metadata.is_file() && (all_files || is_project_file(&path))) {
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
    fn all_file_browser_lists_payloads_and_preserves_selected_paths() {
        let root = TempDir::new("all-files");
        fs::write(root.path.join("words.txt"), b"words").unwrap();
        fs::write(root.path.join("wordlist"), b"words").unwrap();
        fs::write(root.path.join(".secret"), b"hidden").unwrap();
        fs::create_dir(root.path.join("folder")).unwrap();
        let mut dialog = FileDialog::open_any_file(root.path.clone());
        assert_eq!(names(&dialog), ["folder", "wordlist", "words.txt"]);
        assert!(dialog.chosen_path().is_err());
        dialog.choose_file(root.path.join("words.txt"));
        assert_eq!(dialog.chosen_path().unwrap(), root.path.join("words.txt"));
        dialog.set_file_name("wordlist");
        dialog.selected = None;
        assert_eq!(dialog.chosen_path().unwrap(), root.path.join("wordlist"));
        dialog.show_hidden = true;
        dialog.reload();
        assert!(names(&dialog).contains(&".secret".to_string()));
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
