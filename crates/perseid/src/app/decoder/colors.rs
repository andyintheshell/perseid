//! Colors used for Decoder operations, spans, and wash backgrounds.

use egui::Color32;

use crate::decoder as engine;
use crate::theme;

pub(super) fn transform_color(transform: engine::Transform) -> Color32 {
    match transform {
        engine::Transform::Plain => theme::TEXT_MUTED,
        engine::Transform::Decode(codec) | engine::Transform::Encode(codec) => codec_color(codec),
        engine::Transform::Hash(_) => hash_color(),
    }
}

pub(super) fn tag_color(tag: engine::ColorTag) -> Color32 {
    match tag {
        engine::ColorTag::Codec(codec) => codec_color(codec),
        engine::ColorTag::Hash => hash_color(),
        engine::ColorTag::Plain => theme::TEXT,
    }
}

pub(super) fn tag_wash(tag: engine::ColorTag) -> Color32 {
    match tag {
        engine::ColorTag::Codec(codec) => codec_wash(codec),
        engine::ColorTag::Hash => Color32::from_rgb(0xf1, 0xed, 0xf9),
        engine::ColorTag::Plain => Color32::TRANSPARENT,
    }
}

pub(super) fn codec_color(codec: engine::Codec) -> Color32 {
    match codec {
        engine::Codec::Url => Color32::from_rgb(0xc0, 0x39, 0x2b),
        engine::Codec::Html => Color32::from_rgb(0xc2, 0x6a, 0x00),
        engine::Codec::Base64 => Color32::from_rgb(0x8c, 0x86, 0x00),
        engine::Codec::AsciiHex => Color32::from_rgb(0x2f, 0x7d, 0x4f),
        engine::Codec::Hex => Color32::from_rgb(0x2f, 0x6f, 0x9f),
        engine::Codec::Octal => Color32::from_rgb(0x00, 0x83, 0x8a),
        engine::Codec::Binary => Color32::from_rgb(0xa3, 0x2a, 0xa3),
        engine::Codec::Gzip => Color32::from_rgb(0x8a, 0x4b, 0x1f),
    }
}

fn codec_wash(codec: engine::Codec) -> Color32 {
    match codec {
        engine::Codec::Url => Color32::from_rgb(0xfd, 0xee, 0xec),
        engine::Codec::Html => Color32::from_rgb(0xfd, 0xf2, 0xe3),
        engine::Codec::Base64 => Color32::from_rgb(0xfa, 0xf8, 0xdc),
        engine::Codec::AsciiHex => Color32::from_rgb(0xec, 0xf6, 0xef),
        engine::Codec::Hex => Color32::from_rgb(0xec, 0xf1, 0xf9),
        engine::Codec::Octal => Color32::from_rgb(0xe7, 0xf5, 0xf5),
        engine::Codec::Binary => Color32::from_rgb(0xf7, 0xec, 0xf7),
        engine::Codec::Gzip => Color32::from_rgb(0xf4, 0xed, 0xe8),
    }
}

fn hash_color() -> Color32 {
    Color32::from_rgb(0x6b, 0x4a, 0xa8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_and_tag_colors_follow_the_same_palette() {
        assert_eq!(transform_color(engine::Transform::Plain), theme::TEXT_MUTED);
        assert_eq!(tag_color(engine::ColorTag::Plain), theme::TEXT);

        for codec in engine::Codec::ALL {
            let color = codec_color(codec);
            assert_eq!(transform_color(engine::Transform::Decode(codec)), color);
            assert_eq!(transform_color(engine::Transform::Encode(codec)), color);
            assert_eq!(tag_color(engine::ColorTag::Codec(codec)), color);
        }

        let hash = hash_color();
        assert_eq!(
            transform_color(engine::Transform::Hash(engine::HashAlgorithm::Sha256)),
            hash
        );
        assert_eq!(tag_color(engine::ColorTag::Hash), hash);
    }

    #[test]
    fn wash_colors_match_their_tag_family() {
        for codec in engine::Codec::ALL {
            assert_eq!(tag_wash(engine::ColorTag::Codec(codec)), codec_wash(codec));
        }
        assert_eq!(
            tag_wash(engine::ColorTag::Hash),
            Color32::from_rgb(0xf1, 0xed, 0xf9)
        );
        assert_eq!(tag_wash(engine::ColorTag::Plain), Color32::TRANSPARENT);
    }
}
