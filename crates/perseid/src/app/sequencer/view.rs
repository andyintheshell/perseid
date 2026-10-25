//! Sequencer rendering: the three capture pages, token tables, and result
//! window.
//!
//! The layout follows Burp's manual-load and result screens while using
//! Perseid's shared section tabs, tables, and colour palette. The result
//! window is deliberately self-contained: it takes a finished
//! [`crate::sequencer::Analysis`] and never re-runs tests while drawing.

use std::f32::consts::FRAC_PI_2;

use egui::{
    Align, Align2, Button, Color32, FontId, Key, Label, Layout, Margin, RichText, ScrollArea,
    Sense, Stroke, TextEdit, Vec2,
};
use egui_extras::{Column, TableBuilder};

use super::SequencerState;
use super::state::{
    CapturePart, Feedback, LoadDialog, ResultTab, SequencerSection, read_token_file,
};
use crate::app::widgets;
use crate::history::History;
use crate::sequencer::{
    self, Analysis, AnalysisSettings, BitAnalysis, CharacterAnalysis, CharacterResult,
    EntropyPoint, Significance, TestResult, TokenFormat, format_bits, format_p_value, token_length,
};
use crate::theme;

const TOKEN_ROW_HEIGHT: f32 = 19.0;
const RESULT_WINDOW_ID: &str = "perseid.sequencer.result";
const LOAD_WINDOW_ID: &str = "perseid.sequencer.load";
const RESULT_WINDOW_WIDTH: f32 = 1160.0;
const RESULT_WINDOW_HEIGHT: f32 = 660.0;

impl SequencerState {
    pub(super) fn ui(&mut self, ui: &mut egui::Ui, history: &History) {
        widgets::section_bar(ui, "perseid.sequencer.sections", |ui| {
            if widgets::section_tab(
                ui,
                self.section == SequencerSection::LiveCapture,
                "Live capture",
            )
            .clicked()
            {
                self.section = SequencerSection::LiveCapture;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
            if widgets::section_tab(
                ui,
                self.section == SequencerSection::ManualLoad,
                "Manual load",
            )
            .clicked()
            {
                self.section = SequencerSection::ManualLoad;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
            if widgets::section_tab_gear(
                ui,
                self.section == SequencerSection::Settings,
                "Sequencer settings",
            )
            .clicked()
            {
                self.section = SequencerSection::Settings;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
        });

        widgets::page_pad(ui, |ui| match self.section {
            SequencerSection::LiveCapture => self.live_capture_page(ui, history),
            SequencerSection::ManualLoad => self.manual_load_page(ui),
            SequencerSection::Settings => self.settings_page(ui),
        });
    }

    fn manual_load_page(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Manual load",
            "Load a sample of tokens that you have already obtained, then perform a statistical analysis on the sample. One token per line; comma-separated values are also accepted.",
        );
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if accent_button(ui, "Analyze now", 108.0).clicked() {
                self.analyze_now("manual load");
            }
            if !self.tokens.is_empty()
                && ui
                    .add(Button::new("Clear sample").min_size(Vec2::new(96.0, 26.0)))
                    .clicked()
            {
                self.clear_tokens();
                self.feedback = Some(Feedback {
                    error: false,
                    text: "Cleared the token sample.".to_string(),
                });
            }
        });
        feedback(ui, &self.feedback);
        ui.add_space(6.0);

        self.sample_stats(ui);
        ui.add_space(6.0);
        self.token_workspace(ui);
    }

    fn live_capture_page(&mut self, ui: &mut egui::Ui, history: &History) {
        page_header(
            ui,
            "Live capture",
            "Watch new Proxy HTTP history responses and collect tokens that match the capture expression. Capture starts with the next completed response.",
        );
        ui.add_space(6.0);

        let capturing = self.capture_active();
        ui.horizontal(|ui| {
            if capturing {
                let stop = Button::new(RichText::new("Stop capture").strong().color(theme::RED))
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .min_size(Vec2::new(108.0, 26.0));
                if ui.add(stop).clicked() {
                    self.stop_capture();
                }
                ui.spinner();
                ui.label(
                    RichText::new("Capturing new proxy responses")
                        .small()
                        .color(theme::BLUE),
                );
            } else if accent_button(ui, "Start live capture", 132.0).clicked() {
                self.start_capture(history);
            }
            if accent_button(ui, "Analyze now", 108.0).clicked() {
                self.analyze_now("live capture");
            }
        });
        feedback(ui, &self.feedback);
        ui.add_space(6.0);

        self.sample_stats(ui);
        stat_line(
            ui,
            "Responses scanned:",
            &self.capture.responses_scanned.to_string(),
        );
        ui.add_space(6.0);

        widgets::card(ui, |ui| {
            ui.label(
                RichText::new("Capture settings")
                    .strong()
                    .color(theme::TEXT),
            );
            ui.add_space(3.0);
            capture_settings(ui, &mut self.settings);
        });
        ui.add_space(6.0);

        self.token_workspace(ui);
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Sequencer settings",
            "Configure how tokens are captured and which statistical tests run. Changes apply to the next analysis.",
        );
        ui.add_space(6.0);

        widgets::card(ui, |ui| {
            ui.label(RichText::new("Analysis").strong().color(theme::TEXT));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [112.0, 18.0],
                    Label::new(
                        RichText::new("Token format")
                            .small()
                            .color(theme::TEXT_MUTED),
                    ),
                );
                egui::ComboBox::from_id_salt("perseid.sequencer.format")
                    .width(160.0)
                    .selected_text(self.settings.analysis.format.label())
                    .show_ui(ui, |ui| {
                        for format in TokenFormat::ALL {
                            ui.selectable_value(
                                &mut self.settings.analysis.format,
                                format,
                                format.label(),
                            );
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    [112.0, 18.0],
                    Label::new(
                        RichText::new("Significance level")
                            .small()
                            .color(theme::TEXT_MUTED),
                    ),
                );
                egui::ComboBox::from_id_salt("perseid.sequencer.significance")
                    .width(160.0)
                    .selected_text(self.settings.analysis.significance.percent_label())
                    .show_ui(ui, |ui| {
                        for significance in Significance::ALL {
                            ui.selectable_value(
                                &mut self.settings.analysis.significance,
                                significance,
                                significance.percent_label(),
                            );
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    [112.0, 18.0],
                    Label::new(
                        RichText::new("Character tests")
                            .small()
                            .color(theme::TEXT_MUTED),
                    ),
                );
                egui::ComboBox::from_id_salt("perseid.sequencer.characters")
                    .width(160.0)
                    .selected_text(self.settings.analysis.character_analysis.label())
                    .show_ui(ui, |ui| {
                        for mode in sequencer::CharacterAnalysisMode::ALL {
                            ui.selectable_value(
                                &mut self.settings.analysis.character_analysis,
                                mode,
                                mode.label(),
                            );
                        }
                    });
            });
            ui.checkbox(
                &mut self.settings.analysis.bit_analysis,
                "Run bit-level analysis",
            );
            ui.add_space(3.0);
            ui.label(
                RichText::new(
                    "Auto detects hexadecimal, Base64, then text. Character-level tests are skipped automatically when the sample has fewer than ten observations per symbol.",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
        });
        ui.add_space(8.0);

        widgets::card(ui, |ui| {
            ui.label(RichText::new("Capture").strong().color(theme::TEXT));
            ui.add_space(4.0);
            capture_settings(ui, &mut self.settings);
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui
                .add(Button::new("Reset to defaults").min_size(Vec2::new(130.0, 24.0)))
                .clicked()
            {
                let capture_regex = super::state::DEFAULT_CAPTURE_REGEX.to_string();
                self.settings = super::state::SequencerSettings {
                    analysis: AnalysisSettings::default(),
                    capture_regex,
                    ..super::state::SequencerSettings::default()
                };
                self.feedback = Some(Feedback {
                    error: false,
                    text: "Sequencer settings reset to defaults.".to_string(),
                });
            }
            ui.label(
                RichText::new("Live capture and manual load share this sample.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        });
        feedback(ui, &self.feedback);
    }

    fn sample_stats(&self, ui: &mut egui::Ui) {
        let (shortest, longest) = self.token_stats();
        stat_line(ui, "Tokens loaded:", &self.token_count().to_string());
        stat_line(
            ui,
            "Shortest:",
            &shortest.map_or_else(|| "-".to_string(), |value| value.to_string()),
        );
        stat_line(
            ui,
            "Longest:",
            &longest.map_or_else(|| "-".to_string(), |value| value.to_string()),
        );
    }

    fn token_workspace(&mut self, ui: &mut egui::Ui) {
        let height = (ui.available_height() - 2.0).max(150.0);
        let controls_width = 100.0;
        let gap = 10.0;

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(controls_width, height),
                Layout::top_down(Align::Min),
                |ui| {
                    if ui
                        .add_sized([92.0, 24.0], Button::new("Paste"))
                        .on_hover_text("Paste tokens from the clipboard")
                        .clicked()
                    {
                        self.request_paste(ui.ctx());
                    }
                    if ui
                        .add_sized([92.0, 24.0], Button::new("Load..."))
                        .on_hover_text("Load tokens from a text file")
                        .clicked()
                    {
                        self.load = Some(LoadDialog {
                            path: String::new(),
                            error: None,
                        });
                    }
                    if ui
                        .add_sized([92.0, 24.0], Button::new("Clear"))
                        .on_hover_text("Remove every token from the sample")
                        .clicked()
                    {
                        self.clear_tokens();
                        self.feedback = Some(Feedback {
                            error: false,
                            text: "Cleared the token sample.".to_string(),
                        });
                    }
                },
            );
            ui.add_space(gap);
            let width = ui.available_width().max(220.0);
            ui.allocate_ui_with_layout(
                Vec2::new(width, height),
                Layout::top_down(Align::Min),
                |ui| self.token_table(ui, height),
            );
        });
    }

    fn token_table(&self, ui: &mut egui::Ui, height: f32) {
        egui::Frame::NONE
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(3.0)
            .inner_margin(Margin::symmetric(1, 1))
            .show(ui, |ui| {
                ui.set_min_height(height);
                ui.set_max_height(height);
                let table = TableBuilder::new(ui)
                    .id_salt("perseid.sequencer.tokens")
                    .striped(true)
                    .resizable(true)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(44.0))
                    .column(Column::exact(64.0))
                    .column(Column::remainder().at_least(120.0))
                    .min_scrolled_height(height)
                    .max_scroll_height(height)
                    .auto_shrink([false, false]);
                table
                    .header(20.0, |mut header| {
                        for title in ["#", "Length", "Token"] {
                            header.col(|ui| {
                                ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(TOKEN_ROW_HEIGHT, self.tokens.len(), |mut row| {
                            let index = row.index();
                            row.col(|ui| {
                                ui.label(
                                    RichText::new((index + 1).to_string())
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(token_length(&self.tokens[index]).to_string())
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                            });
                            row.col(|ui| {
                                ui.add(
                                    Label::new(
                                        RichText::new(&self.tokens[index])
                                            .monospace()
                                            .small()
                                            .color(theme::TEXT),
                                    )
                                    .truncate(),
                                );
                            });
                        });
                    });
            });
    }

    /// Draw the floating result window and keep its state across frames.
    pub(super) fn result_window(&mut self, ctx: &egui::Context) {
        if !self.result_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.result_open = false;
            self.result = None;
            return;
        }
        let Some(analysis) = self.result.take() else {
            self.result_open = false;
            return;
        };

        let mut open = true;
        let mut rerun = false;
        egui::Window::new(&self.result_title)
            .id(egui::Id::unique(RESULT_WINDOW_ID))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(egui::pos2(60.0, 78.0))
            .default_size(egui::vec2(RESULT_WINDOW_WIDTH, RESULT_WINDOW_HEIGHT))
            .min_size(egui::vec2(760.0, 420.0))
            .show(ctx, |ui| {
                result_tabs(ui, &mut self.result_tab);
                ui.separator();
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.result_tab {
                        ResultTab::Summary => summary_tab(ui, &analysis),
                        ResultTab::Character => character_tab(ui, &analysis),
                        ResultTab::Bit => bit_tab(ui, &analysis),
                        ResultTab::Settings => {
                            rerun =
                                analysis_settings_tab(ui, &mut self.settings.analysis, &analysis);
                        }
                    });
            });

        if !open {
            self.result_open = false;
            return;
        }
        let source = analysis.source.clone();
        self.result = Some(analysis);
        if rerun {
            self.analyze_now(&source);
        }
    }

    /// Draw the path-entry dialog for manual token files.
    pub(super) fn load_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.load.take() else {
            return;
        };
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            return;
        }
        if let Some(path) = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .next()
        }) {
            dialog.path = path.display().to_string();
        }

        let mut open = true;
        let mut load = false;
        let mut cancel = false;
        egui::Window::new("Load tokens from file")
            .id(egui::Id::unique(LOAD_WINDOW_ID))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("File path").small().color(theme::TEXT_MUTED));
                let response = ui.add(
                    TextEdit::singleline(&mut dialog.path)
                        .desired_width(f32::INFINITY)
                        .hint_text("/path/to/tokens.txt"),
                );
                if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                    load = true;
                }
                if let Some(error) = &dialog.error {
                    ui.label(RichText::new(error).small().color(theme::RED));
                }
                ui.horizontal(|ui| {
                    if ui.button("Load").clicked() {
                        load = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    ui.label(
                        RichText::new("One token per line. You can also drop a file here.")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                });
            });
        if cancel || !open {
            return;
        }
        if load {
            match read_token_file(dialog.path.trim()) {
                Ok(text) => {
                    let tokens = sequencer::parse_tokens(&text);
                    let added = self.add_tokens(tokens);
                    self.feedback = Some(if added > 0 {
                        Feedback {
                            error: false,
                            text: format!("Loaded {added} token(s) from {}.", dialog.path.trim()),
                        }
                    } else {
                        Feedback {
                            error: true,
                            text: "No new tokens were found in that file.".to_string(),
                        }
                    });
                    return;
                }
                Err(error) => dialog.error = Some(error),
            }
        }
        self.load = Some(dialog);
    }
}

fn page_header(ui: &mut egui::Ui, title: &str, description: &str) {
    ui.label(RichText::new(title).size(15.0).strong().color(theme::TEXT));
    ui.add(Label::new(RichText::new(description).small().color(theme::TEXT_MUTED)).wrap());
}

fn accent_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    let button = Button::new(RichText::new(label).strong().color(Color32::WHITE))
        .fill(theme::ACCENT_DARK)
        .stroke(Stroke::new(1.0, theme::ACCENT_DARK))
        .min_size(Vec2::new(width, 26.0));
    ui.add(button)
}

fn feedback(ui: &mut egui::Ui, feedback: &Option<Feedback>) {
    if let Some(feedback) = feedback {
        ui.add_space(3.0);
        let color = if feedback.error {
            theme::RED
        } else {
            theme::GREEN
        };
        ui.add(Label::new(RichText::new(&feedback.text).small().color(color)).wrap());
    }
}

fn stat_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [112.0, 16.0],
            Label::new(RichText::new(label).small().color(theme::TEXT_MUTED)),
        );
        ui.label(RichText::new(value).small().color(theme::TEXT));
    });
}

fn capture_settings(ui: &mut egui::Ui, settings: &mut super::state::SequencerSettings) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [112.0, 18.0],
            Label::new(
                RichText::new("Capture regex")
                    .small()
                    .color(theme::TEXT_MUTED),
            ),
        );
        ui.add(
            TextEdit::singleline(&mut settings.capture_regex)
                .desired_width(360.0)
                .hint_text("[A-Za-z0-9+/=_-]{16,}"),
        );
    });
    ui.horizontal(|ui| {
        ui.add_sized(
            [112.0, 18.0],
            Label::new(
                RichText::new("Capture from")
                    .small()
                    .color(theme::TEXT_MUTED),
            ),
        );
        egui::ComboBox::from_id_salt("perseid.sequencer.capture_part")
            .width(160.0)
            .selected_text(settings.capture_part.label())
            .show_ui(ui, |ui| {
                for part in CapturePart::ALL {
                    ui.selectable_value(&mut settings.capture_part, part, part.label());
                }
            });
    });
    ui.checkbox(&mut settings.deduplicate, "Ignore duplicate tokens");
    ui.label(
        RichText::new(
            "Live capture watches responses that finish after Start. To capture one existing response, right-click its row in Proxy HTTP history and choose Send response to Sequencer.",
        )
        .small()
        .color(theme::TEXT_MUTED),
    );
}

fn result_tabs(ui: &mut egui::Ui, tab: &mut ResultTab) {
    ui.horizontal(|ui| {
        for (value, label) in [
            (ResultTab::Summary, "Summary"),
            (ResultTab::Character, "Character-level analysis"),
            (ResultTab::Bit, "Bit-level analysis"),
            (ResultTab::Settings, "Analysis settings"),
        ] {
            if widgets::section_tab(ui, *tab == value, label).clicked() {
                *tab = value;
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
            }
        }
    });
}

fn summary_tab(ui: &mut egui::Ui, analysis: &Analysis) {
    section_heading(ui, "Overall result");
    ui.label(format!(
        "The overall quality of randomness within the sample is estimated to be: {}.",
        analysis.rating.label()
    ));
    ui.label(format!(
        "At a significance level of {}, the amount of effective entropy is estimated to be: {} bits.",
        analysis.settings.significance.percent_label(),
        format_bits(analysis.primary_entropy)
    ));
    if let Some(reason) = analysis.character.skipped_reason() {
        ui.add_space(4.0);
        ui.add(Label::new(RichText::new(reason).small().color(theme::RED)).wrap());
    }
    if analysis.truncated {
        ui.add_space(3.0);
        ui.add(
            Label::new(
                RichText::new(
                    "The sample was truncated to keep the analysis bounded; the result covers the first part of each token.",
                )
                .small()
                .color(theme::AMBER),
            )
            .wrap(),
        );
    }

    ui.add_space(14.0);
    section_heading(ui, "Effective entropy");
    ui.add(
        Label::new(
            RichText::new(
                "The chart shows the number of bits of effective entropy at each significance level, based on all tests. Each level defines a minimum probability of the observed results occurring if the sample is randomly generated. Using a lower significance level means stronger evidence is required to reject the hypothesis that the sample is random.",
            )
            .small()
            .color(theme::TEXT_MUTED),
        )
        .wrap(),
    );
    ui.add_space(6.0);
    entropy_chart(ui, &analysis.entropy, analysis.total_possible_bits);
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "Primary estimate: {} analysis, {} of {} possible bits.",
            analysis.primary.label(),
            format_bits(analysis.primary_entropy),
            format_bits(analysis.total_possible_bits)
        ))
        .small()
        .color(theme::TEXT_MUTED),
    );

    ui.add_space(14.0);
    section_heading(ui, "Reliability");
    ui.label(format!(
        "The analysis is based on a sample of {} tokens. Based on the sample size, the reliability of the results is: {}.",
        analysis.sample_size,
        analysis.reliability.label()
    ));
    ui.add(
        Label::new(
            RichText::new(
                "Note that statistical tests provide only an indicative guide to the randomness of the sampled data. Results obtained may contain false positives and negatives, and may not correspond to the practical predictability of the tokens sampled.",
            )
            .small()
            .color(theme::TEXT_MUTED),
        )
        .wrap(),
    );

    ui.add_space(14.0);
    section_heading(ui, "Sample");
    ui.label(format!("Sample size: {}.", analysis.sample_size));
    let length = if analysis.shortest == analysis.longest {
        format!("Token length: {}.", analysis.longest)
    } else {
        format!(
            "Token length: {} to {} characters.",
            analysis.shortest, analysis.longest
        )
    };
    ui.label(length);
    ui.label(format!(
        "Detected representation: {}.",
        analysis.format.label()
    ));
}

fn character_tab(ui: &mut egui::Ui, analysis: &Analysis) {
    section_heading(ui, "Character-level analysis");
    match &analysis.character {
        CharacterAnalysis::Skipped { reason } => {
            ui.add(Label::new(RichText::new(reason).small().color(theme::RED)).wrap());
            ui.add_space(6.0);
            ui.add(
                Label::new(
                    RichText::new(
                        "Character-level tests compare how each character position varies across the sample, and whether adjacent characters are independent. They need many observations per symbol, so a small sample is analyzed at the bit level instead.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                )
                .wrap(),
            );
        }
        CharacterAnalysis::Performed(result) => character_result(ui, analysis, result),
    }
}

fn character_result(ui: &mut egui::Ui, analysis: &Analysis, result: &CharacterResult) {
    ui.label(format!(
        "Alphabet: {} ({} symbols).",
        result.alphabet, result.alphabet_size
    ));
    ui.label(format!(
        "Positions: {}; {} tested, {} had insufficient data.",
        result.positions, result.tested_positions, result.insufficient_positions
    ));
    ui.label(format!(
        "Count tests: {} run, {} failed at {}.",
        result.count_tests,
        result.count_failures,
        analysis.settings.significance.percent_label()
    ));
    if result.transition_tests == 0 {
        if let Some(note) = &result.transition_note {
            ui.label(RichText::new(note).small().color(theme::TEXT_MUTED));
        }
    } else {
        ui.label(format!(
            "Transition tests: {} run, {} failed at {}.",
            result.transition_tests,
            result.transition_failures,
            analysis.settings.significance.percent_label()
        ));
    }
    if result.small_sample {
        ui.add_space(4.0);
        ui.add(
            Label::new(
                RichText::new(
                    "Character-level analysis was forced on a small sample. Treat the result as indicative rather than conclusive.",
                )
                .small()
                .color(theme::AMBER),
            )
            .wrap(),
        );
    }

    ui.add_space(12.0);
    section_heading(ui, "Effective entropy");
    entropy_chart(ui, &result.entropy, result.total_bits);
    ui.add_space(8.0);
    entropy_table(
        ui,
        "perseid.sequencer.character.entropy",
        &result.entropy,
        result.total_bits,
    );
}

fn bit_tab(ui: &mut egui::Ui, analysis: &Analysis) {
    section_heading(ui, "Bit-level analysis");
    let Some(bit) = &analysis.bit else {
        ui.add(
            Label::new(
                RichText::new(
                    "Bit-level analysis was disabled in the analysis settings. Enable it and re-run the analysis to examine the decoded token bytes.",
                )
                .small()
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        return;
    };
    bit_result(ui, analysis, bit);
}

fn bit_result(ui: &mut egui::Ui, analysis: &Analysis, bit: &BitAnalysis) {
    ui.label(format!(
        "Bit positions: {}; {} tested, {} had too few samples.",
        bit.positions, bit.tested_positions, bit.insufficient_positions
    ));
    ui.label(format!(
        "Failures at {}: balance {}, runs {}, autocorrelation {}.",
        analysis.settings.significance.percent_label(),
        bit.balance_failures,
        bit.runs_failures,
        bit.correlation_failures
    ));
    ui.label(format!(
        "Total effective entropy: {} of {} bits.",
        format_bits(
            bit.entropy
                .iter()
                .find(|point| point.significance == analysis.settings.significance)
                .map_or(0.0, |point| point.bits)
        ),
        bit.total_bits
    ));
    if !bit.failing_positions.is_empty() {
        let shown: Vec<String> = bit
            .failing_positions
            .iter()
            .take(48)
            .map(usize::to_string)
            .collect();
        let suffix = if bit.failing_positions.len() > shown.len() {
            format!(" and {} more", bit.failing_positions.len() - shown.len())
        } else {
            String::new()
        };
        ui.add(
            Label::new(
                RichText::new(format!(
                    "Failing bit positions: {}{suffix}.",
                    shown.join(", ")
                ))
                .small()
                .color(theme::RED),
            )
            .wrap(),
        );
    }

    ui.add_space(12.0);
    section_heading(ui, "Effective entropy");
    entropy_chart(ui, &bit.entropy, bit.total_bits as f64);
    ui.add_space(8.0);
    entropy_table(
        ui,
        "perseid.sequencer.bit.entropy",
        &bit.entropy,
        bit.total_bits as f64,
    );

    ui.add_space(14.0);
    section_heading(ui, "Whole-stream tests");
    ui.add(
        Label::new(
            RichText::new(format!(
                "These tests run over the concatenated token bytes ({} bits{}).",
                bit.global_bits,
                if bit.global_truncated {
                    ", truncated to the analysis limit"
                } else {
                    ""
                }
            ))
            .small()
            .color(theme::TEXT_MUTED),
        )
        .wrap(),
    );
    ui.add_space(4.0);
    global_tests_table(ui, &bit.global_tests, analysis.settings.significance);
}

fn analysis_settings_tab(
    ui: &mut egui::Ui,
    settings: &mut AnalysisSettings,
    analysis: &Analysis,
) -> bool {
    section_heading(ui, "Analysis settings");
    ui.add(
        Label::new(
            RichText::new(
                "Change how the sample is interpreted or which tests run, then re-run the analysis. The current result stays visible until the new analysis completes.",
            )
            .small()
            .color(theme::TEXT_MUTED),
        )
        .wrap(),
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.add_sized(
            [150.0, 18.0],
            Label::new(
                RichText::new("Token format")
                    .small()
                    .color(theme::TEXT_MUTED),
            ),
        );
        egui::ComboBox::from_id_salt("perseid.sequencer.result.format")
            .width(180.0)
            .selected_text(settings.format.label())
            .show_ui(ui, |ui| {
                for format in TokenFormat::ALL {
                    ui.selectable_value(&mut settings.format, format, format.label());
                }
            });
        ui.label(
            RichText::new(format!("Detected: {}", analysis.format.label()))
                .small()
                .color(theme::TEXT_MUTED),
        );
    });
    ui.horizontal(|ui| {
        ui.add_sized(
            [150.0, 18.0],
            Label::new(
                RichText::new("Significance level")
                    .small()
                    .color(theme::TEXT_MUTED),
            ),
        );
        egui::ComboBox::from_id_salt("perseid.sequencer.result.significance")
            .width(180.0)
            .selected_text(settings.significance.percent_label())
            .show_ui(ui, |ui| {
                for significance in Significance::ALL {
                    ui.selectable_value(
                        &mut settings.significance,
                        significance,
                        significance.percent_label(),
                    );
                }
            });
    });
    ui.horizontal(|ui| {
        ui.add_sized(
            [150.0, 18.0],
            Label::new(
                RichText::new("Character tests")
                    .small()
                    .color(theme::TEXT_MUTED),
            ),
        );
        egui::ComboBox::from_id_salt("perseid.sequencer.result.characters")
            .width(180.0)
            .selected_text(settings.character_analysis.label())
            .show_ui(ui, |ui| {
                for mode in sequencer::CharacterAnalysisMode::ALL {
                    ui.selectable_value(&mut settings.character_analysis, mode, mode.label());
                }
            });
    });
    ui.checkbox(&mut settings.bit_analysis, "Run bit-level analysis");
    ui.add_space(8.0);

    if *settings != analysis.settings {
        ui.add(
            Label::new(
                RichText::new("Settings have changed. Re-run the analysis to apply them.")
                    .small()
                    .color(theme::AMBER),
            )
            .wrap(),
        );
        ui.add_space(4.0);
    }

    let mut rerun = false;
    ui.horizontal(|ui| {
        if accent_button(ui, "Re-run analysis", 120.0).clicked() {
            rerun = true;
        }
        ui.label(
            RichText::new(format!(
                "Sample: {} tokens; source: {}.",
                analysis.sample_size, analysis.source
            ))
            .small()
            .color(theme::TEXT_MUTED),
        );
    });

    ui.add_space(14.0);
    section_heading(ui, "Result details");
    egui::Grid::new("perseid.sequencer.result.details")
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(RichText::new("Rating").small().color(theme::TEXT_MUTED));
            ui.label(analysis.rating.label());
            ui.end_row();
            ui.label(
                RichText::new("Reliability")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.label(analysis.reliability.label());
            ui.end_row();
            ui.label(
                RichText::new("Primary estimate")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.label(format!(
                "{} analysis, {} bits",
                analysis.primary.label(),
                format_bits(analysis.primary_entropy)
            ));
            ui.end_row();
            ui.label(RichText::new("Format").small().color(theme::TEXT_MUTED));
            ui.label(analysis.format.label());
            ui.end_row();
        });
    rerun
}

fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(13.0).strong().color(theme::TEXT));
    ui.add_space(2.0);
}

fn entropy_chart(ui: &mut egui::Ui, entropy: &[EntropyPoint], possible_bits: f64) {
    let width = ui.available_width().max(380.0);
    let height = 226.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);

    let max_value = entropy
        .iter()
        .map(|point| point.bits)
        .fold(possible_bits, f64::max)
        .max(1.0);
    let (axis_max, step) = nice_axis(max_value);
    let divisions = (axis_max / step).round().max(1.0) as usize;

    let left = rect.left() + 112.0;
    let right = rect.right() - 12.0;
    let top = rect.top() + 6.0;
    let bottom = rect.bottom() - 42.0;
    let plot_width = (right - left).max(60.0);
    let plot_height = (bottom - top).max(60.0);
    let rows = entropy.len().max(1);
    let row_height = plot_height / rows as f32;

    let muted = FontId::proportional(10.0);
    let label_font = FontId::proportional(11.0);
    for division in 0..=divisions {
        let fraction = division as f32 / divisions as f32;
        let x = left + plot_width * fraction;
        painter.line_segment(
            [egui::pos2(x, top), egui::pos2(x, bottom)],
            Stroke::new(1.0, theme::BORDER),
        );
        painter.text(
            egui::pos2(x, bottom + 5.0),
            Align2::CENTER_TOP,
            format_bits(axis_max * division as f64 / divisions as f64),
            muted.clone(),
            theme::TEXT_MUTED,
        );
    }
    painter.line_segment(
        [egui::pos2(left, top), egui::pos2(left, bottom)],
        Stroke::new(1.0, theme::TEXT_MUTED),
    );
    painter.line_segment(
        [egui::pos2(left, bottom), egui::pos2(right, bottom)],
        Stroke::new(1.0, theme::TEXT_MUTED),
    );

    for (index, point) in entropy.iter().enumerate() {
        let y = top + row_height * index as f32 + row_height * 0.18;
        let bar_height = (row_height * 0.64).max(4.0);
        painter.text(
            egui::pos2(left - 8.0, y + bar_height * 0.5),
            Align2::RIGHT_CENTER,
            point.significance.chart_label(),
            label_font.clone(),
            theme::TEXT,
        );
        let bar_width = plot_width * (point.bits / axis_max).clamp(0.0, 1.0) as f32;
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(left + 1.0, y),
                Vec2::new(bar_width.max(1.0), bar_height),
            ),
            2.0,
            theme::BLUE,
        );
    }

    let axis_label = painter.layout_no_wrap(
        "Number of bits of entropy".to_string(),
        FontId::proportional(11.0),
        theme::TEXT_MUTED,
    );
    painter.add(
        egui::epaint::TextShape::new(
            egui::pos2((left + right) * 0.5, bottom + 20.0),
            axis_label,
            theme::TEXT_MUTED,
        )
        .with_angle_and_anchor(0.0, Align2::CENTER_TOP),
    );

    let y_label = painter.layout_no_wrap(
        "Significance level".to_string(),
        FontId::proportional(11.0),
        theme::TEXT_MUTED,
    );
    painter.add(
        egui::epaint::TextShape::new(
            egui::pos2(rect.left() + 12.0, (top + bottom) * 0.5),
            y_label,
            theme::TEXT_MUTED,
        )
        .with_angle_and_anchor(-FRAC_PI_2, Align2::CENTER_CENTER),
    );
}

fn nice_axis(value: f64) -> (f64, f64) {
    let step = if value <= 10.0 {
        1.0
    } else if value <= 50.0 {
        5.0
    } else if value <= 100.0 {
        10.0
    } else if value <= 500.0 {
        50.0
    } else if value <= 1_000.0 {
        100.0
    } else {
        500.0
    };
    ((value / step).ceil() * step, step)
}

fn entropy_table(ui: &mut egui::Ui, id_salt: &str, entropy: &[EntropyPoint], possible_bits: f64) {
    egui::Grid::new(id_salt)
        .num_columns(3)
        .spacing([28.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for title in ["Significance", "Effective entropy", "Share"] {
                ui.label(RichText::new(title).strong().small().color(theme::TEXT));
            }
            ui.end_row();
            for point in entropy {
                ui.label(point.significance.chart_label());
                ui.label(format!("{} bits", format_bits(point.bits)));
                let share = if possible_bits > 0.0 {
                    point.bits * 100.0 / possible_bits
                } else {
                    0.0
                };
                ui.label(format!("{share:.1}%"));
                ui.end_row();
            }
        });
}

fn global_tests_table(ui: &mut egui::Ui, tests: &[TestResult], significance: Significance) {
    let table = TableBuilder::new(ui)
        .id_salt("perseid.sequencer.tests")
        .striped(true)
        .resizable(true)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(168.0))
        .column(Column::exact(62.0))
        .column(Column::exact(76.0))
        .column(Column::remainder().at_least(180.0));
    table
        .header(20.0, |mut header| {
            for title in ["Test", "Result", "p-value", "Detail"] {
                header.col(|ui| {
                    ui.label(RichText::new(title).strong().small().color(theme::TEXT));
                });
            }
        })
        .body(|mut body| {
            for test in tests {
                body.row(20.0, |mut row| {
                    row.col(|ui| {
                        ui.label(RichText::new(test.name).small().color(theme::TEXT));
                    });
                    row.col(|ui| {
                        let (label, color) = test_status(test, significance);
                        ui.label(RichText::new(label).small().strong().color(color));
                    });
                    row.col(|ui| {
                        let value = test.p_value.map_or_else(|| "-".to_string(), format_p_value);
                        ui.label(
                            RichText::new(value)
                                .monospace()
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    });
                    row.col(|ui| {
                        ui.add(
                            Label::new(
                                RichText::new(&test.detail).small().color(theme::TEXT_MUTED),
                            )
                            .truncate(),
                        );
                    });
                });
            }
        });
}

fn test_status(test: &TestResult, significance: Significance) -> (&'static str, Color32) {
    if test.informational {
        ("Info", theme::BLUE)
    } else if test.skipped {
        ("Skipped", theme::TEXT_MUTED)
    } else if test.passed_at(significance) == Some(true) {
        ("Pass", theme::GREEN)
    } else {
        ("Fail", theme::RED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::sequencer::state::SequencerSettings;
    use crate::sequencer::{AnalysisKind, CharacterAnalysisMode, Rating, Reliability};

    fn screen_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))
    }

    fn frame(mut run: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            ..Default::default()
        };
        // The first pass lays out scroll areas and windows; inspect the second
        // pass so table rows and window contents are present.
        let mut output = ctx.run_ui(input.clone(), |ui| run(ui));
        output.textures_delta.clear();
        let mut output = ctx.run_ui(input, |ui| run(ui));
        output.textures_delta.clear();
        output
    }

    fn run_pass(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut run: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_pass(input.clone(), |ctx| run(ctx));
        output.textures_delta.clear();
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            ..Default::default()
        };
        let mut output = ctx.run_pass(input, |ctx| run(ctx));
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
    /// label is not on screen.
    fn click_text(ctx: &egui::Context, label: &str, mut run: impl FnMut(&mut egui::Ui)) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            ..Default::default()
        };
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

    /// Click the first widget inside a window whose label matches.
    fn click_window_text(
        ctx: &egui::Context,
        label: &str,
        mut run: impl FnMut(&egui::Context),
    ) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(screen_rect()),
            ..Default::default()
        };
        // Windows animate and lay out their contents over the first passes.
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

    fn entropy(bits: f64) -> Vec<EntropyPoint> {
        Significance::ALL
            .iter()
            .map(|significance| EntropyPoint {
                significance: *significance,
                bits,
            })
            .collect()
    }

    fn character_result(
        transition_tests: usize,
        transition_note: Option<&str>,
        small_sample: bool,
    ) -> CharacterResult {
        CharacterResult {
            alphabet: "AB".to_string(),
            alphabet_size: 2,
            positions: 4,
            tested_positions: 4,
            insufficient_positions: 0,
            count_tests: 4,
            count_failures: 1,
            transition_tests,
            transition_failures: 0,
            transition_note: transition_note.map(str::to_string),
            small_sample,
            total_bits: 12.0,
            entropy: entropy(12.0),
        }
    }

    fn global_tests() -> Vec<TestResult> {
        vec![
            TestResult {
                name: "Monobit frequency",
                detail: "Mostly random.".to_string(),
                p_value: Some(0.42),
                informational: false,
                skipped: false,
            },
            TestResult {
                name: "Runs",
                detail: "Not enough data.".to_string(),
                p_value: None,
                informational: false,
                skipped: true,
            },
            TestResult {
                name: "Zlib compression",
                detail: "Compressed size is 90.0%.".to_string(),
                p_value: None,
                informational: true,
                skipped: false,
            },
            TestResult {
                name: "Poker",
                detail: "Strongly non-random.".to_string(),
                p_value: Some(0.000_001),
                informational: false,
                skipped: false,
            },
        ]
    }

    fn bit_analysis(
        entropy_points: Vec<EntropyPoint>,
        failing_positions: Vec<usize>,
        global_truncated: bool,
    ) -> BitAnalysis {
        BitAnalysis {
            positions: 64,
            tested_positions: 56,
            insufficient_positions: 8,
            balance_failures: 1,
            runs_failures: 2,
            correlation_failures: 3,
            total_bits: 56,
            entropy: entropy_points,
            failing_positions,
            global_tests: global_tests(),
            global_bits: 1024,
            global_truncated,
        }
    }

    fn analysis(
        character: CharacterAnalysis,
        bit: Option<BitAnalysis>,
        truncated: bool,
    ) -> Analysis {
        Analysis {
            source: "unit test".to_string(),
            settings: AnalysisSettings::default(),
            format: TokenFormat::Hex,
            sample_size: 10,
            shortest: 16,
            longest: 16,
            truncated,
            character,
            bit,
            primary: AnalysisKind::Bit,
            entropy: entropy(12.5),
            total_possible_bits: 56.0,
            primary_entropy: 12.5,
            rating: Rating::Good,
            reliability: Reliability::Fair,
        }
    }

    #[test]
    fn nice_axis_uses_readable_steps() {
        assert_eq!(nice_axis(0.0), (0.0, 1.0));
        assert_eq!(nice_axis(10.0), (10.0, 1.0));
        assert_eq!(nice_axis(11.0), (15.0, 5.0));
        assert_eq!(nice_axis(51.0), (60.0, 10.0));
        assert_eq!(nice_axis(101.0), (150.0, 50.0));
        assert_eq!(nice_axis(501.0), (600.0, 100.0));
        assert_eq!(nice_axis(1_001.0), (1_500.0, 500.0));
    }

    #[test]
    fn test_status_distinguishes_every_result_kind() {
        let significance = Significance::OnePercent;
        let tests = global_tests();
        assert_eq!(test_status(&tests[0], significance), ("Pass", theme::GREEN));
        assert_eq!(
            test_status(&tests[1], significance),
            ("Skipped", theme::TEXT_MUTED)
        );
        assert_eq!(test_status(&tests[2], significance), ("Info", theme::BLUE));
        assert_eq!(test_status(&tests[3], significance), ("Fail", theme::RED));
    }

    #[test]
    fn summary_tab_renders_warnings_and_both_length_layouts() {
        let skipped = analysis(
            CharacterAnalysis::Skipped {
                reason: "Character analysis needs a bigger sample.".to_string(),
            },
            Some(bit_analysis(entropy(10.0), Vec::new(), true)),
            true,
        );
        let text = rendered_text(&frame(|ui| summary_tab(ui, &skipped)));
        assert!(text.contains("Character analysis needs a bigger sample."));
        assert!(text.contains("sample was truncated"));
        assert!(text.contains("Token length: 16."));
        assert!(text.contains("good"));

        let mut performed = analysis(
            CharacterAnalysis::Performed(character_result(2, None, false)),
            None,
            false,
        );
        performed.shortest = 8;
        performed.longest = 16;
        let text = rendered_text(&frame(|ui| summary_tab(ui, &performed)));
        assert!(text.contains("Token length: 8 to 16 characters."));
        assert!(!text.contains("sample was truncated"));
    }

    #[test]
    fn character_tab_renders_skipped_performed_and_notes() {
        let skipped = analysis(
            CharacterAnalysis::Skipped {
                reason: "Character analysis was disabled.".to_string(),
            },
            None,
            false,
        );
        let text = rendered_text(&frame(|ui| character_tab(ui, &skipped)));
        assert!(text.contains("Character analysis was disabled."));

        let note = analysis(
            CharacterAnalysis::Performed(character_result(0, Some("Need more pairs."), true)),
            None,
            false,
        );
        let text = rendered_text(&frame(|ui| character_tab(ui, &note)));
        assert!(text.contains("Need more pairs."));
        assert!(text.contains("forced on a small sample"));
        assert!(text.contains("Count tests: 4 run, 1 failed at 1%."));

        let transitions = analysis(
            CharacterAnalysis::Performed(character_result(3, None, false)),
            None,
            false,
        );
        let text = rendered_text(&frame(|ui| character_tab(ui, &transitions)));
        assert!(text.contains("Transition tests: 3 run, 0 failed at 1%."));
        assert!(!text.contains("forced on a small sample"));
    }

    #[test]
    fn bit_tab_renders_skipped_truncated_and_failing_positions() {
        let disabled = analysis(
            CharacterAnalysis::Skipped {
                reason: "Skipped.".to_string(),
            },
            None,
            false,
        );
        let text = rendered_text(&frame(|ui| bit_tab(ui, &disabled)));
        assert!(text.contains("Bit-level analysis was disabled"));

        let failing = analysis(
            CharacterAnalysis::Skipped {
                reason: "Skipped.".to_string(),
            },
            Some(bit_analysis(Vec::new(), (0..60).collect(), true)),
            false,
        );
        let text = rendered_text(&frame(|ui| bit_tab(ui, &failing)));
        assert!(text.contains("0 of 56 bits."));
        assert!(text.contains("and 12 more"));
        assert!(text.contains("truncated to the analysis limit"));
        assert!(text.contains("Pass"));
        assert!(text.contains("Skipped"));
        assert!(text.contains("Info"));
        assert!(text.contains("Fail"));
        assert!(text.contains("<0.00001"));
        assert!(text.contains("-"));

        let complete = analysis(
            CharacterAnalysis::Skipped {
                reason: "Skipped.".to_string(),
            },
            Some(bit_analysis(entropy(9.5), vec![3], false)),
            false,
        );
        let text = rendered_text(&frame(|ui| bit_tab(ui, &complete)));
        assert!(text.contains("Failing bit positions: 3."));
        assert!(!text.contains("and 0 more"));
        assert!(!text.contains("truncated to the analysis limit"));
    }

    #[test]
    fn analysis_settings_tab_reports_changed_settings() {
        let analysis = analysis(
            CharacterAnalysis::Performed(character_result(2, None, false)),
            None,
            false,
        );
        let mut settings = analysis.settings;
        let mut rerun = true;
        let text = rendered_text(&frame(|ui| {
            rerun = analysis_settings_tab(ui, &mut settings, &analysis);
        }));
        assert!(!rerun);
        assert!(text.contains("Re-run analysis"));
        assert!(!text.contains("Settings have changed"));

        settings.bit_analysis = false;
        let text = rendered_text(&frame(|ui| {
            rerun = analysis_settings_tab(ui, &mut settings, &analysis);
        }));
        assert!(!rerun);
        assert!(text.contains("Settings have changed"));
    }

    #[test]
    fn entropy_chart_and_table_handle_empty_and_large_samples() {
        let empty = frame(|ui| entropy_chart(ui, &[], 0.0));
        assert!(!empty.shapes.is_empty());

        let large = frame(|ui| {
            entropy_chart(ui, &entropy(2_000.0), 100.0);
            entropy_table(ui, "perseid.test.entropy.empty", &entropy(5.0), 0.0);
        });
        let text = rendered_text(&large);
        assert!(text.contains("0.0%"));
        assert!(text.contains("5 bits"));
    }

    #[test]
    fn sequencer_pages_render_tokens_and_capture_states() {
        let mut history = History::new(10, 1024 * 1024);
        history.load(Vec::new());

        for section in [
            SequencerSection::LiveCapture,
            SequencerSection::ManualLoad,
            SequencerSection::Settings,
        ] {
            let mut state = SequencerState::new();
            state.section = section;
            state.add_tokens(["deadbeef".to_string(), "cafebabe".to_string()]);
            state.feedback = Some(Feedback {
                error: section == SequencerSection::Settings,
                text: "unit-test feedback".to_string(),
            });
            state.capture.active = section == SequencerSection::LiveCapture;
            state.capture.responses_scanned = 7;

            let text = rendered_text(&frame(|ui| state.ui(ui, &history)));
            assert!(text.contains("unit-test feedback"));
            if section != SequencerSection::Settings {
                assert!(text.contains("deadbeef"));
            }
            match section {
                SequencerSection::LiveCapture => {
                    assert!(text.contains("Stop capture"));
                    assert!(text.contains("Responses scanned:"));
                    assert!(text.contains("7"));
                }
                SequencerSection::ManualLoad => assert!(text.contains("Manual load")),
                SequencerSection::Settings => assert!(text.contains("Sequencer settings")),
            }
        }

        let mut state = SequencerState::new();
        state.section = SequencerSection::LiveCapture;
        let text = rendered_text(&frame(|ui| state.ui(ui, &history)));
        assert!(text.contains("Start live capture"));
    }

    #[test]
    fn result_window_renders_every_tab_and_keeps_the_analysis() {
        let ctx = egui::Context::default();
        let mut state = SequencerState::new();
        let both = analysis(
            CharacterAnalysis::Performed(character_result(2, None, false)),
            Some(bit_analysis(entropy(10.0), vec![1, 2], false)),
            false,
        );
        state.result = Some(both);
        state.result_open = true;
        state.result_title = "Perseid Sequencer [unit test analysis #1]".to_string();

        for (tab, expected) in [
            (ResultTab::Summary, "Overall result"),
            (ResultTab::Character, "Character-level analysis"),
            (ResultTab::Bit, "Bit-level analysis"),
            (ResultTab::Settings, "Analysis settings"),
        ] {
            state.result_tab = tab;
            let output = run_pass(&ctx, Vec::new(), |ctx| state.result_window(ctx));
            let text = rendered_text(&output);
            assert!(text.contains(expected), "missing {expected:?}");
            assert!(text.contains("Perseid Sequencer [unit test analysis #1]"));
            assert!(state.result_open);
            assert!(state.result.is_some());
        }
    }

    #[test]
    fn result_window_closes_on_escape_or_missing_analysis() {
        let ctx = egui::Context::default();
        let mut state = SequencerState::new();
        state.result = Some(analysis(
            CharacterAnalysis::Skipped {
                reason: "Skipped.".to_string(),
            },
            None,
            false,
        ));
        state.result_open = true;
        let escape = egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        run_pass(&ctx, vec![escape], |ctx| state.result_window(ctx));
        assert!(!state.result_open);
        assert!(state.result.is_none());

        state.result_open = true;
        state.result = None;
        run_pass(&ctx, Vec::new(), |ctx| state.result_window(ctx));
        assert!(!state.result_open);
    }

    #[test]
    fn load_window_keeps_errors_and_cancels_on_escape() {
        let ctx = egui::Context::default();
        let mut state = SequencerState::new();
        run_pass(&ctx, Vec::new(), |ctx| state.load_window(ctx));

        state.load = Some(LoadDialog {
            path: "/missing/tokens.txt".to_string(),
            error: Some("could not read /missing/tokens.txt".to_string()),
        });
        let output = run_pass(&ctx, Vec::new(), |ctx| state.load_window(ctx));
        assert!(state.load.is_some());
        assert!(rendered_text(&output).contains("could not read /missing/tokens.txt"));

        let escape = egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        run_pass(&ctx, vec![escape], |ctx| state.load_window(ctx));
        assert!(state.load.is_none());
    }

    #[test]
    fn clicking_load_reads_files_reports_errors_and_cancels() {
        let ctx = egui::Context::default();
        let mut state = SequencerState::new();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "perseid-sequencer-view-{}-{nonce}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "alpha\nbeta").unwrap();

        state.load = Some(LoadDialog {
            path: path.display().to_string(),
            error: None,
        });
        assert!(click_window_text(&ctx, "Load", |ctx| state.load_window(ctx)));
        assert!(state.load.is_none());
        assert_eq!(state.tokens, ["alpha", "beta"]);
        assert_eq!(
            state.feedback.as_ref().unwrap().text,
            format!("Loaded 2 token(s) from {}.", path.display())
        );

        state.load = Some(LoadDialog {
            path: path.display().to_string(),
            error: None,
        });
        assert!(click_window_text(&ctx, "Load", |ctx| state.load_window(ctx)));
        assert!(state.load.is_none());
        assert_eq!(
            state.feedback.as_ref().unwrap().text,
            "No new tokens were found in that file."
        );

        state.load = Some(LoadDialog {
            path: "/missing/perseid-tokens.txt".to_string(),
            error: None,
        });
        assert!(click_window_text(&ctx, "Load", |ctx| state.load_window(ctx)));
        let dialog = state.load.as_ref().unwrap();
        assert!(
            dialog
                .error
                .as_deref()
                .is_some_and(|error| error.contains("could not read"))
        );

        assert!(click_window_text(&ctx, "Cancel", |ctx| state.load_window(ctx)));
        assert!(state.load.is_none());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn clicking_result_settings_changes_options_and_reruns() {
        let ctx = egui::Context::default();
        let mut state = SequencerState::new();
        state.tokens = vec![
            "aaaaaaaaaaaaaaaa".to_string(),
            "bbbbbbbbbbbbbbbb".to_string(),
        ];
        state.result = Some(analysis(
            CharacterAnalysis::Skipped {
                reason: "Skipped.".to_string(),
            },
            None,
            false,
        ));
        state.result_open = true;

        assert!(click_window_text(&ctx, "Analysis settings", |ctx| state.result_window(ctx)));
        assert_eq!(state.result_tab, ResultTab::Settings);

        assert!(click_window_text(&ctx, "Auto detect", |ctx| state.result_window(ctx)));
        assert!(click_window_text(&ctx, "Base64", |ctx| state.result_window(ctx)));
        assert_eq!(state.settings.analysis.format, TokenFormat::Base64);

        assert!(click_window_text(&ctx, "1%", |ctx| state.result_window(ctx)));
        assert!(click_window_text(&ctx, "0.1%", |ctx| state.result_window(ctx)));
        assert_eq!(
            state.settings.analysis.significance,
            Significance::PointOnePercent
        );

        assert!(click_window_text(&ctx, "Auto", |ctx| state.result_window(ctx)));
        assert!(click_window_text(&ctx, "Always", |ctx| state.result_window(ctx)));
        assert_eq!(
            state.settings.analysis.character_analysis,
            CharacterAnalysisMode::Always
        );

        assert!(click_window_text(&ctx, "Re-run analysis", |ctx| state.result_window(ctx)));
        assert_eq!(state.analysis_count, 1);
        assert_eq!(state.result_tab, ResultTab::Summary);
        assert!(state.result.is_some());
        assert!(state.result_open);
    }

    #[test]
    fn settings_defaults_cover_the_shared_controls() {
        let settings = SequencerSettings::default();
        assert_eq!(settings.capture_part, CapturePart::Body);
        assert_eq!(
            settings.capture_regex,
            super::super::state::DEFAULT_CAPTURE_REGEX
        );
        assert!(settings.deduplicate);
        assert_eq!(SequencerSection::default(), SequencerSection::ManualLoad);
        assert_eq!(ResultTab::default(), ResultTab::Summary);
        assert_eq!(
            CapturePart::ALL.map(CapturePart::label),
            ["Response body", "Response headers", "Headers and body"]
        );
        assert_eq!(
            CharacterAnalysisMode::ALL
                .map(CharacterAnalysisMode::label)
                .len(),
            3
        );
        assert_eq!(TokenFormat::ALL.map(TokenFormat::label).len(), 4);
        assert_eq!(Significance::ALL.len(), 5);
    }

    #[test]
    fn clicking_live_capture_controls_starts_stops_and_analyzes() {
        let ctx = egui::Context::default();
        let history = History::new(10, 1024);
        let mut state = SequencerState::new();
        state.section = SequencerSection::LiveCapture;
        state.add_tokens([
            "deadbeefcafebabe".to_string(),
            "0011223344556677".to_string(),
        ]);

        assert!(click_text(&ctx, "Start live capture", |ui| state.ui(ui, &history)));
        assert!(state.capture_active());
        assert!(click_text(&ctx, "Stop capture", |ui| state.ui(ui, &history)));
        assert!(!state.capture_active());
        assert!(click_text(&ctx, "Analyze now", |ui| state.ui(ui, &history)));
        assert!(state.result_open);
        assert!(state.result.is_some());
    }

    #[test]
    fn clicking_manual_workspace_controls_requests_and_clears() {
        let ctx = egui::Context::default();
        let history = History::new(10, 1024);
        let mut state = SequencerState::new();
        state.add_tokens([
            "deadbeefcafebabe".to_string(),
            "0011223344556677".to_string(),
        ]);

        assert!(click_text(&ctx, "Paste", |ui| state.ui(ui, &history)));
        assert_eq!(state.paste.as_ref().map(|paste| paste.frames_left), Some(3));
        state.paste = None;

        assert!(click_text(&ctx, "Load...", |ui| state.ui(ui, &history)));
        assert!(state.load.is_some());
        state.load = None;

        assert!(click_text(&ctx, "Clear", |ui| state.ui(ui, &history)));
        assert!(state.tokens.is_empty());
        assert_eq!(
            state.feedback.as_ref().unwrap().text,
            "Cleared the token sample."
        );
    }

    #[test]
    fn clicking_reset_restores_sequencer_defaults() {
        let ctx = egui::Context::default();
        let history = History::new(10, 1024);
        let mut state = SequencerState::new();
        state.section = SequencerSection::Settings;
        state.settings.analysis.format = TokenFormat::Hex;
        state.settings.analysis.bit_analysis = false;
        state.settings.capture_regex = "custom".to_string();

        assert!(click_text(&ctx, "Reset to defaults", |ui| state.ui(ui, &history)));
        assert_eq!(state.settings.analysis, AnalysisSettings::default());
        assert_eq!(
            state.settings.capture_regex,
            super::super::state::DEFAULT_CAPTURE_REGEX
        );
        assert_eq!(
            state.feedback.as_ref().unwrap().text,
            "Sequencer settings reset to defaults."
        );
    }

    #[test]
    fn clicking_settings_combos_updates_capture_and_analysis_options() {
        let ctx = egui::Context::default();
        let history = History::new(10, 1024);
        let mut state = SequencerState::new();
        state.section = SequencerSection::Settings;

        assert!(click_text(&ctx, "Auto detect", |ui| state.ui(ui, &history)));
        assert!(click_text(&ctx, "Hex", |ui| state.ui(ui, &history)));
        assert_eq!(state.settings.analysis.format, TokenFormat::Hex);

        assert!(click_text(&ctx, "1%", |ui| state.ui(ui, &history)));
        assert!(click_text(&ctx, "10%", |ui| state.ui(ui, &history)));
        assert_eq!(
            state.settings.analysis.significance,
            Significance::TenPercent
        );

        assert!(click_text(&ctx, "Auto", |ui| state.ui(ui, &history)));
        assert!(click_text(&ctx, "Never", |ui| state.ui(ui, &history)));
        assert_eq!(
            state.settings.analysis.character_analysis,
            CharacterAnalysisMode::Never
        );

        assert!(click_text(&ctx, "Response body", |ui| state.ui(ui, &history)));
        assert!(click_text(&ctx, "Response headers", |ui| state.ui(ui, &history)));
        assert_eq!(state.settings.capture_part, CapturePart::Headers);

        assert!(click_text(&ctx, "Run bit-level analysis", |ui| state.ui(ui, &history)));
        assert!(!state.settings.analysis.bit_analysis);

        assert!(click_text(&ctx, "Ignore duplicate tokens", |ui| state.ui(ui, &history)));
        assert!(!state.settings.deduplicate);
    }

    #[test]
    fn clicking_section_tabs_and_manual_controls_updates_the_state() {
        let ctx = egui::Context::default();
        let history = History::new(10, 1024);
        let mut state = SequencerState::new();
        state.add_tokens([
            "deadbeefcafebabe".to_string(),
            "0011223344556677".to_string(),
        ]);

        assert!(click_text(&ctx, "Live capture", |ui| state.ui(ui, &history)));
        assert_eq!(state.section, SequencerSection::LiveCapture);
        assert!(click_text(&ctx, "Sequencer settings", |ui| state.ui(ui, &history)));
        assert_eq!(state.section, SequencerSection::Settings);
        assert!(click_text(&ctx, "Manual load", |ui| state.ui(ui, &history)));
        assert_eq!(state.section, SequencerSection::ManualLoad);

        assert!(click_text(&ctx, "Analyze now", |ui| state.ui(ui, &history)));
        assert!(state.result_open);
        assert!(state.result.is_some());
        state.result_open = false;
        state.result = None;

        assert!(click_text(&ctx, "Clear sample", |ui| state.ui(ui, &history)));
        assert!(state.tokens.is_empty());
        assert_eq!(
            state.feedback.as_ref().unwrap().text,
            "Cleared the token sample."
        );
    }
}
