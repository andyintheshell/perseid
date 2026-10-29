//! Floating windows: live memory usage, the About box, and first-run
//! certificate setup.

use egui::Label;

use super::*;
use crate::memory;

impl PerseidApp {
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

        let window_response = egui::Window::new("Memory usage")
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
        #[cfg(feature = "screenshots")]
        if let (Some(driver), Some(window)) = (self.screenshots.as_mut(), window_response) {
            driver.record_memory_rect(window.response.rect);
        }
        #[cfg(not(feature = "screenshots"))]
        let _ = window_response;
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
        egui::Window::new("About Perseid")
            .id(egui::Id::unique("perseid.about"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
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
                    RichText::new(format!("Perseid {}", env!("CARGO_PKG_VERSION")))
                        .strong()
                        .color(theme::TEXT),
                );
                ui.hyperlink_to(
                    RichText::new(
                        branding::REPOSITORY_URL.trim_start_matches("https://"),
                    )
                    .color(theme::ACCENT_DARK),
                    branding::REPOSITORY_URL,
                );
                ui.add_space(2.0);
                ui.weak("A lightweight web security workbench. Inspect, modify, and replay HTTP traffic.");
                ui.weak("MIT OR Apache-2.0");
            });
        self.about_open = open;
    }

    pub(super) fn tls_setup_window(&mut self, ctx: &egui::Context) {
        if !self.tls_setup_open || self.proxy.is_none() {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.tls_setup_open = false;
            return;
        }

        let portal_url = self.certificate_page_url();
        let cert_path = ca_store::cert_path(&self.ca_dir);
        let mut open = self.tls_setup_open;
        let mut open_page = false;
        let mut dismiss = false;
        egui::Window::new("Trust Perseid's HTTPS certificate")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_max_width(500.0);
                ui.label(
                    RichText::new("HTTPS interception is ready")
                        .size(17.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.label(
                    "Perseid created a local certificate authority on first launch. To inspect \
                     HTTPS, install its certificate in the browser you use for testing. Trust is \
                     never installed automatically.",
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Certificate")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(cert_path.display().to_string())
                        .monospace()
                        .small()
                        .color(theme::TEXT),
                );
                ui.add_space(8.0);
                if let Some(url) = &portal_url {
                    ui.label(
                        "The proxy serves a certificate download page. Open it, download the \
                         certificate, and import it:",
                    );
                    ui.add_space(4.0);
                    if ui.button("Open certificate page").clicked() {
                        open_page = true;
                    }
                    ui.hyperlink_to(
                        RichText::new(url)
                            .monospace()
                            .small()
                            .color(theme::ACCENT_DARK),
                        url,
                    );
                } else {
                    ui.label(
                        "Start the proxy, then open Proxy > TLS to use the certificate page or \
                         import the file directly.",
                    );
                }
                ui.add_space(8.0);
                ui.label(
                    "Then point the browser at the Perseid proxy. Decrypted requests appear in \
                     HTTP history, and you can turn interception off at any time under Proxy > \
                     TLS.",
                );
                ui.add_space(10.0);
                if ui.button("Not now").clicked() {
                    dismiss = true;
                }
            });
        if dismiss {
            open = false;
        }
        if open_page && let Some(url) = portal_url {
            ctx.open_url(egui::OpenUrl::new_tab(url));
            open = false;
        }
        self.tls_setup_open = open;
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

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Shape, Vec2};

    use crate::memory::{MemoryMeter, SAMPLE_INTERVAL};

    use super::*;

    #[test]
    fn about_window_starts_centered_and_can_be_dragged() {
        let ctx = egui::Context::default();
        let mut app = PerseidApp::test_app();
        app.about_open = true;
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 900.0));
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let run = |input: egui::RawInput, app: &mut PerseidApp| {
            let mut output = ctx.run_ui(input, |ui| app.about_window(ui.ctx()));
            output.textures_delta.clear();
        };
        for _ in 0..3 {
            run(input.clone(), &mut app);
        }
        let rect = || {
            ctx.memory(|memory| memory.area_rect(egui::Id::unique("perseid.about")))
                .unwrap()
        };
        let initial = rect();
        assert!(
            (initial.center() - screen.center()).length() < 2.0,
            "{initial:?}"
        );

        let start = initial.left_top() + Vec2::new(100.0, 10.0);
        let end = start + Vec2::new(100.0, 80.0);
        input.events = vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        run(input.clone(), &mut app);
        input.events = vec![egui::Event::PointerMoved(end)];
        run(input.clone(), &mut app);
        input.events = vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }];
        run(input.clone(), &mut app);
        input.events.clear();
        run(input, &mut app);
        let moved = rect();
        assert!(
            (moved.min - initial.min - (end - start)).length() < 2.0,
            "{initial:?} -> {moved:?}"
        );
    }

    /// Run `memory_graph` in a headless context and return the painted shapes.
    fn graph_shapes(samples: &[u64]) -> Vec<Shape> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(520.0, 320.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| memory_graph(ui, samples));
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .collect()
    }

    fn data_points(shapes: &[Shape], expected: usize) -> Vec<Pos2> {
        shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Path(path) if path.points.len() == expected => Some(path.points.clone()),
                _ => None,
            })
            .expect("graph should draw a data path")
    }

    fn chart_rect(shapes: &[Shape]) -> Rect {
        shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Rect(rect) => Some(rect.rect),
                _ => None,
            })
            .expect("graph should draw a background frame")
            .shrink2(Vec2::new(10.0, 12.0))
    }

    #[test]
    fn memory_graph_draws_one_point_per_sample_with_a_marker_on_the_last() {
        let samples = [1024, 2048, 4096, 3072];
        let shapes = graph_shapes(&samples);
        let points = data_points(&shapes, samples.len());
        assert_eq!(points.len(), 4);
        let last = points.last().unwrap();
        assert!(shapes.iter().any(|shape| matches!(
            shape,
            Shape::Circle(circle) if (circle.center - *last).length() < 0.01
        )));
    }

    #[test]
    fn memory_graph_spreads_points_left_to_right_across_the_chart() {
        let samples = [100, 200, 300, 400, 500];
        let shapes = graph_shapes(&samples);
        let chart = chart_rect(&shapes);
        let points = data_points(&shapes, samples.len());
        assert!((points[0].x - chart.left()).abs() < 0.01);
        assert!((points[2].x - chart.center().x).abs() < 0.01);
        assert!((points[4].x - chart.right()).abs() < 0.01);
        assert!(points.windows(2).all(|pair| pair[0].x < pair[1].x));
    }

    #[test]
    fn memory_graph_places_larger_samples_higher_and_keeps_headroom() {
        let samples = [0, 500, 1000];
        let shapes = graph_shapes(&samples);
        let chart = chart_rect(&shapes);
        let points = data_points(&shapes, samples.len());
        assert!((points[0].y - chart.bottom()).abs() < 0.01);
        assert!(points[1].y < points[0].y);
        assert!(points[2].y < points[1].y);
        // The 10% ceiling headroom keeps the peak below the top edge.
        let headroom = points[2].y - chart.top();
        assert!((headroom - chart.height() * (1.0 - 1.0 / 1.1)).abs() < 0.5);
    }

    #[test]
    fn memory_graph_centers_a_lone_sample() {
        let shapes = graph_shapes(&[777]);
        let chart = chart_rect(&shapes);
        let points = data_points(&shapes, 1);
        assert!((points[0].x - chart.center().x).abs() < 0.01);
        assert!(chart.contains(points[0]));
    }

    #[test]
    fn memory_graph_draws_only_the_frame_when_there_is_nothing_to_plot() {
        for samples in [&[][..], &[0, 0, 0]] {
            let shapes = graph_shapes(samples);
            assert!(shapes.iter().any(|shape| matches!(shape, Shape::Rect(_))));
            assert!(
                !shapes
                    .iter()
                    .any(|shape| matches!(shape, Shape::Path(_) | Shape::Circle(_)))
            );
        }
    }

    #[test]
    fn memory_meter_starts_with_a_single_sample() {
        let meter = MemoryMeter::new();
        assert_eq!(meter.samples().len(), 1);
        assert!(meter.time_until_next_sample() <= SAMPLE_INTERVAL);
    }
}
