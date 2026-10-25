//! Target tab: the captured-site map and the scope editor.
//!
//! The site map follows Burp's URL view: a host/folder/resource tree on the
//! left, a filterable Contents table for the selected node, and the shared
//! request/response and Inspector panes underneath. Everything is derived from
//! captured HTTP history; there is no crawler and no inferred content.

use std::collections::BTreeMap;
use std::time::SystemTime;

use egui::{
    Align, Align2, Color32, FontId, Label, Layout, Rect, RichText, Sense, Shape, Stroke, TextEdit,
};
use egui_extras::{Column, TableBuilder};

use crate::history::HistoryItem;
use crate::site_map::{self, SiteEntry, SiteMap, SiteMapFilter, SiteNodeId, SiteTreeNode};
use crate::theme;

use super::util::captured_hosts;
use super::widgets::{card, empty_state, page_pad, section_bar, section_tab, table_frame};
use super::*;

const SITE_MAP_DETAIL_ID: &str = "perseid.site-map.detail";
pub(super) const SITE_MAP_SEARCH_ID: &str = "perseid.site-map.search";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SiteMapTab {
    #[default]
    Contents,
    Issues,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SiteMapSort {
    Host,
    Method,
    Url,
    Params,
    Status,
    Length,
    Mime,
    Title,
    Notes,
    Time,
}

impl SiteMapSort {
    fn label(self) -> &'static str {
        match self {
            Self::Host => "Host",
            Self::Method => "Method",
            Self::Url => "URL",
            Self::Params => "Params",
            Self::Status => "Status code",
            Self::Length => "Length",
            Self::Mime => "MIME type",
            Self::Title => "Title",
            Self::Notes => "Notes",
            Self::Time => "Time requested",
        }
    }
}

/// View-local state for the Target > Site map tab.
pub(super) struct SiteMapUi {
    selected_node: Option<SiteNodeId>,
    /// Expansion state keyed by [`SiteNodeId::key`]. Hosts and the root folder
    /// default to open, matching Burp's initial tree.
    open: BTreeMap<String, bool>,
    filter: SiteMapFilter,
    filter_open: bool,
    help_open: bool,
    tab: SiteMapTab,
    search: String,
    sort: SiteMapSort,
    sort_ascending: bool,
    paused: bool,
    scroll_to_selected: bool,
    /// IDs in the Contents table for the current node, filter, and search.
    /// The keyboard handler uses this snapshot from the previous frame.
    visible_ids: Vec<u64>,
}

impl Default for SiteMapUi {
    fn default() -> Self {
        Self::new()
    }
}

impl SiteMapUi {
    pub(super) fn new() -> Self {
        Self {
            selected_node: None,
            open: BTreeMap::new(),
            filter: SiteMapFilter::default(),
            filter_open: false,
            help_open: false,
            tab: SiteMapTab::Contents,
            search: String::new(),
            sort: SiteMapSort::Url,
            sort_ascending: true,
            paused: false,
            scroll_to_selected: false,
            visible_ids: Vec::new(),
        }
    }

    pub(super) fn paused(&self) -> bool {
        self.paused
    }

    /// Whether the Contents table is the active right-hand tab.
    pub(super) fn on_contents(&self) -> bool {
        self.tab == SiteMapTab::Contents
    }

    /// Whether a site-map window is open over the workspace.
    pub(super) fn windows_open(&self) -> bool {
        self.filter_open || self.help_open
    }

    #[cfg(test)]
    pub(super) fn set_issues_tab(&mut self) {
        self.tab = SiteMapTab::Issues;
    }

    #[cfg(test)]
    pub(super) fn set_contents_tab(&mut self) {
        self.tab = SiteMapTab::Contents;
    }

    #[cfg(test)]
    pub(super) fn set_visible_ids(&mut self, ids: Vec<u64>) {
        self.visible_ids = ids;
    }

    #[cfg(test)]
    pub(super) fn set_windows_open(&mut self, open: bool) {
        self.filter_open = open;
        self.help_open = open;
    }

    #[cfg(feature = "screenshots")]
    pub(super) fn reset(&mut self) {
        *self = Self::new();
    }
}

#[derive(Clone, Copy, Debug)]
enum SiteMapRowAction {
    Repeater,
    Intruder,
    Sequencer,
    ComparerRequest,
    ComparerResponse,
}

#[derive(Clone, Copy, Debug)]
enum SiteMapMenuAction {
    CopyUrls,
    ExpandAll,
    CollapseAll,
    ResetFilter,
    ClearSearch,
}

#[derive(Clone, Copy, Debug)]
enum SiteMapIcon {
    Help,
    Pause,
    Play,
    Settings,
}

struct SiteRowRef<'a> {
    entry: &'a SiteEntry,
    item: Option<&'a HistoryItem>,
    notes: &'a str,
}

impl PerseidApp {
    pub(super) fn target_tab(&mut self, ui: &mut egui::Ui) {
        section_bar(ui, "perseid.target.sections", |ui| {
            if section_tab(
                ui,
                self.target_section == TargetSection::SiteMap,
                "Site map",
            )
            .clicked()
            {
                self.target_section = TargetSection::SiteMap;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
            if section_tab(ui, self.target_section == TargetSection::Scope, "Scope").clicked() {
                self.target_section = TargetSection::Scope;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
            if section_tab(ui, self.target_section == TargetSection::Issues, "Issues").clicked() {
                self.target_section = TargetSection::Issues;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
        });

        match self.target_section {
            TargetSection::SiteMap => self.site_map_view(ui),
            TargetSection::Scope => self.scope_view(ui),
            TargetSection::Issues => target_issues_view(ui),
        }
    }

    fn site_map_view(&mut self, ui: &mut egui::Ui) {
        self.ensure_site_map();
        let ctx = ui.ctx().clone();
        self.site_map_toolbar(ui);

        // Take the cached map out while this frame draws so the mutable app
        // state (selection, filters, and the shared detail panes) can be used
        // without holding a borrow of `self.site_map`.
        let Some((revision, map)) = self.site_map.take() else {
            empty_state(
                ui,
                "No HTTP endpoints",
                "Browse an HTTP or HTTPS site through the proxy to populate the site map.",
            );
            self.site_map_windows(&ctx);
            return;
        };
        if map.is_empty() {
            let body = if self.scope_enabled {
                "No in-scope HTTP endpoints. Adjust the scope in Target > Scope, or turn off scope filtering."
            } else {
                "Browse an HTTP or HTTPS site through the proxy to populate the site map. HTTPS requests appear after decryption."
            };
            empty_state(ui, "No HTTP endpoints", body);
            self.site_map = Some((revision, map));
            self.site_map_windows(&ctx);
            return;
        }

        let tree = map.tree(self.site_map_ui.filter);
        let mut node_changed = false;
        if self
            .site_map_ui
            .selected_node
            .as_ref()
            .is_none_or(|selected| !tree.iter().any(|node| node.find(selected).is_some()))
        {
            self.site_map_ui.selected_node = tree.first().map(|node| node.id.clone());
            self.site_map_ui.scroll_to_selected = true;
            node_changed = true;
        }

        let mut picked = None;
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
                picked = site_map_tree(ui, &tree, &mut self.site_map_ui);
            });
        if let Some(node) = picked {
            self.site_map_ui.selected_node = Some(node);
            self.site_map_ui.scroll_to_selected = true;
            node_changed = true;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ui, |ui| {
                if self.site_map_ui.tab == SiteMapTab::Contents {
                    egui::Panel::bottom(SITE_MAP_DETAIL_ID)
                        .resizable(true)
                        .default_size((ui.available_height() * 0.46).max(220.0))
                        .min_size(190.0)
                        .frame(
                            egui::Frame::NONE
                                .fill(theme::SURFACE)
                                .inner_margin(egui::Margin::same(5)),
                        )
                        .show(ui, |ui| self.detail_panel(ui));
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::SURFACE))
                    .show(ui, |ui| {
                        let action = self.site_map_contents(ui, &map, &tree, node_changed);
                        if let Some(action) = action {
                            self.apply_site_map_row_action(action);
                        }
                    });
            });

        self.site_map = Some((revision, map));
        self.site_map_windows(&ctx);
    }

    fn site_map_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.menu_button("URL view", |ui| {
                        let _ = ui.selectable_label(true, "URL view");
                        ui.add_enabled(false, egui::Button::new("Crawl paths view"))
                            .on_disabled_hover_text(
                                "Perseid does not crawl. The site map only shows requests captured by the proxy.",
                            );
                    })
                    .response
                    .on_hover_text("Organize captured requests by host, folder, and URL");
                    ui.add_enabled(false, egui::Button::new("Crawl paths view"))
                        .on_disabled_hover_text(
                            "Perseid does not crawl. Crawl paths require Burp Scanner or a crawler.",
                        );

                    let controls_width = 3.0 * 26.0 + 3.0 * ui.spacing().item_spacing.x;
                    let summary = self.site_map_ui.filter.summary();
                    let filter_width = (ui.available_width() - controls_width)
                        .clamp(120.0, 900.0);
                    let response = filter_bar(ui, filter_width, &summary);
                    if response.clicked() {
                        self.site_map_ui.filter_open = true;
                    }
                    response.on_hover_text("Configure the site map display filter");

                    if icon_button(ui, SiteMapIcon::Help, false)
                        .on_hover_text("About the site map")
                        .clicked()
                    {
                        self.site_map_ui.help_open = true;
                    }
                    let icon = if self.site_map_ui.paused {
                        SiteMapIcon::Play
                    } else {
                        SiteMapIcon::Pause
                    };
                    let pause_hint = if self.site_map_ui.paused {
                        "Resume live site map updates"
                    } else {
                        "Pause live site map updates; captured traffic is still recorded"
                    };
                    if icon_button(ui, icon, self.site_map_ui.paused)
                        .on_hover_text(pause_hint)
                        .clicked()
                    {
                        self.site_map_ui.paused = !self.site_map_ui.paused;
                        // Keep the current snapshot visible until the next
                        // frame's revision check rebuilds it after new traffic.
                    }
                    if icon_button(ui, SiteMapIcon::Settings, false)
                        .on_hover_text("Site map filter settings")
                        .clicked()
                    {
                        self.site_map_ui.filter_open = true;
                    }
                });
            });
    }

    fn site_map_contents(
        &mut self,
        ui: &mut egui::Ui,
        map: &SiteMap,
        tree: &[SiteTreeNode],
        node_changed: bool,
    ) -> Option<SiteMapRowAction> {
        let mut menu_action = None;
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if contents_tab(ui, self.site_map_ui.tab == SiteMapTab::Contents, "Contents")
                        .clicked()
                    {
                        self.site_map_ui.tab = SiteMapTab::Contents;
                    }
                    if contents_tab(ui, self.site_map_ui.tab == SiteMapTab::Issues, "Issues (0)")
                        .clicked()
                    {
                        self.site_map_ui.tab = SiteMapTab::Issues;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.menu_button("...", |ui| {
                            if ui.button("Copy visible URLs").clicked() {
                                menu_action = Some(SiteMapMenuAction::CopyUrls);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Expand all folders").clicked() {
                                menu_action = Some(SiteMapMenuAction::ExpandAll);
                                ui.close();
                            }
                            if ui.button("Collapse all folders").clicked() {
                                menu_action = Some(SiteMapMenuAction::CollapseAll);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Reset site map filter").clicked() {
                                menu_action = Some(SiteMapMenuAction::ResetFilter);
                                ui.close();
                            }
                            if !self.site_map_ui.search.is_empty()
                                && ui.button("Clear search").clicked()
                            {
                                menu_action = Some(SiteMapMenuAction::ClearSearch);
                                ui.close();
                            }
                        })
                        .response
                        .on_hover_text("Contents options");

                        let search_width = (ui.available_width() * 0.26).clamp(150.0, 260.0);
                        let response = ui.add(
                            TextEdit::singleline(&mut self.site_map_ui.search)
                                .id(egui::Id::unique(SITE_MAP_SEARCH_ID))
                                .hint_text("Search")
                                .margin(egui::Margin {
                                    left: 6,
                                    right: 22,
                                    top: 2,
                                    bottom: 2,
                                })
                                .desired_width(search_width),
                        );
                        paint_search_icon(ui, response.rect);
                    });
                });
            });
        ui.separator();

        if self.site_map_ui.tab == SiteMapTab::Issues {
            self.site_map_ui.visible_ids.clear();
            site_map_issues(ui);
            return None;
        }

        let Some(node) = self.site_map_ui.selected_node.clone() else {
            let filtered = self.site_map_ui.filter != SiteMapFilter::SHOW_ALL;
            let (title, body) = if filtered {
                (
                    "No visible items",
                    "The site map filter is hiding every item. Relax the filter or show all items.",
                )
            } else {
                (
                    "No node selected",
                    "Select a host, folder, or resource in the site map.",
                )
            };
            self.site_map_ui.visible_ids.clear();
            self.selected = None;
            self.detail = None;
            site_map_empty_contents(ui, &mut self.site_map_ui, title, body);
            return None;
        };

        let needle = self.site_map_ui.search.trim().to_lowercase();
        let filter = self.site_map_ui.filter;
        let mut rows: Vec<SiteRowRef<'_>> = Vec::new();
        for host in &map.hosts {
            for entry in &host.entries {
                if !filter.allows(entry) || !node.contains(entry) {
                    continue;
                }
                let notes = self.inspector.note(entry.id).unwrap_or_default();
                let item = self.history.get(entry.id);
                let row = SiteRowRef { entry, item, notes };
                if needle.is_empty() || site_row_matches(&row, &needle) {
                    rows.push(row);
                }
            }
        }

        if rows.is_empty() {
            let (title, body) = if !needle.is_empty() {
                (
                    "No contents match the search",
                    "Clear the Contents search or try a different term.",
                )
            } else {
                (
                    "No visible items",
                    "The site map filter is hiding everything under this node.",
                )
            };
            site_map_empty_contents(ui, &mut self.site_map_ui, title, body);
            self.site_map_ui.visible_ids.clear();
            self.selected = None;
            self.detail = None;
            return None;
        }

        sort_site_rows(
            &mut rows,
            self.site_map_ui.sort,
            self.site_map_ui.sort_ascending,
        );
        self.site_map_ui.visible_ids = rows.iter().map(|row| row.entry.id).collect();

        let selected_in_rows = self
            .selected
            .is_some_and(|id| rows.iter().any(|row| row.entry.id == id));
        if !selected_in_rows {
            let next = if node_changed {
                rows.iter()
                    .max_by_key(|row| row.entry.timestamp)
                    .map(|row| row.entry.id)
            } else {
                rows.first().map(|row| row.entry.id)
            };
            if self.selected != next {
                self.selected = next;
                self.detail = None;
            }
            self.site_map_ui.scroll_to_selected = next.is_some();
        }

        let mut clicked = None;
        let mut action = None;
        let mut clicked_sort = None;
        table_frame(ui, |ui| {
            // Scale the initial column widths to the available space so every
            // Burp column stays visible on smaller windows. The user can still
            // drag the dividers.
            let width = ui.available_width();
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::initial(width * 0.14).at_least(36.0))
                .column(Column::initial(width * 0.055).at_least(28.0))
                .column(Column::initial(width * 0.18).at_least(44.0))
                .column(Column::initial(width * 0.045).at_least(20.0))
                .column(Column::initial(width * 0.065).at_least(30.0))
                .column(Column::initial(width * 0.06).at_least(30.0))
                .column(Column::initial(width * 0.075).at_least(32.0))
                .column(Column::remainder().at_least(40.0))
                .column(Column::initial(width * 0.085).at_least(28.0))
                .column(Column::initial(width * 0.115).at_least(48.0));

            if self.site_map_ui.scroll_to_selected {
                if let Some(row) = self
                    .selected
                    .and_then(|id| rows.iter().position(|row| row.entry.id == id))
                {
                    table = table.scroll_to_row(row, Some(Align::Center));
                }
                self.site_map_ui.scroll_to_selected = false;
            }

            table
                .header(22.0, |mut header| {
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
                        header.col(|ui| {
                            if sort_header(
                                ui,
                                sort,
                                self.site_map_ui.sort,
                                self.site_map_ui.sort_ascending,
                            )
                            .clicked()
                            {
                                clicked_sort = Some(sort);
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows.len(), |mut row| {
                        let index = row.index();
                        let site_row = &rows[index];
                        let entry = site_row.entry;
                        row.set_selected(self.selected == Some(entry.id));

                        let host = format!("{}://{}", entry.scheme, entry.host);
                        row.col(|ui| {
                            ui.add(
                                Label::new(RichText::new(&host).small().color(theme::TEXT))
                                    .truncate(),
                            )
                            .on_hover_text(entry.url());
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&entry.method).small().color(theme::TEXT));
                        });
                        row.col(|ui| {
                            ui.add(
                                Label::new(RichText::new(&entry.path).small().color(theme::TEXT))
                                    .truncate(),
                            )
                            .on_hover_text(entry.url());
                        });
                        row.col(|ui| {
                            if entry.params > 0 {
                                let response =
                                    ui.allocate_response(egui::vec2(12.0, 12.0), Sense::hover());
                                let center = response.rect.center();
                                let stroke = Stroke::new(1.6, theme::GREEN);
                                ui.painter().line_segment(
                                    [
                                        center + egui::vec2(-3.5, 0.0),
                                        center + egui::vec2(-1.0, 2.6),
                                    ],
                                    stroke,
                                );
                                ui.painter().line_segment(
                                    [
                                        center + egui::vec2(-1.0, 2.6),
                                        center + egui::vec2(4.0, -3.0),
                                    ],
                                    stroke,
                                );
                                response.on_hover_text(if entry.params_uncertain {
                                    format!(
                                        "At least {} parameter{}; request body was not fully captured",
                                        entry.params,
                                        if entry.params == 1 { "" } else { "s" }
                                    )
                                } else {
                                    format!(
                                        "{} parameter{}",
                                        entry.params,
                                        if entry.params == 1 { "" } else { "s" }
                                    )
                                });
                            }
                        });
                        row.col(|ui| {
                            let color = entry
                                .status
                                .map(view::status_class_color)
                                .unwrap_or(theme::GRAY);
                            ui.label(RichText::new(entry.status_label()).small().color(color));
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(entry.length.to_string())
                                    .small()
                                    .color(theme::TEXT),
                            )
                            .on_hover_text(view::human_bytes(entry.length));
                        });
                        row.col(|ui| {
                            let mime = entry.mime_label();
                            ui.add(
                                Label::new(
                                    RichText::new(if mime.is_empty() { "-" } else { &mime })
                                        .small()
                                        .color(theme::TEXT),
                                )
                                .truncate(),
                            );
                        });
                        row.col(|ui| {
                            ui.add(
                                Label::new(RichText::new(&entry.title).small().color(theme::TEXT))
                                    .truncate(),
                            )
                            .on_hover_text(&entry.title);
                        });
                        row.col(|ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(site_row.notes)
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                )
                                .truncate(),
                            )
                            .on_hover_text(
                                if site_row.notes.is_empty() {
                                    "Add notes in Inspector > Notes"
                                } else {
                                    site_row.notes
                                },
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(site_map_time(entry.timestamp))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            )
                            .on_hover_text(format!(
                                "Latest of {} captured request{}",
                                entry.count,
                                if entry.count == 1 { "" } else { "s" }
                            ));
                        });

                        let hover = format!(
                            "{}\n{} request{} captured; showing exchange #{}",
                            entry.url(),
                            entry.count,
                            if entry.count == 1 { "" } else { "s" },
                            entry.id
                        );
                        let response = row.response().on_hover_text(hover);
                        if response.clicked() || response.secondary_clicked() {
                            clicked = Some(entry.id);
                        }
                        response.context_menu(|ui| {
                            let can_send = site_row.item.is_some();
                            if ui
                                .add_enabled(can_send, egui::Button::new("Send to Repeater"))
                                .clicked()
                            {
                                action = Some(SiteMapRowAction::Repeater);
                                ui.close();
                            }
                            if ui
                                .add_enabled(can_send, egui::Button::new("Send to Intruder"))
                                .clicked()
                            {
                                action = Some(SiteMapRowAction::Intruder);
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    can_send,
                                    egui::Button::new("Send request to Comparer"),
                                )
                                .clicked()
                            {
                                action = Some(SiteMapRowAction::ComparerRequest);
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    site_row.item.is_some_and(|item| item.response.is_some()),
                                    egui::Button::new("Send response to Comparer"),
                                )
                                .clicked()
                            {
                                action = Some(SiteMapRowAction::ComparerResponse);
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    site_row.item.is_some_and(|item| {
                                        item.response.is_some() && !item.is_in_flight()
                                    }),
                                    egui::Button::new("Send response to Sequencer"),
                                )
                                .clicked()
                            {
                                action = Some(SiteMapRowAction::Sequencer);
                                ui.close();
                            }
                            if let Some(item) = site_row.item {
                                super::curl::context_menu(ui, item);
                            }
                            if ui.button("Copy URL").clicked() {
                                ui.ctx().copy_text(entry.url());
                                ui.close();
                            }
                        });
                    });
                });
        });

        if let Some(sort) = clicked_sort {
            if self.site_map_ui.sort == sort {
                self.site_map_ui.sort_ascending = !self.site_map_ui.sort_ascending;
            } else {
                self.site_map_ui.sort = sort;
                self.site_map_ui.sort_ascending = true;
            }
        }
        if let Some(id) = clicked
            && self.selected != Some(id)
        {
            self.selected = Some(id);
            self.detail = None;
        }

        match menu_action {
            Some(SiteMapMenuAction::CopyUrls) => {
                let urls = rows
                    .iter()
                    .map(|row| row.entry.url())
                    .collect::<Vec<_>>()
                    .join("\n");
                ui.ctx().copy_text(urls);
            }
            Some(SiteMapMenuAction::ExpandAll) => {
                set_all_open(tree, &mut self.site_map_ui.open, true);
            }
            Some(SiteMapMenuAction::CollapseAll) => {
                set_all_open(tree, &mut self.site_map_ui.open, false);
            }
            Some(SiteMapMenuAction::ResetFilter) => {
                self.site_map_ui.filter = SiteMapFilter::default();
            }
            Some(SiteMapMenuAction::ClearSearch) => {
                self.site_map_ui.search.clear();
            }
            None => {}
        }

        action
    }

    fn apply_site_map_row_action(&mut self, action: SiteMapRowAction) {
        match action {
            SiteMapRowAction::Repeater => self.send_selected_to_repeater(),
            SiteMapRowAction::Intruder => self.send_selected_to_intruder(),
            SiteMapRowAction::Sequencer => self.send_selected_to_sequencer(),
            SiteMapRowAction::ComparerRequest => {
                self.send_selected_to_comparer(MessagePane::Request);
            }
            SiteMapRowAction::ComparerResponse => {
                self.send_selected_to_comparer(MessagePane::Response);
            }
        }
    }

    /// Move the Contents selection by `delta`, clamped to the visible rows.
    pub(super) fn move_site_map_selection(&mut self, delta: isize) {
        let len = self.site_map_ui.visible_ids.len();
        let current = self.selected.and_then(|id| {
            self.site_map_ui
                .visible_ids
                .iter()
                .position(|&row| row == id)
        });
        let Some(index) = super::selection::moved_row_index(len, current, delta) else {
            return;
        };
        self.select_site_map_row(index);
    }

    /// Select the first or last Contents row for Home/End.
    pub(super) fn select_site_map_edge(&mut self, first: bool) {
        if self.site_map_ui.visible_ids.is_empty() {
            return;
        }
        let index = if first {
            0
        } else {
            self.site_map_ui.visible_ids.len() - 1
        };
        self.select_site_map_row(index);
    }

    fn select_site_map_row(&mut self, index: usize) {
        let Some(&id) = self.site_map_ui.visible_ids.get(index) else {
            return;
        };
        self.select_id(id);
        self.site_map_ui.scroll_to_selected = true;
    }

    fn site_map_windows(&mut self, ctx: &egui::Context) {
        if self.site_map_ui.filter_open {
            let mut open = self.site_map_ui.filter_open;
            let mut close = false;
            let mut filter = self.site_map_ui.filter;
            let mut changed = false;
            egui::Window::new("Site map filter")
                .id(egui::Id::unique("perseid.site-map.filter-window"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(440.0)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(
                            "Filters only change what is displayed. Hidden traffic stays in the history and reappears when a filter is relaxed.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(8.0);

                    ui.label(
                        RichText::new("Request type")
                            .strong()
                            .color(theme::TEXT),
                    );
                    changed |= ui
                        .checkbox(
                            &mut filter.parameterized_only,
                            "Show only parameterized requests",
                        )
                        .changed();
                    changed |= ui
                        .checkbox(&mut filter.hide_not_found, "Hide not found items (404)")
                        .changed();
                    let mut all_requested = true;
                    ui.add_enabled(
                        false,
                        egui::Checkbox::new(&mut all_requested, "Show only requested items"),
                    )
                    .on_disabled_hover_text(
                        "Every Perseid site-map entry comes from a completed or in-flight request. Perseid does not infer unrequested URLs.",
                    );

                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("MIME type")
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.columns(2, |columns| {
                        changed |= columns[0]
                            .checkbox(&mut filter.hide_css, "Hide CSS")
                            .changed();
                        changed |= columns[0]
                            .checkbox(&mut filter.hide_images, "Hide images")
                            .changed();
                        changed |= columns[1]
                            .checkbox(&mut filter.hide_binary, "Hide general binary")
                            .changed();
                    });

                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Status code")
                            .strong()
                            .color(theme::TEXT),
                    );
                    changed |= ui
                        .checkbox(&mut filter.hide_4xx, "Hide 4xx responses")
                        .changed();

                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Folders")
                            .strong()
                            .color(theme::TEXT),
                    );
                    changed |= ui
                        .checkbox(&mut filter.hide_empty_folders, "Hide empty folders")
                        .changed();

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Reset to defaults").clicked() {
                            filter = SiteMapFilter::default();
                            changed = true;
                        }
                        if ui.button("Show all items").clicked() {
                            filter = SiteMapFilter::SHOW_ALL;
                            changed = true;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        });
                    });
                });
            self.site_map_ui.filter_open = open && !close;
            if changed {
                self.site_map_ui.filter = filter;
            }
        }

        if self.site_map_ui.help_open {
            let mut open = self.site_map_ui.help_open;
            egui::Window::new("Site map help")
                .id(egui::Id::unique("perseid.site-map.help-window"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(460.0)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Mapping captured traffic")
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "The site map is built from HTTP exchanges captured by the proxy. \
                         HTTPS requests appear after decryption. Hosts contain folders and \
                         resources; selecting a node filters the Contents table to that node \
                         and everything below it.",
                    );
                    ui.add_space(8.0);
                    ui.label(RichText::new("What this view does not do").strong());
                    ui.label(
                        "Perseid does not crawl or infer unrequested URLs. The crawl-paths \
                         view and scanner Issues are therefore unavailable. Capturing a URL \
                         through the proxy is what adds it to the map.",
                    );
                    ui.add_space(8.0);
                    ui.label(RichText::new("Filters").strong());
                    ui.label(
                        "The filter bar only changes what is displayed. Hidden items are never \
                         deleted; clearing a filter makes them visible again.",
                    );
                });
            self.site_map_ui.help_open = open;
        }
    }

    fn scope_view(&mut self, ui: &mut egui::Ui) {
        let captured_hosts = captured_hosts(&self.history);
        let mut dirty = false;
        let mut settings_dirty = false;
        let mut clear_scope = false;

        page_pad(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_max_width(780.0);
                    card(ui, |ui| {
                        ui.label(
                        RichText::new("Scope rules")
                            .size(14.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        "Scope filters the site map and HTTP history. It can also block \
                         out-of-scope traffic before it is forwarded.",
                    );
                    ui.add_space(8.0);
                    let mut enabled = self.scope_enabled;
                    if ui
                        .checkbox(&mut enabled, "Filter views to the scope")
                        .changed()
                    {
                        self.scope_enabled = enabled;
                        dirty = true;
                    }
                    let mut enforce = self.scope_enforce;
                    if ui
                        .checkbox(&mut enforce, "Block out-of-scope traffic")
                        .on_hover_text(
                            "Return 403 for requests and CONNECT tunnels outside the scope. \
                             An empty scope allows every host.",
                        )
                        .changed()
                    {
                        self.scope_enforce = enforce;
                        settings_dirty = true;
                    }
                    if (self.scope_enabled || self.scope_enforce) && self.scope.is_empty() {
                        ui.label(
                            RichText::new(
                                "No rules yet, so every host is considered in scope and nothing is blocked.",
                            )
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label("Include host");
                        let response = ui.add(
                            TextEdit::singleline(&mut self.scope_input)
                                .hint_text("example.com or https://example.com/path")
                                .desired_width(340.0),
                        );
                        let enter =
                            response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                        if (ui.button("Add").clicked() || enter)
                            && let Some(rule) =
                                site_map::normalize_scope_input(&self.scope_input)
                        {
                            if self.scope.insert(rule) {
                                dirty = true;
                                settings_dirty = true;
                            }
                            self.scope_input.clear();
                        }
                    });

                    ui.add_space(8.0);
                    if self.scope.is_empty() {
                        ui.label(
                            RichText::new("No explicit scope rules.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    } else {
                        let rules: Vec<String> = self.scope.iter().cloned().collect();
                        for rule in rules {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&rule).monospace().color(theme::TEXT));
                                if ui.small_button("Remove").clicked() {
                                    self.scope.remove(&rule);
                                    dirty = true;
                                    settings_dirty = true;
                                }
                            });
                        }
                        if ui.button("Clear scope").clicked() {
                            clear_scope = true;
                        }
                    }
                });

                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.label(
                        RichText::new("Captured hosts")
                            .size(13.5)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        RichText::new(
                            "Add a host from observed traffic. Matching also covers subdomains.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(4.0);
                    if captured_hosts.is_empty() {
                        ui.label(
                            RichText::new("No hosts captured yet.")
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                    for (host, count) in &captured_hosts {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(host).monospace().color(theme::TEXT));
                            ui.label(
                                RichText::new(format!("x{count}"))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if !self.scope.is_empty()
                                && site_map::host_in_scope(host, &self.scope)
                            {
                                ui.label(
                                    RichText::new("in scope")
                                        .small()
                                        .color(theme::GREEN),
                                );
                            } else if ui.small_button("Add").clicked() {
                                self.scope.insert(host.clone());
                                dirty = true;
                                settings_dirty = true;
                            }
                        });
                    }
                });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "When blocking is enabled, scope is checked before interception and again \
                         after edits, so a rewrite cannot move a request outside the guardrail \
                         unnoticed.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            });
        });

        if clear_scope {
            self.scope.clear();
            dirty = true;
            settings_dirty = true;
        }
        if dirty {
            self.invalidate_views();
        }
        if settings_dirty {
            self.apply_proxy_settings();
        }
    }
}

fn site_map_tree(
    ui: &mut egui::Ui,
    tree: &[SiteTreeNode],
    state: &mut SiteMapUi,
) -> Option<SiteNodeId> {
    let mut picked = None;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if tree.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("No items match the site map filter.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                return;
            }
            for node in tree {
                tree_node_row(ui, node, 0, &[], true, state, &mut picked);
            }
        });
    picked
}

#[allow(clippy::too_many_arguments)]
fn tree_node_row(
    ui: &mut egui::Ui,
    node: &SiteTreeNode,
    depth: usize,
    ancestors_continue: &[bool],
    is_last: bool,
    state: &mut SiteMapUi,
    picked: &mut Option<SiteNodeId>,
) {
    let key = node.id.key();
    let default_open = depth == 0
        || matches!(
            &node.id,
            SiteNodeId::Directory { path, .. } if path == "/"
        );
    let open = *state.open.entry(key.clone()).or_insert(default_open);
    let has_children = !node.children.is_empty();
    let selected = state.selected_node.as_ref() == Some(&node.id);

    let indent = 4.0 + depth as f32 * 16.0;
    let row_height = 21.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, egui::CornerRadius::same(2), theme::SELECTED);
            painter.rect_filled(
                Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
                egui::CornerRadius::ZERO,
                theme::ACCENT,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, egui::CornerRadius::same(2), theme::ACCENT_WASH);
        }

        paint_tree_guides(
            painter,
            rect,
            depth,
            ancestors_continue,
            is_last,
            open && has_children,
            indent + 6.0,
        );

        let arrow_center = egui::pos2(rect.left() + indent + 6.0, rect.center().y);
        if has_children {
            paint_chevron(painter, arrow_center, open, theme::TEXT_MUTED);
        }

        let icon_center = egui::pos2(rect.left() + indent + 22.0, rect.center().y);
        match &node.id {
            SiteNodeId::Host { .. } => {
                if node.scheme.eq_ignore_ascii_case("https") {
                    paint_lock(painter, icon_center, theme::BLUE);
                } else if node.scheme.eq_ignore_ascii_case("mixed") {
                    paint_mixed_scheme(painter, icon_center);
                } else {
                    paint_globe(painter, icon_center, theme::GRAY);
                }
            }
            SiteNodeId::Directory { path, .. } if path == "/" => {
                paint_gear_icon(painter, icon_center, theme::GRAY);
            }
            SiteNodeId::Directory { .. } => {
                paint_folder(painter, icon_center);
            }
            SiteNodeId::Endpoint { .. } => {
                paint_file(painter, icon_center);
            }
        }

        painter
            .with_clip_rect(rect.shrink2(egui::vec2(0.0, 1.0)))
            .text(
                egui::pos2(rect.left() + indent + 34.0, rect.center().y),
                Align2::LEFT_CENTER,
                &node.label,
                FontId::proportional(11.8),
                theme::TEXT,
            );
    }

    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, selected, &node.label)
    });
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    let hover = match &node.id {
        SiteNodeId::Host { host } => {
            let scheme = match node.scheme.as_str() {
                "https" => "HTTPS",
                "http" => "HTTP",
                "mixed" => "HTTP and HTTPS",
                _ => "captured",
            };
            format!(
                "{host} — {scheme}; {} captured endpoint{}",
                node.count,
                plural(node.count)
            )
        }
        SiteNodeId::Directory { .. } => {
            format!(
                "{} endpoint{} in this folder",
                node.count,
                plural(node.count)
            )
        }
        SiteNodeId::Endpoint { .. } => {
            let methods = if node.methods.is_empty() {
                "captured resource".to_string()
            } else {
                node.methods.join(", ")
            };
            format!(
                "{methods} — {} captured URL{}",
                node.count,
                plural(node.count)
            )
        }
    };
    let response = response.on_hover_text(hover);

    if has_children {
        let arrow_rect = Rect::from_center_size(
            egui::pos2(rect.left() + indent + 6.0, rect.center().y),
            egui::vec2(14.0, 18.0),
        );
        let arrow = ui
            .interact(
                arrow_rect,
                ui.scope_id().with(("site-map-arrow", &key)),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if arrow.clicked() {
            state.open.insert(key, !open);
            return;
        }
    }
    if response.clicked() {
        *picked = Some(node.id.clone());
    }

    if open {
        for (index, child) in node.children.iter().enumerate() {
            let mut ancestors = ancestors_continue.to_vec();
            ancestors.push(!is_last);
            let child_is_last = index + 1 == node.children.len();
            tree_node_row(
                ui,
                child,
                depth + 1,
                &ancestors,
                child_is_last,
                state,
                picked,
            );
        }
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn site_map_issues(ui: &mut egui::Ui) {
    empty_state(
        ui,
        "No issues to show",
        "Perseid does not run a vulnerability scanner. Issues from Burp Scanner or another \
         scanner are not imported into this workspace.",
    );
}

fn target_issues_view(ui: &mut egui::Ui) {
    page_pad(ui, |ui| {
        empty_state(
            ui,
            "No issues to show",
            "Perseid does not run a vulnerability scanner, so it has no Target issues to list. \
             Captured requests are still available in Target > Site map and Proxy > HTTP history.",
        );
    });
}

fn site_map_empty_contents(ui: &mut egui::Ui, state: &mut SiteMapUi, title: &str, body: &str) {
    ui.add_space(38.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(14.0).strong().color(theme::TEXT));
        ui.add_space(3.0);
        ui.label(RichText::new(body).color(theme::TEXT_MUTED));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if !state.search.is_empty() && ui.button("Clear search").clicked() {
                state.search.clear();
            }
            if state.filter != SiteMapFilter::SHOW_ALL && ui.button("Show all items").clicked() {
                state.filter = SiteMapFilter::SHOW_ALL;
            }
        });
    });
}

fn site_row_matches(row: &SiteRowRef<'_>, needle: &str) -> bool {
    let entry = row.entry;
    let status = entry.status_label();
    let url = entry.url();
    [
        entry.host.as_str(),
        entry.method.as_str(),
        entry.path.as_str(),
        url.as_str(),
        entry.title.as_str(),
        entry.mime.as_str(),
        row.notes,
        status.as_str(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(needle))
        || entry.params > 0 && "params parameter".contains(needle)
}

fn sort_site_rows(rows: &mut [SiteRowRef<'_>], sort: SiteMapSort, ascending: bool) {
    rows.sort_by(|left, right| {
        let order = compare_site_rows(left, right, sort);
        let order = if ascending { order } else { order.reverse() };
        order
            .then_with(|| left.entry.path.cmp(&right.entry.path))
            .then_with(|| left.entry.host.cmp(&right.entry.host))
            .then_with(|| left.entry.method.cmp(&right.entry.method))
    });
}

fn compare_site_rows(
    left: &SiteRowRef<'_>,
    right: &SiteRowRef<'_>,
    sort: SiteMapSort,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let l = left.entry;
    let r = right.entry;
    match sort {
        SiteMapSort::Host => l
            .host
            .to_ascii_lowercase()
            .cmp(&r.host.to_ascii_lowercase())
            .then_with(|| l.path.cmp(&r.path)),
        SiteMapSort::Method => l.method.cmp(&r.method),
        SiteMapSort::Url => l.path.cmp(&r.path),
        SiteMapSort::Params => l.params.cmp(&r.params),
        SiteMapSort::Status => l
            .status
            .unwrap_or(u16::MAX)
            .cmp(&r.status.unwrap_or(u16::MAX)),
        SiteMapSort::Length => l.length.cmp(&r.length),
        SiteMapSort::Mime => l
            .mime_label()
            .to_ascii_lowercase()
            .cmp(&r.mime_label().to_ascii_lowercase()),
        SiteMapSort::Title => l
            .title
            .to_ascii_lowercase()
            .cmp(&r.title.to_ascii_lowercase()),
        SiteMapSort::Notes => left
            .notes
            .to_ascii_lowercase()
            .cmp(&right.notes.to_ascii_lowercase()),
        SiteMapSort::Time => match l.timestamp.cmp(&r.timestamp) {
            Ordering::Equal => l.id.cmp(&r.id),
            order => order,
        },
    }
}

fn sort_header(
    ui: &mut egui::Ui,
    sort: SiteMapSort,
    active: SiteMapSort,
    ascending: bool,
) -> egui::Response {
    let mut label = sort.label().to_string();
    if sort == active {
        label.push_str(if ascending { " ^" } else { " v" });
    }
    ui.add(
        Label::new(RichText::new(label).strong().small().color(theme::TEXT)).sense(Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .on_hover_text("Click to sort")
}

fn contents_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    section_tab(ui, selected, label)
}

fn site_map_time(time: SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    local.format("%H:%M:%S %d %b").to_string()
}

fn set_all_open(tree: &[SiteTreeNode], open: &mut BTreeMap<String, bool>, value: bool) {
    for node in tree {
        open.insert(node.id.key(), value);
        set_all_open(&node.children, open, value);
    }
}

fn filter_bar(ui: &mut egui::Ui, width: f32, summary: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 25.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() {
            theme::ACCENT_WASH
        } else {
            theme::SURFACE_ALT
        };
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(3),
            fill,
            Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );
        paint_funnel(
            ui.painter(),
            egui::pos2(rect.left() + 13.0, rect.center().y),
            theme::TEXT_MUTED,
        );
        let clip = Rect::from_min_max(
            egui::pos2(rect.left() + 25.0, rect.top()),
            egui::pos2(rect.right() - 7.0, rect.bottom()),
        );
        ui.painter().with_clip_rect(clip).text(
            egui::pos2(rect.left() + 26.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!("Site map filter: {summary}"),
            FontId::proportional(11.5),
            theme::TEXT,
        );
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Site map filter"));
    response
}

fn icon_button(ui: &mut egui::Ui, icon: SiteMapIcon, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 25.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if active {
            theme::ACCENT_SOFT
        } else if response.hovered() {
            theme::ACCENT_WASH
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(3), fill);
        }
        let color = if active {
            theme::ACCENT_DARK
        } else {
            theme::TEXT_MUTED
        };
        let center = rect.center();
        match icon {
            SiteMapIcon::Help => {
                ui.painter()
                    .circle_stroke(center, 7.0, Stroke::new(1.2, color));
                ui.painter().text(
                    center,
                    Align2::CENTER_CENTER,
                    "?",
                    FontId::proportional(11.0),
                    color,
                );
            }
            SiteMapIcon::Pause => {
                ui.painter().rect_filled(
                    Rect::from_center_size(center + egui::vec2(-3.0, 0.0), egui::vec2(2.4, 10.0)),
                    0.0,
                    color,
                );
                ui.painter().rect_filled(
                    Rect::from_center_size(center + egui::vec2(3.0, 0.0), egui::vec2(2.4, 10.0)),
                    0.0,
                    color,
                );
            }
            SiteMapIcon::Play => {
                ui.painter().add(Shape::convex_polygon(
                    vec![
                        center + egui::vec2(-4.0, -6.0),
                        center + egui::vec2(-4.0, 6.0),
                        center + egui::vec2(6.0, 0.0),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
            SiteMapIcon::Settings => {
                for (offset, knob) in [(-5.0_f32, -3.0_f32), (0.0, 2.0), (5.0, -1.0)] {
                    let y = center.y + offset;
                    ui.painter().line_segment(
                        [egui::pos2(center.x - 7.0, y), egui::pos2(center.x + 7.0, y)],
                        Stroke::new(1.3, color),
                    );
                    ui.painter()
                        .circle_filled(egui::pos2(center.x + knob, y), 2.0, color);
                }
            }
        }
    }
    let label = match icon {
        SiteMapIcon::Help => "Site map help",
        SiteMapIcon::Pause => "Pause live updates",
        SiteMapIcon::Play => "Resume live updates",
        SiteMapIcon::Settings => "Site map filter settings",
    };
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, active, label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_search_icon(ui: &egui::Ui, rect: Rect) {
    let center = egui::pos2(rect.right() - 12.0, rect.center().y - 0.5);
    let stroke = Stroke::new(1.2, theme::TEXT_MUTED);
    ui.painter().circle_stroke(center, 3.5, stroke);
    ui.painter().line_segment(
        [center + egui::vec2(2.5, 2.5), center + egui::vec2(5.5, 5.5)],
        stroke,
    );
}

/// Faint hierarchy lines and junction dots, drawn to match Burp's tree.
fn paint_tree_guides(
    painter: &egui::Painter,
    rect: Rect,
    depth: usize,
    ancestors_continue: &[bool],
    is_last: bool,
    expanded: bool,
    child_guide_offset: f32,
) {
    const GUIDE: Color32 = Color32::from_rgb(0xcf, 0xc6, 0xbb);
    const GUIDE_DOT: Color32 = Color32::from_rgb(0xb3, 0xa9, 0x9d);
    let stroke = Stroke::new(1.0, GUIDE);
    for (column, continues) in ancestors_continue
        .iter()
        .enumerate()
        .take(depth.saturating_sub(1))
    {
        if *continues {
            let x = rect.left() + 4.0 + column as f32 * 16.0 + 6.0;
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                stroke,
            );
        }
    }

    if depth > 0 {
        let x = rect.left() + 4.0 + (depth as f32 - 1.0) * 16.0 + 6.0;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.center().y)],
            stroke,
        );
        if !is_last {
            painter.line_segment(
                [egui::pos2(x, rect.center().y), egui::pos2(x, rect.bottom())],
                stroke,
            );
        }
        let icon_left = rect.left() + 4.0 + depth as f32 * 16.0 + 16.0;
        painter.line_segment(
            [
                egui::pos2(x, rect.center().y),
                egui::pos2(icon_left, rect.center().y),
            ],
            stroke,
        );
        painter.circle_filled(egui::pos2(x, rect.center().y), 1.5, GUIDE_DOT);
    }

    if expanded {
        let x = rect.left() + child_guide_offset;
        painter.line_segment(
            [egui::pos2(x, rect.center().y), egui::pos2(x, rect.bottom())],
            stroke,
        );
    }
}

fn paint_chevron(painter: &egui::Painter, center: egui::Pos2, open: bool, color: Color32) {
    let points = if open {
        vec![
            center + egui::vec2(-3.0, -1.5),
            center + egui::vec2(3.0, -1.5),
            center + egui::vec2(0.0, 2.5),
        ]
    } else {
        vec![
            center + egui::vec2(-1.5, -3.0),
            center + egui::vec2(2.5, 0.0),
            center + egui::vec2(-1.5, 3.0),
        ]
    };
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

fn paint_folder(painter: &egui::Painter, center: egui::Pos2) {
    let stroke = Stroke::new(1.2, theme::GRAY);
    let left = center.x - 6.0;
    let right = center.x + 6.0;
    let top = center.y - 4.0;
    let bottom = center.y + 5.0;
    painter.add(Shape::closed_line(
        vec![
            egui::pos2(left, top),
            egui::pos2(center.x - 1.0, top),
            egui::pos2(center.x + 1.0, top + 2.0),
            egui::pos2(right, top + 2.0),
            egui::pos2(right, bottom),
            egui::pos2(left, bottom),
        ],
        stroke,
    ));
}

fn paint_file(painter: &egui::Painter, center: egui::Pos2) {
    let page = Rect::from_center_size(center, egui::vec2(9.0, 11.0));
    let stroke = Stroke::new(1.0, theme::GRAY);
    painter.rect(
        page,
        egui::CornerRadius::same(1),
        theme::SURFACE,
        stroke,
        egui::StrokeKind::Inside,
    );
    painter.add(Shape::line(
        vec![
            page.right_top() + egui::vec2(-3.0, 1.0),
            page.right_top() + egui::vec2(-3.0, 3.5),
            page.right_top() + egui::vec2(-0.5, 3.5),
        ],
        stroke,
    ));
    painter.line_segment(
        [
            egui::pos2(page.left() + 2.0, page.center().y + 1.0),
            egui::pos2(page.right() - 2.0, page.center().y + 1.0),
        ],
        Stroke::new(0.8, theme::GRAY),
    );
}

/// Burp uses a gear for the host's root path `/`.
fn paint_gear_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.1, color);
    painter.circle_stroke(center, 3.8, stroke);
    painter.circle_stroke(center, 1.3, stroke);
    for tooth in 0..8 {
        let angle = tooth as f32 * std::f32::consts::FRAC_PI_4;
        let direction = egui::vec2(angle.cos(), angle.sin());
        painter.line_segment([center + direction * 4.2, center + direction * 5.8], stroke);
    }
}

fn paint_lock(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let body = Rect::from_center_size(center + egui::vec2(0.0, 2.0), egui::vec2(8.0, 6.0));
    painter.rect_filled(body, egui::CornerRadius::same(1), color);
    painter.add(Shape::line(
        vec![
            center + egui::vec2(-3.0, 0.0),
            center + egui::vec2(-3.0, -3.0),
            center + egui::vec2(0.0, -5.0),
            center + egui::vec2(3.0, -3.0),
            center + egui::vec2(3.0, 0.0),
        ],
        Stroke::new(1.2, color),
    ));
    painter.circle_filled(center + egui::vec2(0.0, 2.0), 1.0, theme::SURFACE);
}

fn paint_globe(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    painter.circle_stroke(center, 5.0, Stroke::new(1.1, color));
    painter.line_segment(
        [
            egui::pos2(center.x - 5.0, center.y),
            egui::pos2(center.x + 5.0, center.y),
        ],
        Stroke::new(1.0, color),
    );
    painter.add(Shape::line(
        vec![
            center + egui::vec2(0.0, -5.0),
            center + egui::vec2(2.3, -2.5),
            center + egui::vec2(2.3, 2.5),
            center + egui::vec2(0.0, 5.0),
            center + egui::vec2(-2.3, 2.5),
            center + egui::vec2(-2.3, -2.5),
            center + egui::vec2(0.0, -5.0),
        ],
        Stroke::new(1.0, color),
    ));
}

/// A host captured over both HTTP and HTTPS: a globe with a small lock badge.
fn paint_mixed_scheme(painter: &egui::Painter, center: egui::Pos2) {
    paint_globe(painter, center + egui::vec2(-3.5, 0.0), theme::GRAY);
    let lock = center + egui::vec2(4.5, 3.0);
    let body = Rect::from_center_size(lock, egui::vec2(5.0, 4.0));
    painter.rect_stroke(
        body,
        egui::CornerRadius::same(1),
        Stroke::new(1.0, theme::BLUE),
        egui::StrokeKind::Inside,
    );
    painter.add(Shape::line(
        vec![
            lock + egui::vec2(-1.8, -2.0),
            lock + egui::vec2(-1.8, -3.0),
            lock + egui::vec2(0.0, -4.0),
            lock + egui::vec2(1.8, -3.0),
            lock + egui::vec2(1.8, -2.0),
        ],
        Stroke::new(1.0, theme::BLUE),
    ));
}

fn paint_funnel(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    painter.add(Shape::line(
        vec![
            egui::pos2(center.x - 5.0, center.y - 4.0),
            egui::pos2(center.x + 5.0, center.y - 4.0),
            egui::pos2(center.x + 1.0, center.y + 1.0),
            egui::pos2(center.x + 1.0, center.y + 5.0),
            egui::pos2(center.x - 1.0, center.y + 3.5),
            egui::pos2(center.x - 1.0, center.y + 1.0),
            egui::pos2(center.x - 5.0, center.y - 4.0),
        ],
        stroke,
    ));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, SystemTime};

    use crate::history::{FALLBACK_HISTORY_BYTES, History};
    use crate::proxy::{
        CapturedBody, ExchangeEnd, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead,
    };
    use crate::site_map::SiteMapFilter;

    use super::*;

    /// Build history rows the way the proxy reports captured exchanges.
    fn history(entries: &[(&str, &str, &str)]) -> History {
        let mut history = History::new(100, FALLBACK_HISTORY_BYTES);
        for (index, (method, host, path)) in entries.iter().enumerate() {
            let id = index as u64 + 1;
            history.apply(ProxyEvent::Started {
                id,
                timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(id),
                request: RequestInfo {
                    method: method.to_string(),
                    uri: format!("http://{host}{path}"),
                    scheme: "http".to_string(),
                    host: host.to_string(),
                    path: path.to_string(),
                    version: "HTTP/1.1".to_string(),
                    headers: Vec::new(),
                },
            });
        }
        history
    }

    #[test]
    fn captured_host_list_counts_each_host_and_sorts_by_name() {
        let history = history(&[
            ("GET", "zeta.test", "/"),
            ("GET", "alpha.test", "/a"),
            ("POST", "alpha.test", "/b"),
        ]);
        assert_eq!(
            captured_hosts(&history),
            vec![("alpha.test".to_string(), 2), ("zeta.test".to_string(), 1)]
        );
    }

    #[test]
    fn captured_host_list_skips_requests_without_a_host() {
        let history = history(&[("GET", "", "/orphan"), ("GET", "alpha.test", "/")]);
        assert_eq!(
            captured_hosts(&history),
            vec![("alpha.test".to_string(), 1)]
        );
    }

    #[test]
    fn site_map_tree_groups_endpoints_under_their_host() {
        let history = history(&[
            ("GET", "example.com", "/a"),
            ("POST", "example.com", "/a"),
            ("GET", "api.example.com", "/v1"),
        ]);
        let map = SiteMap::build(&history, None);
        let tree = map.tree(SiteMapFilter::SHOW_ALL);
        let hosts: Vec<&str> = tree
            .iter()
            .map(|host| match &host.id {
                SiteNodeId::Host { host } => host.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(hosts, vec!["api.example.com", "example.com"]);
        // Host labels are bare hostnames; the scheme is carried separately so
        // the tree can choose a lock or globe icon without widening the label.
        assert_eq!(tree[0].label, "api.example.com");
        assert_eq!(tree[0].scheme, "http");
        assert_eq!(tree[1].label, "example.com");
        assert_eq!(tree[1].scheme, "http");
        assert_eq!(map.endpoint_count(), 3);
        let example = &tree[1];
        let leaf = example.children[0]
            .find(&SiteNodeId::Endpoint {
                host: "example.com".to_string(),
                path: "/a".to_string(),
            })
            .expect("shared path leaf");
        assert_eq!(leaf.methods, vec!["GET".to_string(), "POST".to_string()]);
        assert_eq!(leaf.count, 2);
    }

    #[test]
    fn site_map_keyboard_selection_moves_between_visible_rows() {
        let mut app = PerseidApp::test_app();
        app.site_map_ui.visible_ids = vec![10, 20, 30];
        app.selected = Some(20);

        app.move_site_map_selection(1);
        assert_eq!(app.selected, Some(30));
        app.move_site_map_selection(1);
        assert_eq!(app.selected, Some(30));

        app.select_site_map_edge(true);
        assert_eq!(app.selected, Some(10));
        app.select_site_map_edge(false);
        assert_eq!(app.selected, Some(30));

        // With no visible rows the selection is left untouched.
        app.site_map_ui.visible_ids.clear();
        app.move_site_map_selection(-1);
        assert_eq!(app.selected, Some(30));
    }

    #[test]
    fn site_map_tree_hides_hosts_outside_the_scope() {
        let scope = BTreeSet::from(["example.com".to_string()]);
        let history = history(&[
            ("GET", "example.com", "/"),
            ("GET", "api.example.com", "/v1"),
            ("GET", "other.test", "/"),
        ]);
        let map = SiteMap::build(&history, Some(&scope));
        let tree = map.tree(SiteMapFilter::SHOW_ALL);
        let hosts: Vec<&str> = tree
            .iter()
            .map(|host| match &host.id {
                SiteNodeId::Host { host } => host.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(hosts, vec!["api.example.com", "example.com"]);
    }

    #[test]
    fn default_filter_matches_burp_filter_bar() {
        assert_eq!(
            SiteMapFilter::default().summary(),
            "Hiding not found items; hiding CSS, image and general binary content; hiding 4xx responses; hiding empty folders"
        );
    }

    #[test]
    fn scope_helper_treats_every_host_as_in_scope_without_rules() {
        assert!(site_map::host_in_scope("anything.test", &BTreeSet::new()));
    }

    #[test]
    fn scope_helper_matches_subdomains_and_ignores_ports() {
        let scope = BTreeSet::from(["example.com".to_string()]);
        assert!(site_map::host_in_scope("example.com", &scope));
        assert!(site_map::host_in_scope("example.com:8443", &scope));
        assert!(site_map::host_in_scope("api.example.com", &scope));
        assert!(!site_map::host_in_scope("notexample.com", &scope));
    }

    #[test]
    fn scope_input_normalizes_urls_ports_and_case() {
        assert_eq!(
            site_map::normalize_scope_input(" https://Example.com/path "),
            Some("example.com".to_string())
        );
        assert_eq!(
            site_map::normalize_scope_input("example.com:8443"),
            Some("example.com:8443".to_string())
        );
        assert_eq!(site_map::normalize_scope_input("   "), None);
    }

    fn screen() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        }
    }

    fn test_context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        // Keep widget positions stable between the frames used to synthesize
        // pointer events.
        ctx.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        ctx
    }

    fn target_frame(
        ctx: &egui::Context,
        app: &mut PerseidApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut input = screen();
        input.events = events;
        let mut output = ctx.run_ui(input, |ui| app.target_tab(ui));
        output.textures_delta.clear();
        output
    }

    fn rendered_text(output: &egui::FullOutput) -> String {
        fn collect(shape: &egui::Shape, text: &mut String) {
            match shape {
                egui::Shape::Text(shape) => {
                    text.push_str(shape.galley.text());
                    text.push('\n');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, text);
                    }
                }
                _ => {}
            }
        }
        let mut text = String::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }

    /// Find the on-screen bounds of an accesskit node by visible label.
    ///
    /// Rendering with accessibility enabled lets tests drive widgets without
    /// hard-coding layout coordinates, so the assertions survive styling
    /// changes.
    fn accesskit_rect(
        output: &egui::FullOutput,
        needle: &str,
        require_click: bool,
    ) -> Option<Rect> {
        let update = output.platform_output.accesskit_update.as_ref()?;
        let mut fallback = None;
        for (_, node) in &update.nodes {
            let Some(label) = node.label() else {
                continue;
            };
            if label != needle && !label.contains(needle) {
                continue;
            }
            let Some(bounds) = node.bounds() else {
                continue;
            };
            let rect = Rect::from_min_max(
                egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
                egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
            );
            let clickable = node.supports_action(egui::accesskit::Action::Click);
            let candidate = !require_click || clickable;
            if label == needle && candidate {
                return Some(rect);
            }
            if fallback.is_none() && candidate {
                fallback = Some(rect);
            }
        }
        fallback
    }

    /// Bounds of a drawn text run. Unlike accesskit, this also finds plain
    /// labels such as sort headers that are clickable but not exposed as
    /// accessibility buttons.
    fn rendered_text_rect(output: &egui::FullOutput, needle: &str) -> Option<Rect> {
        fn collect(shape: &egui::Shape, clip: Rect, needle: &str, found: &mut Option<Rect>) {
            match shape {
                egui::Shape::Text(shape) => {
                    let text = shape.galley.text();
                    if text == needle || (found.is_none() && text.contains(needle)) {
                        let rect =
                            Rect::from_min_size(shape.pos, shape.galley.size()).intersect(clip);
                        if rect.is_positive() {
                            *found = Some(rect);
                        }
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, clip, needle, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = None;
        for clipped in &output.shapes {
            collect(&clipped.shape, clipped.clip_rect, needle, &mut found);
        }
        found
    }

    fn pointer_button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn click(
        ctx: &egui::Context,
        app: &mut PerseidApp,
        label: &str,
        button: egui::PointerButton,
    ) -> bool {
        let output = target_frame(ctx, app, Vec::new());
        let Some(rect) = accesskit_rect(&output, label, false) else {
            return false;
        };
        let pos = rect.center();
        target_frame(ctx, app, vec![egui::Event::PointerMoved(pos)]);
        target_frame(ctx, app, vec![pointer_button(pos, button, true)]);
        target_frame(ctx, app, vec![pointer_button(pos, button, false)]);
        true
    }

    fn click_primary(ctx: &egui::Context, app: &mut PerseidApp, label: &str) -> bool {
        click(ctx, app, label, egui::PointerButton::Primary)
    }

    fn click_at(
        ctx: &egui::Context,
        app: &mut PerseidApp,
        pos: egui::Pos2,
        button: egui::PointerButton,
    ) {
        target_frame(ctx, app, vec![egui::Event::PointerMoved(pos)]);
        target_frame(ctx, app, vec![pointer_button(pos, button, true)]);
        target_frame(ctx, app, vec![pointer_button(pos, button, false)]);
    }

    fn click_text(
        ctx: &egui::Context,
        app: &mut PerseidApp,
        label: &str,
        button: egui::PointerButton,
    ) -> bool {
        let output = target_frame(ctx, app, Vec::new());
        let Some(rect) = rendered_text_rect(&output, label) else {
            return false;
        };
        click_at(ctx, app, rect.center(), button);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn capture(
        history: &mut History,
        id: u64,
        method: &str,
        scheme: &str,
        host: &str,
        path: &str,
        status: u16,
        mime: &str,
        body: &[u8],
        request_body: &[u8],
        request_headers: &[(&str, &str)],
    ) {
        history.apply(ProxyEvent::Started {
            id,
            timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(id),
            request: RequestInfo {
                method: method.to_string(),
                uri: format!("{scheme}://{host}{path}"),
                scheme: scheme.to_string(),
                host: host.to_string(),
                path: path.to_string(),
                version: "HTTP/1.1".to_string(),
                headers: request_headers
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
            },
        });
        let mut headers = Vec::new();
        if !mime.is_empty() {
            headers.push(("content-type".to_string(), mime.to_string()));
        }
        history.apply(ProxyEvent::ResponseHead {
            id,
            head: ResponseHead {
                version: "HTTP/1.1".to_string(),
                status,
                reason: String::new(),
                headers,
            },
        });
        history.apply(ProxyEvent::Finished {
            id,
            end: ExchangeEnd {
                duration: Duration::from_millis(5),
                request_body: CapturedBody::from_bytes(request_body.to_vec()),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(body.to_vec()),
                },
            },
        });
    }

    /// App with a small, varied capture set that exercises every Contents
    /// column, both schemes, mixed-scheme hosts, filtered MIME types, and
    /// parameterized form bodies.
    fn captured_app() -> PerseidApp {
        let mut app = PerseidApp::test_app();
        let mut history = History::new(100, FALLBACK_HISTORY_BYTES);
        capture(
            &mut history,
            1,
            "GET",
            "http",
            "example.com",
            "/",
            200,
            "text/html",
            b"<html><head><title>Home &amp; Away</title></head></html>",
            b"",
            &[],
        );
        capture(
            &mut history,
            2,
            "GET",
            "https",
            "example.com",
            "/products?sort=price",
            200,
            "text/html",
            b"<title>Products</title>",
            b"",
            &[],
        );
        capture(
            &mut history,
            3,
            "POST",
            "https",
            "example.com",
            "/products",
            302,
            "text/html",
            b"",
            b"name=Demo&price=10",
            &[("content-type", "application/x-www-form-urlencoded")],
        );
        capture(
            &mut history,
            4,
            "GET",
            "http",
            "example.com",
            "/assets/site.css",
            200,
            "text/css",
            b"body{}",
            b"",
            &[],
        );
        capture(
            &mut history,
            5,
            "GET",
            "http",
            "example.com",
            "/missing",
            404,
            "text/html",
            b"not found",
            b"",
            &[],
        );
        capture(
            &mut history,
            6,
            "GET",
            "https",
            "api.example.com",
            "/v1/items?limit=10",
            200,
            "application/json",
            br#"{"items":[]}"#,
            b"",
            &[],
        );
        capture(
            &mut history,
            7,
            "GET",
            "http",
            "mixed.test",
            "/plain",
            200,
            "text/html",
            b"<title>Plain</title>",
            b"",
            &[],
        );
        capture(
            &mut history,
            8,
            "GET",
            "https",
            "mixed.test",
            "/secure",
            200,
            "text/html",
            b"<title>Secure</title>",
            b"",
            &[],
        );
        capture(
            &mut history,
            9,
            "GET",
            "https",
            "example.com",
            "/logo.png",
            200,
            "image/png",
            b"\x89PNG",
            b"",
            &[],
        );
        app.history = history;
        app
    }

    #[test]
    fn site_map_renders_burp_columns_and_captured_rows() {
        let mut app = captured_app();
        app.inspector
            .apply_workspace(&crate::project::InspectorWorkspace {
                notes: vec![crate::project::InspectorNoteWorkspace {
                    id: 1,
                    text: "remember the login flow".to_string(),
                }],
                ..Default::default()
            });
        // Select the host explicitly so the row assertions do not depend on
        // which host sorts first in the tree.
        app.site_map_ui.selected_node = Some(SiteNodeId::Host {
            host: "example.com".to_string(),
        });
        let ctx = test_context();
        let output = target_frame(&ctx, &mut app, Vec::new());
        let text = rendered_text(&output);
        for expected in [
            "Contents",
            "Issues (0)",
            "Site map filter: Hiding not found items",
            "example.com",
            "mixed.test",
            "GET",
            "/products",
            "HTML",
            "Products",
            "Time requested",
            "remember the login flow",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }

        let map = &app.site_map.as_ref().expect("map built").1;
        assert_eq!(map.hosts.len(), 3);
        assert_eq!(map.endpoint_count(), 9);
        assert_eq!(app.selected, Some(1));
        assert!(!app.site_map_ui.visible_ids.is_empty());
    }

    #[test]
    fn site_map_tree_nodes_filter_the_contents_table() {
        let mut app = captured_app();
        app.site_map_ui.filter = SiteMapFilter::SHOW_ALL;
        let ctx = test_context();
        target_frame(&ctx, &mut app, Vec::new());

        assert!(
            click_text(&ctx, &mut app, "products", egui::PointerButton::Primary),
            "products node should be clickable"
        );
        let selected = app
            .site_map_ui
            .selected_node
            .clone()
            .expect("node selected");
        assert_eq!(
            selected,
            SiteNodeId::Endpoint {
                host: "example.com".to_string(),
                path: "/products".to_string(),
            }
        );
        assert_eq!(app.site_map_ui.visible_ids, vec![3, 2]);

        // The Contents search matches displayed URLs, titles, and notes.
        app.site_map_ui.search = "products".to_string();
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("Products"));
        assert_eq!(app.site_map_ui.visible_ids, vec![3, 2]);

        app.site_map_ui.search = "no-such-row".to_string();
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("No contents match the search"));
        assert!(app.site_map_ui.visible_ids.is_empty());
        assert_eq!(app.selected, None);

        // Clearing the node selection also clears the selected exchange and
        // detail cache instead of leaving a stale detail pane behind. Call the
        // Contents view directly because the outer site-map view selects the
        // first tree node whenever the selection is empty.
        app.site_map_ui.search.clear();
        app.site_map_ui.selected_node = None;
        app.selected = Some(1);
        app.detail = None;
        let map = SiteMap::build(&app.history, None);
        let tree = map.tree(SiteMapFilter::SHOW_ALL);
        let mut output = ctx.run_ui(screen(), |ui| {
            app.site_map_contents(ui, &map, &tree, false);
        });
        let text = rendered_text(&output);
        output.textures_delta.clear();
        assert!(text.contains("No node selected"));
        assert!(text.contains("Select a host, folder, or resource"));
        assert_eq!(app.selected, None);
        assert!(app.detail.is_none());
        assert!(app.site_map_ui.visible_ids.is_empty());

        // A node whose rows are all hidden by the filter shows the filter
        // empty state and clears the selection.
        app.site_map_ui.selected_node = Some(SiteNodeId::Endpoint {
            host: "example.com".to_string(),
            path: "/assets/site.css".to_string(),
        });
        app.site_map_ui.filter = SiteMapFilter {
            parameterized_only: true,
            ..SiteMapFilter::SHOW_ALL
        };
        app.selected = Some(4);
        let mut output = ctx.run_ui(screen(), |ui| {
            app.site_map_contents(ui, &map, &tree, false);
        });
        let text = rendered_text(&output);
        output.textures_delta.clear();
        assert!(text.contains("No visible items"));
        assert!(app.site_map_ui.visible_ids.is_empty());
        assert_eq!(app.selected, None);
    }

    #[test]
    fn site_map_toolbar_opens_filter_and_help_windows() {
        let mut app = captured_app();
        let ctx = test_context();
        target_frame(&ctx, &mut app, Vec::new());

        assert!(click_primary(&ctx, &mut app, "Site map help"));
        assert!(app.site_map_ui.help_open);
        assert!(app.site_map_ui.windows_open());
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("Mapping captured traffic"));
        assert!(text.contains("What this view does not do"));

        assert!(click_primary(&ctx, &mut app, "Site map filter settings"));
        assert!(app.site_map_ui.filter_open);
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("Show only parameterized requests"));
        assert!(text.contains("Hide general binary"));

        assert!(click_primary(&ctx, &mut app, "Show all items"));
        assert_eq!(app.site_map_ui.filter, SiteMapFilter::SHOW_ALL);
        assert!(click_primary(&ctx, &mut app, "Reset to defaults"));
        assert_eq!(app.site_map_ui.filter, SiteMapFilter::default());
        assert!(click_primary(&ctx, &mut app, "Close"));
        assert!(!app.site_map_ui.filter_open);

        assert!(click_primary(&ctx, &mut app, "Pause live updates"));
        assert!(app.site_map_ui.paused());
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(!text.is_empty());
        assert!(click_primary(&ctx, &mut app, "Resume live updates"));
        assert!(!app.site_map_ui.paused());

        app.site_map_ui.help_open = false;
        assert!(!app.site_map_ui.windows_open());
    }

    #[test]
    fn site_map_contents_menu_controls_folders_filter_and_search() {
        let mut app = captured_app();
        app.site_map_ui.filter = SiteMapFilter::SHOW_ALL;
        let ctx = test_context();
        target_frame(&ctx, &mut app, Vec::new());

        assert!(click_primary(&ctx, &mut app, "..."));
        assert!(click_primary(&ctx, &mut app, "Expand all folders"));
        let tree = app
            .site_map
            .as_ref()
            .expect("map cached")
            .1
            .tree(SiteMapFilter::SHOW_ALL);
        let mut expected = BTreeMap::new();
        set_all_open(&tree, &mut expected, true);
        for (key, open) in expected {
            assert_eq!(app.site_map_ui.open.get(&key), Some(&open), "{key}");
        }

        assert!(click_primary(&ctx, &mut app, "..."));
        assert!(click_primary(&ctx, &mut app, "Collapse all folders"));
        assert!(app.site_map_ui.open.values().any(|open| !open));

        app.site_map_ui.filter = SiteMapFilter::SHOW_ALL;
        assert!(click_primary(&ctx, &mut app, "..."));
        assert!(click_primary(&ctx, &mut app, "Reset site map filter"));
        assert_eq!(app.site_map_ui.filter, SiteMapFilter::default());

        app.site_map_ui.search = "products".to_string();
        assert!(click_primary(&ctx, &mut app, "..."));
        assert!(click_primary(&ctx, &mut app, "Clear search"));
        assert!(app.site_map_ui.search.is_empty());
    }

    #[test]
    fn site_map_sort_headers_toggle_and_cover_every_column() {
        let mut app = captured_app();
        app.site_map_ui.filter = SiteMapFilter::SHOW_ALL;
        let ctx = test_context();
        target_frame(&ctx, &mut app, Vec::new());

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
            let before = (app.site_map_ui.sort, app.site_map_ui.sort_ascending);
            assert!(
                click_text(&ctx, &mut app, sort.label(), egui::PointerButton::Primary),
                "missing header {}",
                sort.label()
            );
            if before.0 == sort {
                assert_eq!(app.site_map_ui.sort, sort);
                assert_ne!(app.site_map_ui.sort_ascending, before.1);
            } else {
                assert_eq!(app.site_map_ui.sort, sort);
                assert!(app.site_map_ui.sort_ascending);
            }
        }
    }

    #[test]
    fn site_map_row_actions_reach_every_workspace() {
        let mut app = captured_app();
        app.selected = Some(1);

        app.apply_site_map_row_action(SiteMapRowAction::Repeater);
        assert_eq!(app.main_tab, MainTab::Repeater);
        assert_eq!(app.repeater_tabs.len(), 1);

        app.apply_site_map_row_action(SiteMapRowAction::Intruder);
        assert_eq!(app.main_tab, MainTab::Intruder);

        app.apply_site_map_row_action(SiteMapRowAction::ComparerRequest);
        app.apply_site_map_row_action(SiteMapRowAction::ComparerResponse);
        assert_eq!(app.comparer_workspace().len(), 2);

        app.apply_site_map_row_action(SiteMapRowAction::Sequencer);
        assert_eq!(app.main_tab, MainTab::Sequencer);

        // Missing selections are a no-op rather than a panic.
        app.selected = None;
        app.apply_site_map_row_action(SiteMapRowAction::Repeater);
    }

    #[test]
    fn site_map_pause_freezes_the_snapshot_until_resume() {
        let mut app = captured_app();
        let ctx = test_context();
        target_frame(&ctx, &mut app, Vec::new());
        let before = app.site_map.as_ref().expect("map built").1.endpoint_count();

        app.site_map_ui.paused = true;
        capture(
            &mut app.history,
            20,
            "GET",
            "https",
            "example.com",
            "/while-paused",
            200,
            "text/html",
            b"<title>Paused</title>",
            b"",
            &[],
        );
        target_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            app.site_map
                .as_ref()
                .expect("paused map")
                .1
                .endpoint_count(),
            before
        );

        app.site_map_ui.paused = false;
        target_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            app.site_map
                .as_ref()
                .expect("resumed map")
                .1
                .endpoint_count(),
            before + 1
        );
    }

    #[test]
    fn empty_and_filtered_site_maps_render_accurate_empty_states() {
        let mut app = PerseidApp::test_app();
        let ctx = test_context();
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("No HTTP endpoints"));
        assert!(text.contains("Browse an HTTP or HTTPS site"));

        app.scope_enabled = true;
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("No in-scope HTTP endpoints"));

        // Captured traffic that the default filter hides leaves the map
        // populated but the tree empty.
        let mut filtered = PerseidApp::test_app();
        capture(
            &mut filtered.history,
            1,
            "GET",
            "https",
            "example.com",
            "/site.css",
            200,
            "text/css",
            b"body{}",
            b"",
            &[],
        );
        let text = rendered_text(&target_frame(&ctx, &mut filtered, Vec::new()));
        assert!(text.contains("No items match the site map filter"));
        assert!(text.contains("No visible items"));
    }

    #[test]
    fn site_map_issues_tabs_and_target_issues_explain_their_empty_state() {
        let mut app = captured_app();
        app.site_map_ui.tab = SiteMapTab::Issues;
        app.target_section = TargetSection::SiteMap;
        let ctx = test_context();
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("No issues to show"));
        assert!(text.contains("Perseid does not run a vulnerability scanner"));
        assert!(app.site_map_ui.visible_ids.is_empty());

        app.target_section = TargetSection::Issues;
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("No issues to show"));
        assert!(text.contains("Captured requests are still available"));
    }

    #[test]
    fn scope_view_renders_hosts_and_edits_rules() {
        let mut app = captured_app();
        app.target_section = TargetSection::Scope;
        app.scope.insert("example.com".to_string());
        let ctx = test_context();
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("Scope rules"));
        assert!(text.contains("Captured hosts"));
        assert!(text.contains("example.com"));
        assert!(text.contains("x6"));
        assert!(text.contains("in scope"));
        assert!(text.contains("Remove"));
        assert!(click_primary(&ctx, &mut app, "Remove"));
        assert!(app.scope.is_empty());

        // With no captured hosts only the Include host form offers Add, so
        // the click target is unambiguous.
        app.history.clear();
        app.scope_input = "api.example.com".to_string();
        assert!(click_primary(&ctx, &mut app, "Add"));
        assert!(app.scope.contains("api.example.com"));
        assert!(app.scope_input.is_empty());

        app.scope.insert("one.test".to_string());
        app.scope.insert("two.test".to_string());
        assert!(click_primary(&ctx, &mut app, "Clear scope"));
        assert!(app.scope.is_empty());

        assert!(click_primary(&ctx, &mut app, "Filter views to the scope"));
        assert!(app.scope_enabled);
        assert!(click_primary(&ctx, &mut app, "Block out-of-scope traffic"));
        assert!(app.scope_enforce);
    }

    #[test]
    fn scope_view_without_history_reports_no_captured_hosts() {
        let mut app = PerseidApp::test_app();
        app.target_section = TargetSection::Scope;
        let ctx = test_context();
        let text = rendered_text(&target_frame(&ctx, &mut app, Vec::new()));
        assert!(text.contains("No hosts captured yet."));
        assert!(text.contains("No explicit scope rules."));
    }

    #[test]
    fn site_map_helpers_cover_matching_sorting_and_labels() {
        assert_eq!(plural(0), "s");
        assert_eq!(plural(1), "");
        assert!(!site_map_time(SystemTime::UNIX_EPOCH).is_empty());

        let mut app = captured_app();
        app.inspector
            .apply_workspace(&crate::project::InspectorWorkspace {
                notes: vec![crate::project::InspectorNoteWorkspace {
                    id: 2,
                    text: "note text".to_string(),
                }],
                ..Default::default()
            });
        let map = SiteMap::build(&app.history, None);
        let entry2 = map
            .hosts
            .iter()
            .flat_map(|host| &host.entries)
            .find(|entry| entry.id == 2)
            .expect("products query entry");
        let entry3 = map
            .hosts
            .iter()
            .flat_map(|host| &host.entries)
            .find(|entry| entry.id == 3)
            .expect("products form entry");
        assert!(site_row_matches(
            &SiteRowRef {
                entry: entry2,
                item: app.history.get(2),
                notes: "note text",
            },
            "note"
        ));
        for needle in [
            "example.com",
            "get",
            "products",
            "sort=price",
            "products",
            "html",
            "200",
            "parameter",
        ] {
            assert!(
                site_row_matches(
                    &SiteRowRef {
                        entry: entry2,
                        item: app.history.get(2),
                        notes: "",
                    },
                    needle
                ),
                "expected match for {needle}"
            );
        }
        assert!(!site_row_matches(
            &SiteRowRef {
                entry: entry2,
                item: app.history.get(2),
                notes: "",
            },
            "absent"
        ));

        let mut rows = vec![
            SiteRowRef {
                entry: entry2,
                item: app.history.get(2),
                notes: "note text",
            },
            SiteRowRef {
                entry: entry3,
                item: app.history.get(3),
                notes: "",
            },
        ];
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
            sort_site_rows(&mut rows, sort, true);
            sort_site_rows(&mut rows, sort, false);
        }

        let mut open = BTreeMap::new();
        let tree = map.tree(SiteMapFilter::SHOW_ALL);
        set_all_open(&tree, &mut open, true);
        assert!(open.values().all(|open| *open));
        set_all_open(&tree, &mut open, false);
        assert!(open.values().all(|open| !*open));
    }

    #[test]
    fn site_map_mixed_scheme_host_renders_the_combined_icon() {
        let mut app = captured_app();
        app.site_map_ui.filter = SiteMapFilter::SHOW_ALL;
        let ctx = test_context();
        let output = target_frame(&ctx, &mut app, Vec::new());
        let tree = app
            .site_map
            .as_ref()
            .expect("map built")
            .1
            .tree(SiteMapFilter::SHOW_ALL);
        let mixed = tree
            .iter()
            .find(|node| node.label == "mixed.test")
            .expect("mixed host");
        assert_eq!(mixed.scheme, "mixed");
        assert!(!output.shapes.is_empty());
    }
}
