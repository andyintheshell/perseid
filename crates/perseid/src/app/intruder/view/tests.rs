use egui::FontId;

use crate::theme;

use super::layout::{byte_offset_for_char, request_layout_job};

#[test]
fn request_layout_preserves_every_byte_and_marks_positions() {
    let font = FontId::monospace(crate::highlight::MESSAGE_FONT_SIZE);
    let cases = [
        "GET /?id=$$1$$ HTTP/1.1\nHost: example.test\n\n",
        "POST / HTTP/1.1\r\n\r\nuser=$$admin$$",
        "GET /?q=$$unclosed HTTP/1.1\n\n",
        "GET / HTTP/1.1\n\n",
    ];
    for text in cases {
        let job = request_layout_job(text, 200.0, &font);
        assert_eq!(job.text, text);
    }
    let job = request_layout_job(cases[0], 200.0, &font);
    assert!(
        job.sections
            .iter()
            .any(|section| section.format.background == theme::SELECTED),
        "the payload position should receive a background highlight"
    );
}

#[test]
fn byte_offsets_are_utf8_character_boundaries() {
    let text = "Привет $$мир$$";
    let start = byte_offset_for_char(text, 9);
    let end = byte_offset_for_char(text, 12);
    assert_eq!(&text[start..end], "мир");
    assert_eq!(byte_offset_for_char(text, 10_000), text.len());
}
