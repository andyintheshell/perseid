//! Request-editor layout: keep `$$` markers visible and highlight positions.

use egui::{Color32, FontId};

use crate::theme;

/// Build the request editor layout while keeping `$$` visible and marking the
/// enclosed position. The text is appended verbatim so selection, copying, and
/// editing still operate on the raw request.
pub(super) fn request_layout_job(
    text: &str,
    wrap_width: f32,
    font: &FontId,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.keep_trailing_whitespace = true;

    let marker = crate::proxy::MARKER;
    let mut cursor = 0;
    while let Some(relative_open) = text[cursor..].find(marker) {
        let open = cursor + relative_open;
        append_text(
            &mut job,
            &text[cursor..open],
            font,
            theme::TEXT,
            Color32::TRANSPARENT,
        );
        let content_start = open + marker.len();
        let Some(relative_close) = text[content_start..].find(marker) else {
            append_text(
                &mut job,
                &text[open..],
                font,
                theme::RED,
                Color32::TRANSPARENT,
            );
            return job;
        };
        let close = content_start + relative_close;
        append_text(
            &mut job,
            marker,
            font,
            theme::ACCENT_DARK,
            theme::ACCENT_SOFT,
        );
        append_text(
            &mut job,
            &text[content_start..close],
            font,
            theme::TEXT,
            theme::SELECTED,
        );
        append_text(
            &mut job,
            marker,
            font,
            theme::ACCENT_DARK,
            theme::ACCENT_SOFT,
        );
        cursor = close + marker.len();
    }
    append_text(
        &mut job,
        &text[cursor..],
        font,
        theme::TEXT,
        Color32::TRANSPARENT,
    );
    job
}

fn append_text(
    job: &mut egui::text::LayoutJob,
    text: &str,
    font: &FontId,
    color: Color32,
    background: Color32,
) {
    if text.is_empty() {
        return;
    }
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color,
            background,
            ..Default::default()
        },
    );
}

pub(super) fn byte_offset_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(offset, _)| offset)
}
