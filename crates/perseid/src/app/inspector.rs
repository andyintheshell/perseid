//! Burp-style Inspector side panel for captured exchanges.
//!
//! The Inspector sits to the right of the message editor and presents the
//! selected request and response as parsed, collapsible name-value sections:
//! attributes, query and body parameters, cookies, and headers. The values are
//! decoded for reading, while the message editor keeps showing the wire-shaped
//! representation.
//!
//! The side panel follows Burp's default layout: it is open on first use, has
//! a fixed vertical tab rail on the right, and remembers the width the user
//! drags it to. Sections start collapsed; the header toggle expands or
//! collapses every section that contains data.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Align2, FontId, RichText, Sense, Stroke, TextEdit};

use crate::history::HistoryItem;
use crate::theme;
use crate::view;

const RAIL_WIDTH: f32 = 24.0;
const RAIL_TAB_MAX_HEIGHT: f32 = 116.0;
const RAIL_TAB_MIN_HEIGHT: f32 = 64.0;
const SECTION_HEIGHT: f32 = 27.0;
const ICON_SIZE: f32 = 18.0;

/// Which tool is showing in the side panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SideTab {
    #[default]
    Inspector,
    Notes,
}

/// Persistent side-panel state.
pub(super) struct InspectorState {
    /// The panel is open by default, matching the reference layout.
    pub open: bool,
    pub tab: SideTab,
    /// Section ids expanded by the user. Sections start collapsed.
    expanded: BTreeSet<&'static str>,
    /// Per-exchange notes, kept in memory for this session.
    notes: BTreeMap<u64, String>,
}

impl Default for InspectorState {
    fn default() -> Self {
        Self {
            open: true,
            tab: SideTab::Inspector,
            expanded: BTreeSet::new(),
            notes: BTreeMap::new(),
        }
    }
}

impl InspectorState {
    #[cfg(feature = "screenshots")]
    pub(super) fn reset(&mut self) {
        self.open = true;
        self.tab = SideTab::Inspector;
        self.expanded.clear();
        self.notes.clear();
    }

    pub(super) fn clear_notes(&mut self) {
        self.notes.clear();
    }

    fn toggle(&mut self, id: &'static str) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id);
        }
    }

    fn expand_all<'a>(&mut self, sections: impl Iterator<Item = &'a InspectorSection>) {
        for section in sections {
            if !section.rows.is_empty() {
                self.expanded.insert(section.id);
            }
        }
    }

    fn collapse_all(&mut self) {
        self.expanded.clear();
    }
}

/// One parsed Inspector section.
pub(super) struct InspectorSection {
    /// Stable across exchanges so expansion state and egui ids survive row
    /// changes.
    pub id: &'static str,
    pub title: &'static str,
    pub rows: Vec<InspectorRow>,
}

impl InspectorSection {
    fn new(id: &'static str, title: &'static str, rows: Vec<InspectorRow>) -> Self {
        Self { id, title, rows }
    }
}

/// One name-value pair inside an Inspector section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InspectorRow {
    pub name: String,
    pub value: String,
}

impl InspectorRow {
    fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Parsed request and response views for the selected exchange.
pub(super) struct InspectorModel {
    pub request: Vec<InspectorSection>,
    pub response: Vec<InspectorSection>,
}

impl InspectorModel {
    pub(super) fn from_history(item: &HistoryItem) -> Self {
        Self {
            request: request_sections(item),
            response: response_sections(item),
        }
    }

    fn sections(&self) -> impl Iterator<Item = &InspectorSection> {
        self.request.iter().chain(self.response.iter())
    }
}

fn request_sections(item: &HistoryItem) -> Vec<InspectorSection> {
    let mut sections = Vec::new();

    sections.push(InspectorSection::new(
        "request.attributes",
        "Request attributes",
        vec![
            InspectorRow::new("Method", item.request.method.clone()),
            InspectorRow::new("Path", item.request.path.clone()),
            InspectorRow::new("Protocol", item.request.version.clone()),
        ],
    ));

    let query = item
        .request
        .path
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    sections.push(InspectorSection::new(
        "request.query",
        "Request query parameters",
        parse_urlencoded(query),
    ));

    if let Some(body) = request_body_parameters(item) {
        sections.push(InspectorSection::new(
            "request.body",
            "Request body parameters",
            body,
        ));
    }

    sections.push(InspectorSection::new(
        "request.cookies",
        "Request cookies",
        cookie_rows(&item.request.headers, "cookie"),
    ));

    let mut headers: Vec<InspectorRow> = item
        .request
        .headers
        .iter()
        .map(|(name, value)| InspectorRow::new(view::display_header_name(name), value.clone()))
        .collect();
    // HTTP/2 carries the authority in a pseudo-header. Show the same synthetic
    // Host row that the message editor uses so the two views agree.
    if item.request.version.starts_with("HTTP/2")
        && !item
            .request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"))
        && !item.request.host.is_empty()
    {
        headers.insert(0, InspectorRow::new("Host", item.request.host.clone()));
    }
    sections.push(InspectorSection::new(
        "request.headers",
        "Request headers",
        headers,
    ));

    sections
}

fn response_sections(item: &HistoryItem) -> Vec<InspectorSection> {
    let Some(response) = &item.response else {
        return Vec::new();
    };

    let mime = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.split(';').next().unwrap_or(value).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".to_string());
    let status = match item.status() {
        Some(status) if !response.reason.is_empty() => {
            format!("{status} {}", response.reason)
        }
        Some(status) => status.to_string(),
        None => "-".to_string(),
    };

    vec![
        InspectorSection::new(
            "response.attributes",
            "Response attributes",
            vec![
                InspectorRow::new("Status code", status),
                InspectorRow::new("Protocol", response.version.clone()),
                InspectorRow::new("MIME type", mime),
                InspectorRow::new(
                    "Length",
                    item.response_body()
                        .map(|body| view::human_bytes(body.total_len()))
                        .unwrap_or_else(|| "-".to_string()),
                ),
            ],
        ),
        InspectorSection::new(
            "response.cookies",
            "Response cookies",
            cookie_rows(&response.headers, "set-cookie"),
        ),
        InspectorSection::new(
            "response.headers",
            "Response headers",
            response
                .headers
                .iter()
                .map(|(name, value)| {
                    InspectorRow::new(view::display_header_name(name), value.clone())
                })
                .collect(),
        ),
    ]
}

fn request_body_parameters(item: &HistoryItem) -> Option<Vec<InspectorRow>> {
    let content_type = item
        .request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())?;
    if !content_type
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("application/x-www-form-urlencoded")
    {
        return None;
    }
    let body = std::str::from_utf8(item.request_body.data()).ok()?;
    Some(parse_urlencoded(truncate_text(
        body,
        view::TEXT_PREVIEW_LIMIT,
    )))
}

/// Truncate at a UTF-8 character boundary so the form parser never sees a
/// split code point.
fn truncate_text(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn parse_urlencoded(input: &str) -> Vec<InspectorRow> {
    form_urlencoded::parse(input.as_bytes())
        .map(|(name, value)| InspectorRow::new(name.into_owned(), value.into_owned()))
        .collect()
}

fn cookie_rows(headers: &[(String, String)], header: &str) -> Vec<InspectorRow> {
    let response_cookie = header.eq_ignore_ascii_case("set-cookie");
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(header))
        .flat_map(|(_, value)| {
            // A Set-Cookie header starts with one cookie and then carries
            // attributes such as Path and HttpOnly. A request Cookie header
            // carries several `name=value` pairs instead.
            let take = if response_cookie { 1 } else { usize::MAX };
            value.split(';').take(take)
        })
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(InspectorRow::new(name, decode_encoded_value(value.trim())))
        })
        .collect()
}

/// Decode percent-escaped values for display. `+` is left alone because it is
/// literal in cookies and header values; query and form parsing uses the
/// `application/x-www-form-urlencoded` rules through `form_urlencoded`.
fn decode_encoded_value(value: &str) -> String {
    if !value.contains('%') {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Draw the side-panel rail and, when open, its Inspector or Notes content.
///
/// Call this before laying out the request/response panes. The rail is attached
/// first so it stays on the window's outer edge; the content panel then sits
/// immediately to its left.
pub(super) fn side_panel(
    ui: &mut egui::Ui,
    state: &mut InspectorState,
    selected_id: u64,
    model: &InspectorModel,
) {
    egui::Panel::right("perseid.inspector.rail")
        .exact_size(RAIL_WIDTH)
        .frame(egui::Frame::NONE.fill(theme::SURFACE_ALT))
        .show(ui, |ui| rail(ui, state));

    if !state.open {
        return;
    }

    let default_width = (ui.available_width() * 0.20).clamp(220.0, 360.0);
    egui::Panel::right("perseid.inspector.panel")
        .resizable(true)
        .default_size(default_width)
        .min_size(180.0)
        .max_size((ui.available_width() * 0.45).max(240.0))
        .frame(
            egui::Frame::NONE
                .fill(theme::SURFACE)
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ui, |ui| match state.tab {
            SideTab::Inspector => inspector_content(ui, state, model),
            SideTab::Notes => notes_content(ui, state, selected_id),
        });
}

fn rail(ui: &mut egui::Ui, state: &mut InspectorState) {
    let tab_height =
        ((ui.available_height() - 8.0) * 0.5).clamp(RAIL_TAB_MIN_HEIGHT, RAIL_TAB_MAX_HEIGHT);
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.add_space(4.0);

    if rail_tab(
        ui,
        "Inspector",
        state.open && state.tab == SideTab::Inspector,
        tab_height,
    )
    .clicked()
    {
        if state.tab == SideTab::Inspector {
            state.open = !state.open;
        } else {
            state.tab = SideTab::Inspector;
            state.open = true;
        }
    }
    if rail_tab(
        ui,
        "Notes",
        state.open && state.tab == SideTab::Notes,
        tab_height,
    )
    .clicked()
    {
        state.tab = SideTab::Notes;
        state.open = true;
    }
}

/// A narrow tab with its label rotated to run top-to-bottom, like Burp's side
/// panel rail.
fn rail_tab(ui: &mut egui::Ui, label: &str, selected: bool, height: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(RAIL_WIDTH, height), Sense::click());
    let fill = if selected {
        theme::ACCENT_WASH
    } else if response.hovered() {
        theme::SURFACE_TINT
    } else {
        theme::SURFACE_ALT
    };
    ui.painter().rect_filled(rect, 0.0, fill);

    // A short accent edge on the content-facing side marks the open tab.
    if selected {
        let edge = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + 4.0),
            egui::pos2(rect.left() + 2.0, rect.bottom() - 4.0),
        );
        ui.painter().rect_filled(edge, 0.0, theme::ACCENT);
    }

    let color = if selected {
        theme::ACCENT_DARK
    } else {
        theme::TEXT_MUTED
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(11.5), color);
    let top_left = rect.center() - galley.size() * 0.5;
    let shape = egui::epaint::TextShape::new(top_left, galley, color)
        .with_angle_and_anchor(std::f32::consts::FRAC_PI_2, Align2::CENTER_CENTER);
    ui.painter().add(shape);

    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    response
        .on_hover_text(if selected && label == "Inspector" {
            "Collapse the Inspector"
        } else {
            label
        })
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn inspector_content(ui: &mut egui::Ui, state: &mut InspectorState, model: &InspectorModel) {
    let all_expanded = model
        .sections()
        .all(|section| section.rows.is_empty() || state.expanded.contains(section.id));
    match header(ui, "Inspector", Some(all_expanded)) {
        HeaderAction::ToggleAll => {
            if all_expanded {
                state.collapse_all();
            } else {
                state.expand_all(model.sections());
            }
        }
        HeaderAction::Close => state.open = false,
        HeaderAction::None => {}
    }

    egui::ScrollArea::vertical()
        .id_salt("perseid.inspector.scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(ui.available_width());
            for (index, section) in model.sections().enumerate() {
                section_ui(ui, state, section, index);
            }
            ui.add_space(6.0);
        });
}

fn notes_content(ui: &mut egui::Ui, state: &mut InspectorState, selected_id: u64) {
    if header(ui, "Notes", None) == HeaderAction::Close {
        state.open = false;
    }
    let note = state.notes.entry(selected_id).or_default();
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.add(
                TextEdit::multiline(note)
                    .desired_width(f32::INFINITY)
                    .desired_rows(10)
                    .hint_text("Notes for this exchange"),
            );
        });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderAction {
    None,
    ToggleAll,
    Close,
}

fn header(ui: &mut egui::Ui, title: &str, all_expanded: Option<bool>) -> HeaderAction {
    let mut action = HeaderAction::None;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(13.0).strong().color(theme::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "Close the side panel", HeaderIcon::Close).clicked() {
                        action = HeaderAction::Close;
                    }
                    if let Some(all_expanded) = all_expanded {
                        let (icon, tooltip) = if all_expanded {
                            (HeaderIcon::CollapseAll, "Collapse all sections")
                        } else {
                            (HeaderIcon::ExpandAll, "Expand all sections")
                        };
                        if icon_button(ui, tooltip, icon).clicked() {
                            action = HeaderAction::ToggleAll;
                        }
                    }
                });
            });
        });
    ui.separator();
    action
}

#[derive(Clone, Copy)]
enum HeaderIcon {
    ExpandAll,
    CollapseAll,
    Close,
}

fn icon_button(ui: &mut egui::Ui, tooltip: &str, icon: HeaderIcon) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ICON_SIZE, ICON_SIZE), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 3.0, theme::ACCENT_WASH);
    }
    let center = rect.center();
    let stroke = Stroke::new(1.2, theme::TEXT_MUTED);
    match icon {
        HeaderIcon::Close => {
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 3.5, center.y - 3.5),
                    egui::pos2(center.x + 3.5, center.y + 3.5),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 3.5, center.y + 3.5),
                    egui::pos2(center.x + 3.5, center.y - 3.5),
                ],
                stroke,
            );
        }
        HeaderIcon::ExpandAll | HeaderIcon::CollapseAll => {
            let outward = matches!(icon, HeaderIcon::ExpandAll);
            let top_y = center.y - 4.0;
            let bottom_y = center.y + 4.0;
            let top_tip_y = if outward { top_y - 2.5 } else { top_y + 2.5 };
            let bottom_tip_y = if outward {
                bottom_y + 2.5
            } else {
                bottom_y - 2.5
            };
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 3.5, top_y),
                    egui::pos2(center.x, top_tip_y),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(center.x, top_tip_y),
                    egui::pos2(center.x + 3.5, top_y),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(center.x - 3.5, bottom_y),
                    egui::pos2(center.x, bottom_tip_y),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(center.x, bottom_tip_y),
                    egui::pos2(center.x + 3.5, bottom_y),
                ],
                stroke,
            );
        }
    }
    response
        .on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn section_ui(
    ui: &mut egui::Ui,
    state: &mut InspectorState,
    section: &InspectorSection,
    index: usize,
) {
    let expanded = state.expanded.contains(section.id);
    let fill = if index.is_multiple_of(2) {
        theme::SURFACE
    } else {
        theme::SURFACE_TINT
    };

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SECTION_HEIGHT),
        Sense::click(),
    );
    let hovered_fill = if response.hovered() {
        theme::ACCENT_WASH
    } else {
        fill
    };
    ui.painter().rect_filled(rect, 0.0, hovered_fill);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, theme::BORDER),
    );

    let text_left = rect.left() + 9.0;
    let chevron_x = rect.right() - 12.0;
    let count = section.rows.len();
    let count_text = count.to_string();
    let count_font = FontId::proportional(10.5);
    let count_galley =
        ui.painter()
            .layout_no_wrap(count_text.clone(), count_font, theme::TEXT_MUTED);
    let pill_width = (count_galley.size().x + 12.0).max(20.0);
    let pill = egui::Rect::from_center_size(
        egui::pos2(chevron_x - 13.0 - pill_width * 0.5, rect.center().y),
        egui::vec2(pill_width, 16.0),
    );
    ui.painter().rect_filled(pill, 8.0, theme::SURFACE_ALT);
    ui.painter().galley(
        egui::pos2(
            pill.center().x - count_galley.size().x * 0.5,
            pill.center().y - count_galley.size().y * 0.5,
        ),
        count_galley,
        theme::TEXT_MUTED,
    );

    let title_galley = ui.painter().layout_no_wrap(
        section.title.to_owned(),
        FontId::proportional(11.5),
        theme::TEXT,
    );
    ui.painter().galley(
        egui::pos2(text_left, rect.center().y - title_galley.size().y * 0.5),
        title_galley,
        theme::TEXT,
    );

    let stroke = Stroke::new(1.3, theme::TEXT_MUTED);
    let chevron_y = rect.center().y;
    let tip = if expanded { -1.8 } else { 1.8 };
    ui.painter().line_segment(
        [
            egui::pos2(chevron_x - 3.0, chevron_y - tip),
            egui::pos2(chevron_x, chevron_y + tip),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(chevron_x, chevron_y + tip),
            egui::pos2(chevron_x + 3.0, chevron_y - tip),
        ],
        stroke,
    );

    if response.clicked() {
        state.toggle(section.id);
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, expanded, section.title)
        });

    if expanded {
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin {
                left: 12,
                right: 8,
                top: 5,
                bottom: 7,
            })
            .show(ui, |ui| {
                if section.rows.is_empty() {
                    ui.label(RichText::new("No items").small().color(theme::TEXT_MUTED));
                } else {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for row in &section.rows {
                        row_ui(ui, row);
                    }
                }
            });
    }
}

fn row_ui(ui: &mut egui::Ui, row: &InspectorRow) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let name_width = (ui.available_width() * 0.40).clamp(58.0, 150.0);
        ui.add_sized(
            [name_width, 16.0],
            egui::Label::new(RichText::new(&row.name).small().color(theme::TEXT_MUTED)).truncate(),
        )
        .on_hover_text(&row.name);
        ui.add(egui::Label::new(RichText::new(&row.value).small().color(theme::TEXT)).wrap())
            .on_hover_text(&row.value);
    });
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::history::Phase;
    use crate::proxy::{CapturedBody, ExchangeResult, RequestInfo, ResponseHead};

    use super::*;

    fn item(request_body: Vec<u8>, content_type: &str) -> HistoryItem {
        HistoryItem {
            id: 1,
            number: 1,
            timestamp: SystemTime::UNIX_EPOCH,
            request: RequestInfo {
                method: "POST".to_string(),
                uri: "http://example.com/search?q=hello+world&tag=a%2Fb".to_string(),
                scheme: "http".to_string(),
                host: "example.com".to_string(),
                path: "/search?q=hello+world&tag=a%2Fb".to_string(),
                version: "HTTP/1.1".to_string(),
                headers: vec![
                    ("host".to_string(), "example.com".to_string()),
                    ("content-type".to_string(), content_type.to_string()),
                    (
                        "cookie".to_string(),
                        "session=abc%2F123; theme=dark+blue".to_string(),
                    ),
                    ("x-custom-header".to_string(), "demo".to_string()),
                ],
            },
            request_body: CapturedBody::from_bytes(request_body),
            response: Some(ResponseHead {
                version: "HTTP/1.1".to_string(),
                status: 200,
                reason: "OK".to_string(),
                headers: vec![
                    (
                        "content-type".to_string(),
                        "text/html; charset=utf-8".to_string(),
                    ),
                    (
                        "set-cookie".to_string(),
                        "auth=token; Path=/; HttpOnly".to_string(),
                    ),
                ],
            }),
            phase: Phase::Finished {
                duration: Duration::from_millis(1),
                result: ExchangeResult::Response {
                    body: CapturedBody::from_bytes(b"<html>ok</html>".to_vec()),
                },
            },
        }
    }

    fn section<'a>(sections: &'a [InspectorSection], id: &str) -> &'a InspectorSection {
        sections
            .iter()
            .find(|section| section.id == id)
            .expect("section should be present")
    }

    #[test]
    fn parses_request_sections_and_decodes_values() {
        let item = item(
            b"name=Jane+Doe&role=admin%2Fuser".to_vec(),
            "application/x-www-form-urlencoded",
        );
        let model = InspectorModel::from_history(&item);

        let attributes = section(&model.request, "request.attributes");
        assert_eq!(attributes.rows.len(), 3);
        assert_eq!(attributes.rows[0], InspectorRow::new("Method", "POST"));
        assert_eq!(
            attributes.rows[2],
            InspectorRow::new("Protocol", "HTTP/1.1")
        );

        let query = section(&model.request, "request.query");
        assert_eq!(
            query.rows,
            vec![
                InspectorRow::new("q", "hello world"),
                InspectorRow::new("tag", "a/b"),
            ]
        );

        let body = section(&model.request, "request.body");
        assert_eq!(
            body.rows,
            vec![
                InspectorRow::new("name", "Jane Doe"),
                InspectorRow::new("role", "admin/user"),
            ]
        );

        let cookies = section(&model.request, "request.cookies");
        assert_eq!(
            cookies.rows,
            vec![
                InspectorRow::new("session", "abc/123"),
                InspectorRow::new("theme", "dark+blue"),
            ]
        );

        let headers = section(&model.request, "request.headers");
        assert_eq!(headers.rows.len(), 4);
        assert!(headers.rows.iter().any(|row| row.name == "Content-Type"));
    }

    #[test]
    fn skips_body_parameters_for_non_form_bodies() {
        let item = item(br#"{"name":"Jane"}"#.to_vec(), "application/json");
        let model = InspectorModel::from_history(&item);
        assert!(
            !model
                .request
                .iter()
                .any(|section| section.id == "request.body")
        );
    }

    #[test]
    fn parses_response_sections() {
        let item = item(Vec::new(), "application/json");
        let model = InspectorModel::from_history(&item);

        let attributes = section(&model.response, "response.attributes");
        assert_eq!(
            attributes.rows[0],
            InspectorRow::new("Status code", "200 OK")
        );
        assert_eq!(
            attributes.rows[2],
            InspectorRow::new("MIME type", "text/html")
        );

        let cookies = section(&model.response, "response.cookies");
        assert_eq!(cookies.rows, vec![InspectorRow::new("auth", "token")]);
    }

    #[test]
    fn decodes_percent_escapes_without_treating_plus_as_space() {
        assert_eq!(decode_encoded_value("a+b%2Fc"), "a+b/c");
        assert_eq!(decode_encoded_value("%zz"), "%zz");
        assert_eq!(decode_encoded_value("plain"), "plain");
    }

    #[test]
    fn truncate_text_stops_at_a_character_boundary() {
        assert_eq!(truncate_text("aé", 2), "a");
        assert_eq!(truncate_text("aé", 3), "aé");
        assert_eq!(truncate_text("abc", 10), "abc");
    }

    #[test]
    fn inspector_is_open_and_collapsed_by_default() {
        let state = InspectorState::default();
        assert!(state.open);
        assert_eq!(state.tab, SideTab::Inspector);
        assert!(state.expanded.is_empty());
    }

    #[test]
    fn expand_all_skips_empty_sections_and_collapse_all_clears() {
        let item = item(Vec::new(), "application/json");
        let model = InspectorModel::from_history(&item);
        let mut state = InspectorState::default();

        state.expand_all(model.sections());
        assert!(state.expanded.contains("request.headers"));
        assert!(state.expanded.contains("response.headers"));
        assert!(!state.expanded.contains("request.body"));
        state.collapse_all();
        assert!(state.expanded.is_empty());
    }
}
