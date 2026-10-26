//! Burp-style HTTP history display filter, toolbar, and settings modal.
//!
//! A display filter never deletes captured exchanges: it only decides which
//! rows appear in the history table. The toolbar keeps the most useful state
//! visible, while the modal mirrors Burp's request, MIME, status, search, and
//! file-extension groups.

use std::collections::BTreeSet;

use egui::{
    Align2, Color32, CornerRadius, FontId, Margin, Rect, Response, RichText, Sense, Stroke, Vec2,
};
use regex::{Regex, RegexBuilder};

use super::file_dialog;
use super::util::{mime_type, request_extension, request_uses_tls};
use super::*;
use crate::history::HistoryItem;
use crate::project::HistoryFilterWorkspace;
use crate::site_map;
use crate::theme;

const FILTER_BAR_HEIGHT: f32 = 24.0;
const FILTER_SWITCH_WIDTH: f32 = 30.0;
const FILTER_SWITCH_HEIGHT: f32 = 16.0;
const FILTER_MODAL_WIDTH: f32 = 940.0;

/// Burp-style display filter for Proxy > HTTP history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HistoryFilter {
    enabled: bool,
    in_scope_only: bool,
    hide_without_response: bool,
    parameterized_only: bool,
    show_html: bool,
    show_script: bool,
    show_xml: bool,
    show_css: bool,
    show_other_text: bool,
    show_images: bool,
    show_flash: bool,
    show_other_binary: bool,
    show_2xx: bool,
    show_3xx: bool,
    show_4xx: bool,
    show_5xx: bool,
    search: String,
    search_regex: bool,
    search_case_sensitive: bool,
    search_negative: bool,
    extension_show_only_enabled: bool,
    extension_show_only: String,
    extension_hide_enabled: bool,
    extension_hide: String,
}

impl Default for HistoryFilter {
    fn default() -> Self {
        Self::SHOW_ALL
    }
}

impl HistoryFilter {
    /// A filter that hides nothing.
    pub(super) const SHOW_ALL: Self = Self {
        enabled: true,
        in_scope_only: false,
        hide_without_response: false,
        parameterized_only: false,
        show_html: true,
        show_script: true,
        show_xml: true,
        show_css: true,
        show_other_text: true,
        show_images: true,
        show_flash: true,
        show_other_binary: true,
        show_2xx: true,
        show_3xx: true,
        show_4xx: true,
        show_5xx: true,
        search: String::new(),
        search_regex: false,
        search_case_sensitive: false,
        search_negative: false,
        extension_show_only_enabled: false,
        extension_show_only: String::new(),
        extension_hide_enabled: false,
        extension_hide: String::new(),
    };

    pub(super) fn is_show_all(&self) -> bool {
        *self == Self::SHOW_ALL
    }

    /// A draft that hides every response type. Request-level restrictions and
    /// the search/extension text stay as they were, matching Burp's Hide all.
    fn hide_all(&self) -> Self {
        Self {
            enabled: true,
            hide_without_response: true,
            show_html: false,
            show_script: false,
            show_xml: false,
            show_css: false,
            show_other_text: false,
            show_images: false,
            show_flash: false,
            show_other_binary: false,
            show_2xx: false,
            show_3xx: false,
            show_4xx: false,
            show_5xx: false,
            ..self.clone()
        }
    }

    /// A short sentence for the toolbar. The filter bar is the only place
    /// users see the current state without opening the modal.
    fn summary(&self) -> String {
        if !self.enabled {
            return "Filter off — showing all items".to_string();
        }
        if self.is_show_all() {
            return "Showing all items".to_string();
        }

        let search = HistorySearch::new(self);
        if search.invalid {
            return "Invalid regular expression in the filter search".to_string();
        }

        let mut clauses = Vec::new();
        if self.in_scope_only {
            clauses.push("showing only in-scope items".to_string());
        }
        if self.hide_without_response {
            clauses.push("hiding items without responses".to_string());
        }
        if self.parameterized_only {
            clauses.push("showing only parameterized requests".to_string());
        }

        let mut hidden_mime = Vec::new();
        if !self.show_html {
            hidden_mime.push("HTML");
        }
        if !self.show_script {
            hidden_mime.push("script");
        }
        if !self.show_xml {
            hidden_mime.push("XML");
        }
        if !self.show_css {
            hidden_mime.push("CSS");
        }
        if !self.show_other_text {
            hidden_mime.push("other text");
        }
        if !self.show_images {
            hidden_mime.push("image");
        }
        if !self.show_flash {
            hidden_mime.push("Flash");
        }
        if !self.show_other_binary {
            hidden_mime.push("other binary");
        }
        if !hidden_mime.is_empty() {
            clauses.push(format!("hiding {} content", join_words(&hidden_mime)));
        }

        let mut hidden_status = Vec::new();
        if !self.show_2xx {
            hidden_status.push("2xx");
        }
        if !self.show_3xx {
            hidden_status.push("3xx");
        }
        if !self.show_4xx {
            hidden_status.push("4xx");
        }
        if !self.show_5xx {
            hidden_status.push("5xx");
        }
        if !hidden_status.is_empty() {
            clauses.push(format!("hiding {} responses", join_words(&hidden_status)));
        }

        if !self.search.trim().is_empty() {
            let verb = if self.search_negative {
                "excluding"
            } else {
                "searching for"
            };
            clauses.push(format!("{verb} \"{}\"", truncate_summary(&self.search)));
        }

        let show_only = extension_tokens(&self.extension_show_only);
        if self.extension_show_only_enabled && !show_only.is_empty() {
            clauses.push(format!("showing only {}", extension_words(&show_only)));
        }
        let hidden_extension = extension_tokens(&self.extension_hide);
        if self.extension_hide_enabled && !hidden_extension.is_empty() {
            clauses.push(format!("hiding {}", extension_words(&hidden_extension)));
        }

        if clauses.is_empty() {
            "Showing all items".to_string()
        } else {
            capitalize_first(clauses.join("; "))
        }
    }

    pub(super) fn search(&self) -> HistorySearch {
        HistorySearch::new(self)
    }

    /// Whether the exchange passes every enabled display filter.
    pub(super) fn matches(
        &self,
        item: &HistoryItem,
        scope: &BTreeSet<String>,
        search: &HistorySearch,
    ) -> bool {
        if !self.enabled {
            return true;
        }
        if self.in_scope_only
            && !scope.is_empty()
            && !site_map::host_in_scope(&item.request.host, scope)
        {
            return false;
        }
        if self.hide_without_response && item.response.is_none() {
            return false;
        }
        if self.parameterized_only {
            let parameters = site_map::request_parameter_count(&item.request, &item.request_body);
            if parameters.count == 0 {
                return false;
            }
        }
        if !self.mime_visible(item) || !self.status_visible(item) {
            return false;
        }

        let extension = request_extension(item);
        if self.extension_show_only_enabled
            && has_extension_token(&self.extension_show_only)
            && !extension_list_contains(&self.extension_show_only, &extension)
        {
            return false;
        }
        if self.extension_hide_enabled && extension_list_contains(&self.extension_hide, &extension)
        {
            return false;
        }

        let status = item
            .status()
            .map(|status| status.to_string())
            .unwrap_or_default();
        let mime = site_map::mime_label(&mime_type(item));
        let tls = if request_uses_tls(item) { "tls" } else { "" };
        search.matches([
            item.request.method.as_str(),
            item.request.host.as_str(),
            item.request.path.as_str(),
            item.request.uri.as_str(),
            status.as_str(),
            mime.as_str(),
            extension.as_str(),
            tls,
        ])
    }

    fn mime_visible(&self, item: &HistoryItem) -> bool {
        if item.is_tunnel() || item.response.is_none() {
            return true;
        }
        match mime_class(&mime_type(item)) {
            MimeClass::Html => self.show_html,
            MimeClass::Script => self.show_script,
            MimeClass::Xml => self.show_xml,
            MimeClass::Css => self.show_css,
            MimeClass::OtherText => self.show_other_text,
            MimeClass::Images => self.show_images,
            MimeClass::Flash => self.show_flash,
            MimeClass::OtherBinary => self.show_other_binary,
        }
    }

    fn status_visible(&self, item: &HistoryItem) -> bool {
        let Some(status) = item.status() else {
            return true;
        };
        match status / 100 {
            2 => self.show_2xx,
            3 => self.show_3xx,
            4 => self.show_4xx,
            5 => self.show_5xx,
            // Burp's dialog only has boxes for 2xx-5xx. Informational codes
            // are uncommon in captured application traffic; do not hide them.
            _ => true,
        }
    }

    pub(super) fn to_workspace(&self) -> HistoryFilterWorkspace {
        HistoryFilterWorkspace {
            enabled: self.enabled,
            in_scope_only: self.in_scope_only,
            hide_without_response: self.hide_without_response,
            parameterized_only: self.parameterized_only,
            show_html: self.show_html,
            show_script: self.show_script,
            show_xml: self.show_xml,
            show_css: self.show_css,
            show_other_text: self.show_other_text,
            show_images: self.show_images,
            show_flash: self.show_flash,
            show_other_binary: self.show_other_binary,
            show_2xx: self.show_2xx,
            show_3xx: self.show_3xx,
            show_4xx: self.show_4xx,
            show_5xx: self.show_5xx,
            search: self.search.clone(),
            search_regex: self.search_regex,
            search_case_sensitive: self.search_case_sensitive,
            search_negative: self.search_negative,
            extension_show_only_enabled: self.extension_show_only_enabled,
            extension_show_only: self.extension_show_only.clone(),
            extension_hide_enabled: self.extension_hide_enabled,
            extension_hide: self.extension_hide.clone(),
        }
    }

    pub(super) fn from_workspace(workspace: &HistoryFilterWorkspace) -> Self {
        Self {
            enabled: workspace.enabled,
            in_scope_only: workspace.in_scope_only,
            hide_without_response: workspace.hide_without_response,
            parameterized_only: workspace.parameterized_only,
            show_html: workspace.show_html,
            show_script: workspace.show_script,
            show_xml: workspace.show_xml,
            show_css: workspace.show_css,
            show_other_text: workspace.show_other_text,
            show_images: workspace.show_images,
            show_flash: workspace.show_flash,
            show_other_binary: workspace.show_other_binary,
            show_2xx: workspace.show_2xx,
            show_3xx: workspace.show_3xx,
            show_4xx: workspace.show_4xx,
            show_5xx: workspace.show_5xx,
            search: workspace.search.clone(),
            search_regex: workspace.search_regex,
            search_case_sensitive: workspace.search_case_sensitive,
            search_negative: workspace.search_negative,
            extension_show_only_enabled: workspace.extension_show_only_enabled,
            extension_show_only: workspace.extension_show_only.clone(),
            extension_hide_enabled: workspace.extension_hide_enabled,
            extension_hide: workspace.extension_hide.clone(),
        }
    }
}

/// Compiled search term for one history rebuild.
pub(super) struct HistorySearch {
    literal: String,
    lower: String,
    regex: Option<Regex>,
    case_sensitive: bool,
    negative: bool,
    active: bool,
    invalid: bool,
}

impl HistorySearch {
    fn new(filter: &HistoryFilter) -> Self {
        let pattern = filter.search.trim();
        if pattern.is_empty() {
            return Self {
                literal: String::new(),
                lower: String::new(),
                regex: None,
                case_sensitive: filter.search_case_sensitive,
                negative: filter.search_negative,
                active: false,
                invalid: false,
            };
        }
        let mut invalid = false;
        let regex = if filter.search_regex {
            match RegexBuilder::new(pattern)
                .case_insensitive(!filter.search_case_sensitive)
                .build()
            {
                Ok(regex) => Some(regex),
                Err(_) => {
                    invalid = true;
                    None
                }
            }
        } else {
            None
        };
        // An invalid regex must not silently hide every row; ignore the
        // search term until the user fixes it.
        let active = !invalid;
        Self {
            literal: pattern.to_string(),
            lower: pattern.to_lowercase(),
            regex,
            case_sensitive: filter.search_case_sensitive,
            negative: filter.search_negative,
            active,
            invalid,
        }
    }

    fn matches<'a>(&self, values: impl IntoIterator<Item = &'a str>) -> bool {
        if !self.active {
            return true;
        }
        let hit = values.into_iter().any(|value| {
            if let Some(regex) = &self.regex {
                regex.is_match(value)
            } else if self.case_sensitive {
                value.contains(&self.literal)
            } else {
                value.to_lowercase().contains(&self.lower)
            }
        });
        if self.negative { !hit } else { hit }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MimeClass {
    Html,
    Script,
    Xml,
    Css,
    OtherText,
    Images,
    Flash,
    OtherBinary,
}

fn mime_class(mime: &str) -> MimeClass {
    let mime = mime.trim().to_ascii_lowercase();
    if mime.is_empty() || mime == "-" {
        return MimeClass::OtherBinary;
    }
    if mime == "text/html" || mime == "application/xhtml+xml" {
        return MimeClass::Html;
    }
    if mime == "text/css" {
        return MimeClass::Css;
    }
    if mime == "application/javascript"
        || mime == "text/javascript"
        || mime == "application/x-javascript"
        || mime.contains("ecmascript")
    {
        return MimeClass::Script;
    }
    if mime.starts_with("image/") {
        return MimeClass::Images;
    }
    if mime == "application/xml" || mime == "text/xml" || mime.ends_with("+xml") {
        return MimeClass::Xml;
    }
    if mime == "application/x-shockwave-flash" || mime == "application/vnd.adobe.flash.movie" {
        return MimeClass::Flash;
    }
    if mime.starts_with("text/")
        || mime == "application/json"
        || mime.ends_with("+json")
        || mime == "application/x-www-form-urlencoded"
        || mime == "application/graphql"
    {
        return MimeClass::OtherText;
    }
    MimeClass::OtherBinary
}

fn extension_list_contains(list: &str, extension: &str) -> bool {
    list.split(',').any(|token| {
        token
            .trim()
            .trim_start_matches('.')
            .eq_ignore_ascii_case(extension)
    })
}

fn has_extension_token(list: &str) -> bool {
    list.split(',')
        .any(|token| !token.trim().trim_start_matches('.').is_empty())
}

fn extension_tokens(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|token| token.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|token| !token.is_empty())
        .collect()
}

fn extension_words(extensions: &[String]) -> String {
    let words: Vec<String> = extensions
        .iter()
        .map(|extension| format!(".{extension}"))
        .collect();
    join_words_owned(&words)
}

fn join_words(words: &[&str]) -> String {
    match words {
        [] => String::new(),
        [one] => (*one).to_string(),
        [one, two] => format!("{one} and {two}"),
        _ => {
            let (last, rest) = words.split_last().expect("non-empty word list");
            format!("{}, and {last}", rest.join(", "))
        }
    }
}

fn join_words_owned(words: &[String]) -> String {
    let borrowed: Vec<&str> = words.iter().map(String::as_str).collect();
    join_words(&borrowed)
}

fn capitalize_first(text: String) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => text,
    }
}

fn truncate_summary(text: &str) -> String {
    const LIMIT: usize = 60;
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

impl PerseidApp {
    /// Toolbar above the HTTP history table.
    pub(super) fn history_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        let mut open_filter = false;
        ui.horizontal(|ui| {
            let available = ui.available_width();
            let summary_width = (available - 620.0).clamp(200.0, 900.0);
            let summary = self.history_filter_summary();
            if history_filter_bar(ui, summary_width, &summary).clicked() {
                open_filter = true;
            }
            ui.add_space(4.0);

            if history_search_field(ui, &mut self.filter, egui::Id::unique(FILTER_ID)) {
                self.visible_key = None;
            }

            let switch = filter_switch(ui, &mut self.history_filter.enabled);
            if switch.changed() {
                self.visible_key = None;
            }
            filter_help_badge(ui);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Clear history")
                    .on_hover_text("Discard all captured exchanges")
                    .clicked()
                {
                    clear = true;
                }
                ui.label(
                    RichText::new(format!(
                        "{} shown / {} captured",
                        self.visible.len(),
                        self.history.len()
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });

        if open_filter {
            self.history_filter_draft = Some(self.history_filter.clone());
        }
        if clear {
            self.clear_history();
        }
    }

    fn history_filter_summary(&self) -> String {
        let mut summary = self.history_filter.summary();
        let scope_active = self.scope_enabled && !self.scope.is_empty();
        if scope_active {
            if !self.history_filter.enabled {
                summary = "Filter off; scope filter on".to_string();
            } else if self.history_filter.is_show_all() {
                summary = "Showing only in-scope items".to_string();
            } else if !self.history_filter.in_scope_only {
                summary.push_str("; scope filter on");
            }
        }
        summary
    }

    /// Draw the staged filter modal. Edits stay in a draft until Apply.
    pub(super) fn history_filter_window(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.history_filter_draft.clone() else {
            return;
        };
        let applied = self.history_filter.clone();
        let mut action = FilterAction::None;
        let modal = egui::Modal::new(egui::Id::unique("perseid.history.filter"))
            .backdrop_color(Color32::from_black_alpha(42))
            .frame(file_dialog::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(FILTER_MODAL_WIDTH);
                if file_dialog::title_bar(ui, "HTTP history filter", None) {
                    action = FilterAction::Cancel;
                }
                egui::Frame::NONE
                    .inner_margin(Margin {
                        left: 16,
                        right: 16,
                        top: 10,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        ui.set_width(FILTER_MODAL_WIDTH - 32.0);
                        ui.label(
                            RichText::new(
                                "Display filters hide rows without deleting captured exchanges.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(8.0);

                        ui.columns(3, |columns| {
                            fieldset(&mut columns[0], "Filter by request type", |ui| {
                                ui.set_min_height(94.0);
                                ui.checkbox(
                                    &mut draft.in_scope_only,
                                    "Show only in-scope items",
                                );
                                ui.checkbox(
                                    &mut draft.hide_without_response,
                                    "Hide items without responses",
                                );
                                ui.checkbox(
                                    &mut draft.parameterized_only,
                                    "Show only parameterized requests",
                                );
                            });
                            fieldset(&mut columns[1], "Filter by MIME type", |ui| {
                                ui.set_min_height(94.0);
                                ui.columns(2, |mime| {
                                    mime[0].checkbox(&mut draft.show_html, "HTML");
                                    mime[0].checkbox(&mut draft.show_script, "Script");
                                    mime[0].checkbox(&mut draft.show_xml, "XML");
                                    mime[0].checkbox(&mut draft.show_css, "CSS");
                                    mime[1].checkbox(&mut draft.show_other_text, "Other text");
                                    mime[1].checkbox(&mut draft.show_images, "Images");
                                    mime[1].checkbox(&mut draft.show_flash, "Flash");
                                    mime[1].checkbox(
                                        &mut draft.show_other_binary,
                                        "Other binary",
                                    );
                                });
                            });
                            fieldset(&mut columns[2], "Filter by status code", |ui| {
                                ui.set_min_height(94.0);
                                ui.checkbox(&mut draft.show_2xx, "2xx [success]");
                                ui.checkbox(&mut draft.show_3xx, "3xx [redirection]");
                                ui.checkbox(&mut draft.show_4xx, "4xx [request error]");
                                ui.checkbox(&mut draft.show_5xx, "5xx [server error]");
                            });
                        });

                        ui.add_space(10.0);
                        ui.columns(2, |columns| {
                            fieldset(&mut columns[0], "Filter by search term", |ui| {
                                ui.set_min_height(104.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut draft.search)
                                        .hint_text("Search the request metadata")
                                        .desired_width(ui.available_width()),
                                );
                                ui.add_space(2.0);
                                ui.checkbox(&mut draft.search_regex, "Regex");
                                ui.checkbox(
                                    &mut draft.search_case_sensitive,
                                    "Case sensitive",
                                );
                                ui.checkbox(
                                    &mut draft.search_negative,
                                    "Negative search",
                                );
                                if HistorySearch::new(&draft).invalid {
                                    ui.label(
                                        RichText::new(
                                            "Invalid regular expression; the search is ignored until it is fixed.",
                                        )
                                        .small()
                                        .color(theme::RED),
                                    );
                                }
                            });
                            fieldset(&mut columns[1], "Filter by file extension", |ui| {
                                ui.set_min_height(104.0);
                                ui.horizontal(|ui| {
                                    ui.checkbox(
                                        &mut draft.extension_show_only_enabled,
                                        "Show only:",
                                    );
                                    ui.add_enabled(
                                        draft.extension_show_only_enabled,
                                        egui::TextEdit::singleline(
                                            &mut draft.extension_show_only,
                                        )
                                        .hint_text("asp,aspx,jsp,php")
                                        .desired_width(ui.available_width()),
                                    );
                                });
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    ui.checkbox(
                                        &mut draft.extension_hide_enabled,
                                        "Hide:",
                                    );
                                    ui.add_enabled(
                                        draft.extension_hide_enabled,
                                        egui::TextEdit::singleline(
                                            &mut draft.extension_hide,
                                        )
                                        .hint_text("js,gif,jpg,png,ico,css,woff,woff2")
                                        .desired_width(ui.available_width()),
                                    );
                                });
                            });
                        });

                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(
                                "Filters only change what is displayed. Hidden exchanges remain in the history and the project.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        egui::Sides::new().height(28.0).show(
                            ui,
                            |ui| {
                                if ui.button("Show all").clicked() {
                                    draft = HistoryFilter::SHOW_ALL;
                                }
                                if ui.button("Hide all").clicked() {
                                    draft = draft.hide_all();
                                }
                                if ui
                                    .add_enabled(
                                        draft != applied,
                                        egui::Button::new("Revert changes"),
                                    )
                                    .clicked()
                                {
                                    draft = applied.clone();
                                }
                            },
                            |ui| {
                                if ui
                                    .add_sized(
                                        [118.0, 28.0],
                                        egui::Button::new(
                                            RichText::new("Apply & close")
                                                .color(Color32::WHITE),
                                        )
                                        .fill(theme::ACCENT),
                                    )
                                    .clicked()
                                {
                                    action = FilterAction::ApplyClose;
                                }
                                if ui
                                    .add_sized([82.0, 28.0], egui::Button::new("Apply"))
                                    .clicked()
                                {
                                    action = FilterAction::Apply;
                                }
                                if ui
                                    .add_sized([82.0, 28.0], egui::Button::new("Cancel"))
                                    .clicked()
                                {
                                    action = FilterAction::Cancel;
                                }
                            },
                        );
                    });
            });

        if modal.should_close() && action == FilterAction::None {
            action = FilterAction::Cancel;
        }
        match action {
            FilterAction::Apply | FilterAction::ApplyClose => {
                draft.enabled = true;
                self.history_filter = draft.clone();
                self.visible_key = None;
                if action == FilterAction::ApplyClose {
                    self.history_filter_draft = None;
                } else {
                    self.history_filter_draft = Some(draft);
                }
            }
            FilterAction::Cancel => self.history_filter_draft = None,
            FilterAction::None => self.history_filter_draft = Some(draft),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterAction {
    None,
    Apply,
    ApplyClose,
    Cancel,
}

fn history_filter_bar(ui: &mut egui::Ui, width: f32, summary: &str) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, FILTER_BAR_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() {
            theme::ACCENT_WASH
        } else {
            theme::SURFACE_TINT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(3), fill);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );
        let icon = Rect::from_center_size(
            egui::pos2(rect.left() + 15.0, rect.center().y),
            Vec2::splat(13.0),
        );
        paint_funnel(ui.painter(), icon, theme::TEXT_MUTED);

        let text_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 28.0, rect.top() + 2.0),
            egui::pos2(rect.right() - 22.0, rect.bottom() - 2.0),
        );
        let painter = ui.painter().with_clip_rect(text_rect);
        let galley = painter.layout_no_wrap(
            summary.to_owned(),
            FontId::proportional(11.5),
            theme::TEXT_MUTED,
        );
        painter.galley(
            egui::pos2(text_rect.left(), rect.center().y - galley.size().y * 0.5),
            galley,
            theme::TEXT_MUTED,
        );

        let caret = egui::pos2(rect.right() - 12.0, rect.center().y);
        ui.painter().line_segment(
            [caret + Vec2::new(-3.5, -1.5), caret + Vec2::new(0.0, 2.0)],
            Stroke::new(1.2, theme::TEXT_MUTED),
        );
        ui.painter().line_segment(
            [caret + Vec2::new(0.0, 2.0), caret + Vec2::new(3.5, -1.5)],
            Stroke::new(1.2, theme::TEXT_MUTED),
        );
    }
    response
        .on_hover_text("Edit the HTTP history display filter")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_funnel(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.3, color);
    let left = rect.left();
    let right = rect.right();
    let top = rect.top();
    let middle = rect.center().y + 0.5;
    let bottom = rect.bottom();
    let center = rect.center().x;
    painter.line_segment([egui::pos2(left, top), egui::pos2(right, top)], stroke);
    painter.line_segment(
        [egui::pos2(left, top), egui::pos2(center - 1.2, middle)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(right, top), egui::pos2(center + 1.2, middle)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(center, middle), egui::pos2(center, bottom)],
        stroke,
    );
}

fn history_search_field(ui: &mut egui::Ui, text: &mut String, id: egui::Id) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(242.0, FILTER_BAR_HEIGHT), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(3), Color32::WHITE);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    let action_rect = Rect::from_center_size(
        egui::pos2(rect.right() - 14.0, rect.center().y),
        Vec2::splat(15.0),
    );
    let edit_rect = Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.top() + 3.0),
        egui::pos2(rect.right() - 27.0, rect.bottom() - 3.0),
    );
    let response = ui.put(
        edit_rect,
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(Margin::ZERO)
            .hint_text("Search")
            .desired_width(edit_rect.width()),
    );

    let action = ui
        .interact(action_rect.expand(3.0), id.with("action"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut changed = response.changed();
    if action.clicked() && !text.is_empty() {
        text.clear();
        ui.ctx().memory_mut(|memory| memory.request_focus(id));
        changed = true;
    }

    if text.is_empty() {
        paint_magnifier(ui.painter(), action_rect, theme::TEXT_MUTED);
        action.on_hover_text("Focus the history filter (Ctrl+F)");
    } else {
        let center = action_rect.center();
        let arm = 4.0;
        let stroke = Stroke::new(1.3, theme::TEXT_MUTED);
        ui.painter().line_segment(
            [center + Vec2::new(-arm, -arm), center + Vec2::new(arm, arm)],
            stroke,
        );
        ui.painter().line_segment(
            [center + Vec2::new(arm, -arm), center + Vec2::new(-arm, arm)],
            stroke,
        );
        action.on_hover_text("Clear the filter (Ctrl+F to focus)");
    }
    changed
}

fn paint_magnifier(painter: &egui::Painter, rect: Rect, color: Color32) {
    let center = rect.center() + Vec2::new(-1.0, -1.0);
    painter.circle_stroke(center, 4.0, Stroke::new(1.3, color));
    painter.line_segment(
        [center + Vec2::new(2.8, 2.8), center + Vec2::new(5.6, 5.6)],
        Stroke::new(1.4, color),
    );
}

fn filter_switch(ui: &mut egui::Ui, enabled: &mut bool) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap("Filter on".to_owned(), font.clone(), theme::TEXT);
    let width = FILTER_SWITCH_WIDTH + 7.0 + galley.size().x + 2.0;
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(width, FILTER_BAR_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        let track = Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - FILTER_SWITCH_HEIGHT * 0.5),
            Vec2::new(FILTER_SWITCH_WIDTH, FILTER_SWITCH_HEIGHT),
        );
        let fill = if *enabled {
            theme::ACCENT
        } else {
            theme::BORDER
        };
        ui.painter().rect_filled(track, CornerRadius::same(8), fill);
        let knob_x = if *enabled {
            track.right() - 8.0
        } else {
            track.left() + 8.0
        };
        ui.painter()
            .circle_filled(egui::pos2(knob_x, track.center().y), 6.0, Color32::WHITE);
        ui.painter().text(
            egui::pos2(track.right() + 7.0, rect.center().y),
            Align2::LEFT_CENTER,
            "Filter on",
            font,
            theme::TEXT,
        );
    }
    if response.clicked() {
        *enabled = !*enabled;
        response.mark_changed();
    }
    response
        .on_hover_text("Apply the HTTP history display filter")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn filter_help_badge(ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(FILTER_BAR_HEIGHT), Sense::hover());
    if ui.is_rect_visible(rect) {
        let center = rect.center();
        ui.painter()
            .circle_stroke(center, 8.0, Stroke::new(1.2, theme::TEXT_MUTED));
        ui.painter().text(
            center,
            Align2::CENTER_CENTER,
            "?",
            FontId::proportional(11.0),
            theme::TEXT_MUTED,
        );
    }
    response.on_hover_text(
        "Filter settings hide table rows. The search field is a quick text filter; the filter dialog also supports regular expressions, negative searches, MIME classes, status classes, and file extensions.",
    );
}

fn fieldset(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let response = egui::Frame::NONE
        .inner_margin(Margin {
            left: 12,
            right: 12,
            top: 18,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui);
        });
    let rect = response.response.rect;
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        FontId::proportional(12.0),
        theme::TEXT_MUTED,
    );
    let title_pos = egui::pos2(rect.left() + 10.0, rect.top() - galley.size().y * 0.5);
    ui.painter().rect_filled(
        Rect::from_min_size(
            title_pos - Vec2::new(4.0, 0.0),
            galley.size() + Vec2::new(8.0, 0.0),
        ),
        0.0,
        theme::SURFACE,
    );
    ui.painter().galley(title_pos, galley, theme::TEXT_MUTED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{History, Phase};
    use crate::proxy::{CapturedBody, ExchangeResult, RequestInfo, ResponseHead};
    use std::time::{Duration, SystemTime};

    fn request(method: &str, host: &str, path: &str) -> RequestInfo {
        RequestInfo {
            method: method.to_string(),
            uri: path.to_string(),
            scheme: "https".to_string(),
            host: host.to_string(),
            path: path.to_string(),
            version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
        }
    }

    fn item(method: &str, host: &str, path: &str) -> HistoryItem {
        HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::now(),
            request: request(method, host, path),
            request_body: CapturedBody::default(),
            response: None,
            phase: Phase::InFlight,
        }
    }

    fn numbered_item(id: u64, method: &str, host: &str, path: &str) -> HistoryItem {
        let mut item = item(method, host, path);
        item.id = id;
        item.number = id;
        item
    }

    fn with_response(mut item: HistoryItem, status: u16, mime: &str, body: &[u8]) -> HistoryItem {
        item.response = Some(ResponseHead {
            version: "HTTP/1.1".to_string(),
            status,
            reason: String::new(),
            headers: vec![("content-type".to_string(), mime.to_string())],
        });
        item.phase = Phase::Finished {
            duration: Duration::from_millis(12),
            result: ExchangeResult::Response {
                body: CapturedBody::from_bytes(body.to_vec()),
            },
        };
        item
    }

    fn matches(filter: &HistoryFilter, item: &HistoryItem) -> bool {
        let search = filter.search();
        filter.matches(item, &BTreeSet::new(), &search)
    }

    #[test]
    fn show_all_matches_everything_by_default() {
        let filter = HistoryFilter::default();
        assert!(filter.is_show_all());
        assert_eq!(filter.summary(), "Showing all items");
        assert!(matches(&filter, &item("GET", "example.com", "/")));
        assert!(matches(
            &filter,
            &with_response(
                item("GET", "example.com", "/"),
                404,
                "text/html",
                b"missing"
            )
        ));
    }

    #[test]
    fn disabling_the_filter_ignores_every_criterion() {
        let filter = HistoryFilter {
            enabled: false,
            hide_without_response: true,
            show_2xx: false,
            search: "needle".to_string(),
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(&filter, &item("GET", "example.com", "/")));
        assert_eq!(filter.summary(), "Filter off — showing all items");
    }

    #[test]
    fn hide_without_response_keeps_only_completed_exchanges() {
        let filter = HistoryFilter {
            hide_without_response: true,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(!matches(&filter, &item("GET", "example.com", "/")));
        assert!(matches(
            &filter,
            &with_response(item("GET", "example.com", "/"), 200, "text/plain", b"ok")
        ));
        assert!(filter.summary().contains("Hiding items without responses"));
    }

    #[test]
    fn parameterized_only_accepts_query_and_form_parameters() {
        let filter = HistoryFilter {
            parameterized_only: true,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(!matches(&filter, &item("GET", "example.com", "/")));
        assert!(matches(
            &filter,
            &item("GET", "example.com", "/search?q=one&page=2")
        ));

        let mut form = item("POST", "example.com", "/login");
        form.request.headers = vec![(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        form.request_body = CapturedBody::from_bytes(b"user=admin&password=secret".to_vec());
        assert!(matches(&filter, &form));
    }

    #[test]
    fn mime_classes_can_be_hidden_individually() {
        let filter = HistoryFilter {
            show_css: false,
            show_images: false,
            ..HistoryFilter::SHOW_ALL
        };
        let html = with_response(item("GET", "example.com", "/"), 200, "text/html", b"");
        let css = with_response(
            item("GET", "example.com", "/site.css"),
            200,
            "text/css",
            b"",
        );
        let image = with_response(
            item("GET", "example.com", "/logo.png"),
            200,
            "image/png",
            b"",
        );
        let json = with_response(
            item("GET", "example.com", "/api"),
            200,
            "application/json",
            b"",
        );
        assert!(matches(&filter, &html));
        assert!(!matches(&filter, &css));
        assert!(!matches(&filter, &image));
        assert!(matches(&filter, &json));
        assert!(filter.summary().contains("Hiding CSS and image content"));
    }

    #[test]
    fn unknown_mime_types_are_other_binary() {
        assert_eq!(mime_class(""), MimeClass::OtherBinary);
        assert_eq!(mime_class("-"), MimeClass::OtherBinary);
        assert_eq!(
            mime_class("application/octet-stream"),
            MimeClass::OtherBinary
        );
        assert_eq!(
            mime_class("application/x-shockwave-flash"),
            MimeClass::Flash
        );
        assert_eq!(mime_class("application/problem+json"), MimeClass::OtherText);
        assert_eq!(mime_class("image/svg+xml"), MimeClass::Images);
    }

    #[test]
    fn status_classes_can_be_hidden_individually() {
        let filter = HistoryFilter {
            show_4xx: false,
            show_5xx: false,
            ..HistoryFilter::SHOW_ALL
        };
        let ok = with_response(item("GET", "example.com", "/"), 200, "text/plain", b"");
        let redirect = with_response(item("GET", "example.com", "/"), 302, "text/plain", b"");
        let missing = with_response(item("GET", "example.com", "/"), 404, "text/plain", b"");
        let failed = with_response(item("GET", "example.com", "/"), 503, "text/plain", b"");
        assert!(matches(&filter, &ok));
        assert!(matches(&filter, &redirect));
        assert!(!matches(&filter, &missing));
        assert!(!matches(&filter, &failed));
        assert!(filter.summary().contains("Hiding 4xx and 5xx responses"));
    }

    #[test]
    fn status_filter_does_not_hide_exchanges_without_a_response() {
        let filter = HistoryFilter {
            show_4xx: false,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(&filter, &item("GET", "example.com", "/")));
    }

    #[test]
    fn in_scope_only_uses_the_scope_rules() {
        let filter = HistoryFilter {
            in_scope_only: true,
            ..HistoryFilter::SHOW_ALL
        };
        let scope = BTreeSet::from(["example.com".to_string()]);
        let inside = with_response(item("GET", "example.com", "/"), 200, "text/plain", b"");
        let outside = with_response(item("GET", "other.test", "/"), 200, "text/plain", b"");
        let search = filter.search();
        assert!(filter.matches(&inside, &scope, &search));
        assert!(!filter.matches(&outside, &scope, &search));
        assert!(filter.summary().contains("Showing only in-scope items"));
    }

    #[test]
    fn empty_scope_allows_every_host() {
        let filter = HistoryFilter {
            in_scope_only: true,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(&filter, &item("GET", "example.com", "/")));
    }

    #[test]
    fn plain_search_is_case_insensitive_by_default() {
        let filter = HistoryFilter {
            search: "ADMIN".to_string(),
            ..HistoryFilter::SHOW_ALL
        };
        let admin = item("GET", "example.com", "/admin");
        let home = item("GET", "example.com", "/");
        assert!(matches(&filter, &admin));
        assert!(!matches(&filter, &home));
        assert!(filter.summary().contains("Searching for \"ADMIN\""));
    }

    #[test]
    fn case_sensitive_search_respects_case() {
        let filter = HistoryFilter {
            search: "ADMIN".to_string(),
            search_case_sensitive: true,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(!matches(&filter, &item("GET", "example.com", "/admin")));
        assert!(matches(&filter, &item("GET", "example.com", "/ADMIN")));
    }

    #[test]
    fn regex_search_and_negative_search_work() {
        let regex = HistoryFilter {
            search: r"^/api/v\d+".to_string(),
            search_regex: true,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(
            &regex,
            &item("GET", "example.com", "/api/v2/users")
        ));
        assert!(!matches(&regex, &item("GET", "example.com", "/api/users")));

        let negative = HistoryFilter {
            search: "logout".to_string(),
            search_negative: true,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(
            &negative,
            &item("GET", "example.com", "/dashboard")
        ));
        assert!(!matches(&negative, &item("GET", "example.com", "/logout")));
    }

    #[test]
    fn invalid_regex_is_ignored_and_reported_in_the_summary() {
        let filter = HistoryFilter {
            search: "[".to_string(),
            search_regex: true,
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(&filter, &item("GET", "example.com", "/")));
        assert!(HistorySearch::new(&filter).invalid);
        assert!(filter.summary().contains("Invalid regular expression"));
    }

    #[test]
    fn extension_show_only_and_hide_lists_filter_the_url_path() {
        let show = HistoryFilter {
            extension_show_only_enabled: true,
            extension_show_only: " .PHP, js ".to_string(),
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(
            &show,
            &item("GET", "example.com", "/index.php?page=1")
        ));
        assert!(!matches(&show, &item("GET", "example.com", "/index.css")));

        let hide = HistoryFilter {
            extension_hide_enabled: true,
            extension_hide: ".css,js".to_string(),
            ..HistoryFilter::SHOW_ALL
        };
        assert!(!matches(&hide, &item("GET", "example.com", "/site.css")));
        assert!(matches(&hide, &item("GET", "example.com", "/index.php")));
        assert!(hide.summary().contains("Hiding .css and .js"));
    }

    #[test]
    fn empty_extension_lists_do_not_hide_anything() {
        let filter = HistoryFilter {
            extension_show_only_enabled: true,
            extension_show_only: " , ".to_string(),
            extension_hide_enabled: true,
            extension_hide: " ".to_string(),
            ..HistoryFilter::SHOW_ALL
        };
        assert!(matches(&filter, &item("GET", "example.com", "/index.php")));
    }

    #[test]
    fn search_covers_status_mime_extension_and_tls() {
        let response = with_response(
            item("GET", "example.com", "/assets/app.css"),
            404,
            "text/css",
            b"",
        );
        for term in ["GET", "example.com", "/assets", "404", "CSS", "css", "tls"] {
            let filter = HistoryFilter {
                search: term.to_string(),
                ..HistoryFilter::SHOW_ALL
            };
            assert!(matches(&filter, &response), "expected {term} to match");
        }
    }

    #[test]
    fn show_all_and_hide_all_update_the_summary() {
        let filter = HistoryFilter::SHOW_ALL;
        let hidden = filter.hide_all();
        assert!(!hidden.show_html);
        assert!(!hidden.show_2xx);
        assert!(hidden.hide_without_response);
        let summary = hidden.summary();
        assert!(summary.contains(
            "HTML, script, XML, CSS, other text, image, Flash, and other binary content"
        ));
        assert!(summary.contains("2xx, 3xx, 4xx, and 5xx"));

        assert_eq!(HistoryFilter::SHOW_ALL.summary(), "Showing all items");
    }

    #[test]
    fn visible_rows_apply_quick_search_scope_and_advanced_filters() {
        let mut app = PerseidApp::test_app();
        let completed = with_response(
            numbered_item(1, "GET", "example.com", "/admin"),
            200,
            "text/html",
            b"<title>Admin</title>",
        );
        let in_flight = numbered_item(2, "GET", "example.com", "/admin/pending");
        let outside = with_response(
            numbered_item(3, "GET", "other.test", "/admin"),
            200,
            "text/plain",
            b"",
        );
        app.history.load(vec![completed, in_flight, outside]);
        app.scope = BTreeSet::from(["example.com".to_string()]);
        app.scope_enabled = true;
        app.filter = "admin".to_string();
        app.history_filter = HistoryFilter {
            hide_without_response: true,
            ..HistoryFilter::SHOW_ALL
        };

        app.rebuild_http_visible();
        assert_eq!(app.visible, vec![1]);

        app.filter.clear();
        app.rebuild_http_visible();
        assert_eq!(app.visible, vec![1]);

        app.history_filter.enabled = false;
        app.rebuild_http_visible();
        assert_eq!(app.visible, vec![1, 2]);

        app.scope_enabled = false;
        app.rebuild_http_visible();
        assert_eq!(app.visible, vec![1, 2, 3]);
    }

    #[test]
    fn workspace_record_round_trips_the_filter() {
        let filter = HistoryFilter {
            in_scope_only: true,
            hide_without_response: true,
            parameterized_only: true,
            show_css: false,
            show_4xx: false,
            search: "admin".to_string(),
            search_regex: true,
            search_case_sensitive: true,
            search_negative: true,
            extension_show_only_enabled: true,
            extension_show_only: "php,aspx".to_string(),
            extension_hide_enabled: true,
            extension_hide: "js,css".to_string(),
            ..HistoryFilter::SHOW_ALL
        };
        let workspace = filter.to_workspace();
        assert_eq!(HistoryFilter::from_workspace(&workspace), filter);

        let default = HistoryFilterWorkspace::default();
        assert_eq!(
            HistoryFilter::from_workspace(&default),
            HistoryFilter::SHOW_ALL
        );
    }

    #[test]
    fn summary_truncates_long_search_terms() {
        let filter = HistoryFilter {
            search: "x".repeat(80),
            ..HistoryFilter::SHOW_ALL
        };
        let summary = filter.summary();
        assert!(summary.contains('…'));
        assert!(summary.len() < 120);
    }

    #[test]
    fn matching_history_respects_disabled_filters() {
        let filter = HistoryFilter {
            enabled: false,
            hide_without_response: true,
            ..HistoryFilter::SHOW_ALL
        };
        let mut history = History::new(10, 1024);
        history.load(vec![item("GET", "example.com", "/")]);
        let search = filter.search();
        assert!(
            history
                .iter()
                .all(|item| filter.matches(item, &BTreeSet::new(), &search))
        );
    }
}
