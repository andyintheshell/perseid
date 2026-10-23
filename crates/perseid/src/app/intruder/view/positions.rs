//! The positions bar: add, clear, and auto-mark payload positions.

use egui::{Color32, Label, RichText};

use crate::proxy::{auto_mark, clear_markers, marker_ranges, toggle_marker};
use crate::theme;

use super::super::super::PerseidApp;
use super::INTRUDER_POSITIONS_ID;

impl PerseidApp {
    pub(super) fn intruder_positions(&mut self, ui: &mut egui::Ui) {
        let mut add_marker = false;
        let mut clear_all = false;
        let mut auto = false;

        egui::Panel::top(INTRUDER_POSITIONS_ID)
            .frame(
                egui::Frame::NONE
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                let state = &mut self.intruder;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Positions")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    if ui
                        .button("Add $$")
                        .on_hover_text(
                            "Wrap the selected text in $$ markers. With no selection, insert an empty position at the cursor. Selecting an existing position removes its markers.",
                        )
                        .clicked()
                    {
                        add_marker = true;
                    }
                    if ui
                        .button("Clear $$")
                        .on_hover_text("Remove every position marker from the request")
                        .clicked()
                    {
                        clear_all = true;
                    }
                    if ui
                        .button("Auto $$")
                        .on_hover_text(
                            "Add positions around query values, form values, and cookie values",
                        )
                        .clicked()
                    {
                        auto = true;
                    }

                    ui.separator();
                    let status = position_status(&state.request);
                    ui.label(
                        RichText::new(status.label())
                            .small()
                            .color(status.color()),
                    );
                    if let Some(selection) = &state.selection {
                        ui.label(
                            RichText::new(format!(
                                "selection: {} B",
                                selection.end.saturating_sub(selection.start)
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    }
                });

                if let Some(error) = &state.error {
                    ui.add(
                        Label::new(RichText::new(error).small().color(theme::RED)).truncate(),
                    )
                    .on_hover_text(error);
                } else if let Some(notice) = &state.notice {
                    ui.add(
                        Label::new(RichText::new(notice).small().color(theme::AMBER)).truncate(),
                    )
                    .on_hover_text(notice);
                }
            });

        if add_marker {
            let target = self.intruder.target_range();
            let (request, range) = toggle_marker(&self.intruder.request, target);
            self.intruder.request = request;
            self.intruder.error = None;
            self.intruder.notice = None;
            self.intruder.select_request(ui.ctx(), range);
        }
        if clear_all {
            self.intruder.request = clear_markers(&self.intruder.request);
            self.intruder.error = None;
            self.intruder.notice = Some("Removed every position marker".to_string());
            self.intruder.selection = None;
            self.intruder.cursor = self.intruder.cursor.min(self.intruder.request.len());
        }
        if auto {
            let (request, count) = auto_mark(&self.intruder.request);
            self.intruder.request = request;
            self.intruder.error = None;
            self.intruder.notice = Some(if count == 0 {
                "No new values to mark".to_string()
            } else {
                format!(
                    "Added {count} payload position{}",
                    if count == 1 { "" } else { "s" }
                )
            });
            self.intruder.selection = None;
            self.intruder.cursor = self.intruder.cursor.min(self.intruder.request.len());
        }
    }
}

/// What the positions bar should report for the current request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PositionStatus {
    None,
    Count(usize),
    Unclosed,
}

fn position_status(request: &str) -> PositionStatus {
    match marker_ranges(request) {
        Ok(ranges) if ranges.is_empty() => PositionStatus::None,
        Ok(ranges) => PositionStatus::Count(ranges.len()),
        Err(_) => PositionStatus::Unclosed,
    }
}

impl PositionStatus {
    fn label(self) -> String {
        match self {
            Self::None => "no payload positions".to_string(),
            Self::Count(1) => "1 payload position".to_string(),
            Self::Count(count) => format!("{count} payload positions"),
            Self::Unclosed => "unclosed $$ marker".to_string(),
        }
    }

    fn color(self) -> Color32 {
        match self {
            Self::None => theme::TEXT_MUTED,
            Self::Count(_) => theme::ACCENT_DARK,
            Self::Unclosed => theme::RED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_status_reports_none_counts_and_unclosed_markers() {
        assert_eq!(position_status("GET / HTTP/1.1\n\n"), PositionStatus::None);
        assert_eq!(
            position_status("GET /?a=$$1$$&b=$$2$$ HTTP/1.1\n\n"),
            PositionStatus::Count(2)
        );
        assert_eq!(
            position_status("GET /?a=$$oops HTTP/1.1\n\n"),
            PositionStatus::Unclosed
        );

        assert_eq!(PositionStatus::None.label(), "no payload positions");
        assert_eq!(PositionStatus::Count(1).label(), "1 payload position");
        assert_eq!(PositionStatus::Count(3).label(), "3 payload positions");
        assert_eq!(PositionStatus::Unclosed.label(), "unclosed $$ marker");
        assert_eq!(PositionStatus::None.color(), theme::TEXT_MUTED);
        assert_eq!(PositionStatus::Count(1).color(), theme::ACCENT_DARK);
        assert_eq!(PositionStatus::Unclosed.color(), theme::RED);
    }
}
