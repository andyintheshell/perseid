//! Floating windows: live memory usage and the About box.

use egui::Label;

use super::*;
use crate::memory;

impl WiremothApp {
    pub(super) fn memory_indicator(&mut self, ui: &mut egui::Ui) {
        let label = format!("Memory {}", view::human_bytes(memory::heap_in_use()));
        let mut text = RichText::new(label).small().color(if self.memory_open {
            theme::ACCENT_DARK
        } else {
            theme::TEXT_MUTED
        });
        if self.memory_open {
            text = text.underline();
        }
        let response = ui
            .add(Label::new(text).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            self.memory_open = !self.memory_open;
        }
    }

    pub(super) fn memory_window(&mut self, ctx: &egui::Context) {
        if !self.memory_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.memory_open = false;
            return;
        }

        let current = memory::heap_in_use();
        let peak = memory::heap_peak();
        let samples: Vec<u64> = self.memory.samples().iter().copied().collect();
        let mut open = self.memory_open;

        egui::Window::new("Memory usage")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(470.0)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -44.0))
            .show(ctx, |ui| {
                ui.set_min_width(440.0);
                ui.horizontal(|ui| {
                    memory_metric(ui, "Current", current, theme::ACCENT_DARK);
                    ui.add_space(24.0);
                    memory_metric(ui, "Peak", peak, theme::TEXT);
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Memory").small().color(theme::TEXT_MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let window = if samples.len() > 1 {
                            format!("last {}s", samples.len() - 1)
                        } else {
                            "collecting...".to_owned()
                        };
                        ui.label(RichText::new(window).small().color(theme::TEXT_MUTED));
                    });
                });
                memory_graph(ui, &samples);
            });
        self.memory_open = open;
    }

    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.about_open = false;
            return;
        }
        if self.lockup.is_none() {
            self.lockup = branding::lockup_texture(ctx);
        }
        let mut open = self.about_open;
        let lockup = self.lockup.clone();
        egui::Window::new("About Wiremoth")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                if let Some(lockup) = &lockup {
                    ui.add(
                        egui::Image::from_texture(lockup)
                            .max_width(420.0)
                            .corner_radius(10.0),
                    );
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Wiremoth {}", env!("CARGO_PKG_VERSION")))
                        .strong()
                        .color(theme::TEXT),
                );
                ui.hyperlink_to(
                    RichText::new("github.com/andyintheshell/wiremoth").color(theme::ACCENT_DARK),
                    "https://github.com/andyintheshell/wiremoth",
                );
                ui.add_space(2.0);
                ui.weak("A lightweight, keyboard-first workbench for inspecting HTTP traffic.");
                ui.weak("MIT OR Apache-2.0");
            });
        self.about_open = open;
    }
}

fn memory_metric(ui: &mut egui::Ui, label: &str, value: u64, color: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).small().color(theme::TEXT_MUTED));
        ui.label(
            RichText::new(view::human_bytes(value))
                .size(17.0)
                .strong()
                .color(color),
        );
    });
}

fn memory_graph(ui: &mut egui::Ui, samples: &[u64]) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 132.0),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(6), theme::SURFACE_TINT);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    let chart = rect.shrink2(egui::vec2(10.0, 12.0));
    for row in 0..=4 {
        let y = chart.top() + chart.height() * row as f32 / 4.0;
        painter.line_segment(
            [egui::pos2(chart.left(), y), egui::pos2(chart.right(), y)],
            Stroke::new(1.0, theme::BORDER),
        );
    }

    let Some(max) = samples.iter().copied().max().filter(|max| *max > 0) else {
        return;
    };
    let ceiling = (max as f64 * 1.1).max(1.0);
    let denominator = samples.len().saturating_sub(1).max(1) as f32;
    let points: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = if samples.len() == 1 {
                chart.center().x
            } else {
                chart.left() + chart.width() * index as f32 / denominator
            };
            let y = chart.bottom() - chart.height() * (*value as f64 / ceiling).min(1.0) as f32;
            egui::pos2(x, y)
        })
        .collect();

    painter.add(egui::Shape::line(
        points.clone(),
        Stroke::new(1.5, theme::ACCENT),
    ));
    if let Some(last) = points.last() {
        painter.circle_filled(*last, 3.0, theme::ACCENT);
    }
}
