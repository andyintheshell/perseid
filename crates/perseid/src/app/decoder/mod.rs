//! Decoder: stacked panels for encoding, decoding, hashing, and smart decode.
//!
//! The layout follows Burp's Decoder. Every transformation produces a new
//! editable panel, and the operation that produced it is shown as a colored
//! chip. Text selections are transformed in place within a new panel; applying
//! an operation to an earlier panel replaces the downstream chain.
//!
//! The byte-level transforms live in [`crate::decoder`]. This module owns the
//! panel state, selection handling, coloring, and the egui layout.

mod colors;
mod state;
mod view;

use crate::decoder as engine;
use crate::project::{
    DecoderCodecRecord, DecoderColorTagRecord, DecoderHashRecord, DecoderOperationRecord,
    DecoderPanelWorkspace, DecoderSpanWorkspace, DecoderViewRecord, TextSelectionWorkspace,
};

use super::PerseidApp;

pub(super) use state::DecoderState;

use state::{Panel, ViewMode};

fn codec_record(codec: engine::Codec) -> DecoderCodecRecord {
    match codec {
        engine::Codec::Url => DecoderCodecRecord::Url,
        engine::Codec::Html => DecoderCodecRecord::Html,
        engine::Codec::Base64 => DecoderCodecRecord::Base64,
        engine::Codec::AsciiHex => DecoderCodecRecord::AsciiHex,
        engine::Codec::Hex => DecoderCodecRecord::Hex,
        engine::Codec::Octal => DecoderCodecRecord::Octal,
        engine::Codec::Binary => DecoderCodecRecord::Binary,
        engine::Codec::Gzip => DecoderCodecRecord::Gzip,
    }
}

fn codec_from_record(codec: DecoderCodecRecord) -> engine::Codec {
    match codec {
        DecoderCodecRecord::Url => engine::Codec::Url,
        DecoderCodecRecord::Html => engine::Codec::Html,
        DecoderCodecRecord::Base64 => engine::Codec::Base64,
        DecoderCodecRecord::AsciiHex => engine::Codec::AsciiHex,
        DecoderCodecRecord::Hex => engine::Codec::Hex,
        DecoderCodecRecord::Octal => engine::Codec::Octal,
        DecoderCodecRecord::Binary => engine::Codec::Binary,
        DecoderCodecRecord::Gzip => engine::Codec::Gzip,
    }
}

fn hash_record(hash: engine::HashAlgorithm) -> DecoderHashRecord {
    match hash {
        engine::HashAlgorithm::Blake2b160 => DecoderHashRecord::Blake2b160,
        engine::HashAlgorithm::Blake2b256 => DecoderHashRecord::Blake2b256,
        engine::HashAlgorithm::Blake2b384 => DecoderHashRecord::Blake2b384,
        engine::HashAlgorithm::Blake2b512 => DecoderHashRecord::Blake2b512,
        engine::HashAlgorithm::Blake2s128 => DecoderHashRecord::Blake2s128,
        engine::HashAlgorithm::Blake2s160 => DecoderHashRecord::Blake2s160,
        engine::HashAlgorithm::Blake2s224 => DecoderHashRecord::Blake2s224,
        engine::HashAlgorithm::Blake2s256 => DecoderHashRecord::Blake2s256,
        engine::HashAlgorithm::Blake3 => DecoderHashRecord::Blake3,
        engine::HashAlgorithm::Keccak224 => DecoderHashRecord::Keccak224,
        engine::HashAlgorithm::Keccak256 => DecoderHashRecord::Keccak256,
        engine::HashAlgorithm::Keccak384 => DecoderHashRecord::Keccak384,
        engine::HashAlgorithm::Keccak512 => DecoderHashRecord::Keccak512,
        engine::HashAlgorithm::Md2 => DecoderHashRecord::Md2,
        engine::HashAlgorithm::Md4 => DecoderHashRecord::Md4,
        engine::HashAlgorithm::Md5 => DecoderHashRecord::Md5,
        engine::HashAlgorithm::Sha1 => DecoderHashRecord::Sha1,
        engine::HashAlgorithm::Sha224 => DecoderHashRecord::Sha224,
        engine::HashAlgorithm::Sha256 => DecoderHashRecord::Sha256,
        engine::HashAlgorithm::Sha384 => DecoderHashRecord::Sha384,
        engine::HashAlgorithm::Sha512 => DecoderHashRecord::Sha512,
        engine::HashAlgorithm::Sha512_224 => DecoderHashRecord::Sha512_224,
        engine::HashAlgorithm::Sha512_256 => DecoderHashRecord::Sha512_256,
        engine::HashAlgorithm::Sha3_224 => DecoderHashRecord::Sha3_224,
        engine::HashAlgorithm::Sha3_256 => DecoderHashRecord::Sha3_256,
        engine::HashAlgorithm::Sha3_384 => DecoderHashRecord::Sha3_384,
        engine::HashAlgorithm::Sha3_512 => DecoderHashRecord::Sha3_512,
        engine::HashAlgorithm::Sm3 => DecoderHashRecord::Sm3,
        engine::HashAlgorithm::Tiger => DecoderHashRecord::Tiger,
        engine::HashAlgorithm::Whirlpool => DecoderHashRecord::Whirlpool,
        engine::HashAlgorithm::Ripemd160 => DecoderHashRecord::Ripemd160,
    }
}

fn hash_from_record(hash: DecoderHashRecord) -> engine::HashAlgorithm {
    match hash {
        DecoderHashRecord::Blake2b160 => engine::HashAlgorithm::Blake2b160,
        DecoderHashRecord::Blake2b256 => engine::HashAlgorithm::Blake2b256,
        DecoderHashRecord::Blake2b384 => engine::HashAlgorithm::Blake2b384,
        DecoderHashRecord::Blake2b512 => engine::HashAlgorithm::Blake2b512,
        DecoderHashRecord::Blake2s128 => engine::HashAlgorithm::Blake2s128,
        DecoderHashRecord::Blake2s160 => engine::HashAlgorithm::Blake2s160,
        DecoderHashRecord::Blake2s224 => engine::HashAlgorithm::Blake2s224,
        DecoderHashRecord::Blake2s256 => engine::HashAlgorithm::Blake2s256,
        DecoderHashRecord::Blake3 => engine::HashAlgorithm::Blake3,
        DecoderHashRecord::Keccak224 => engine::HashAlgorithm::Keccak224,
        DecoderHashRecord::Keccak256 => engine::HashAlgorithm::Keccak256,
        DecoderHashRecord::Keccak384 => engine::HashAlgorithm::Keccak384,
        DecoderHashRecord::Keccak512 => engine::HashAlgorithm::Keccak512,
        DecoderHashRecord::Md2 => engine::HashAlgorithm::Md2,
        DecoderHashRecord::Md4 => engine::HashAlgorithm::Md4,
        DecoderHashRecord::Md5 => engine::HashAlgorithm::Md5,
        DecoderHashRecord::Sha1 => engine::HashAlgorithm::Sha1,
        DecoderHashRecord::Sha224 => engine::HashAlgorithm::Sha224,
        DecoderHashRecord::Sha256 => engine::HashAlgorithm::Sha256,
        DecoderHashRecord::Sha384 => engine::HashAlgorithm::Sha384,
        DecoderHashRecord::Sha512 => engine::HashAlgorithm::Sha512,
        DecoderHashRecord::Sha512_224 => engine::HashAlgorithm::Sha512_224,
        DecoderHashRecord::Sha512_256 => engine::HashAlgorithm::Sha512_256,
        DecoderHashRecord::Sha3_224 => engine::HashAlgorithm::Sha3_224,
        DecoderHashRecord::Sha3_256 => engine::HashAlgorithm::Sha3_256,
        DecoderHashRecord::Sha3_384 => engine::HashAlgorithm::Sha3_384,
        DecoderHashRecord::Sha3_512 => engine::HashAlgorithm::Sha3_512,
        DecoderHashRecord::Sm3 => engine::HashAlgorithm::Sm3,
        DecoderHashRecord::Tiger => engine::HashAlgorithm::Tiger,
        DecoderHashRecord::Whirlpool => engine::HashAlgorithm::Whirlpool,
        DecoderHashRecord::Ripemd160 => engine::HashAlgorithm::Ripemd160,
    }
}

fn tag_record(tag: engine::ColorTag) -> DecoderColorTagRecord {
    match tag {
        engine::ColorTag::Codec(codec) => DecoderColorTagRecord::Codec(codec_record(codec)),
        engine::ColorTag::Hash => DecoderColorTagRecord::Hash,
        engine::ColorTag::Plain => DecoderColorTagRecord::Plain,
    }
}

fn tag_from_record(tag: DecoderColorTagRecord) -> engine::ColorTag {
    match tag {
        DecoderColorTagRecord::Codec(codec) => engine::ColorTag::Codec(codec_from_record(codec)),
        DecoderColorTagRecord::Hash => engine::ColorTag::Hash,
        DecoderColorTagRecord::Plain => engine::ColorTag::Plain,
    }
}

fn operation_record(operation: engine::Transform) -> DecoderOperationRecord {
    match operation {
        engine::Transform::Plain => DecoderOperationRecord::Plain,
        engine::Transform::Decode(codec) => DecoderOperationRecord::Decode(codec_record(codec)),
        engine::Transform::Encode(codec) => DecoderOperationRecord::Encode(codec_record(codec)),
        engine::Transform::Hash(hash) => DecoderOperationRecord::Hash(hash_record(hash)),
    }
}

fn operation_from_record(operation: DecoderOperationRecord) -> engine::Transform {
    match operation {
        DecoderOperationRecord::Plain => engine::Transform::Plain,
        DecoderOperationRecord::Decode(codec) => {
            engine::Transform::Decode(codec_from_record(codec))
        }
        DecoderOperationRecord::Encode(codec) => {
            engine::Transform::Encode(codec_from_record(codec))
        }
        DecoderOperationRecord::Hash(hash) => engine::Transform::Hash(hash_from_record(hash)),
    }
}

fn selection_workspace(
    selection: &Option<std::ops::Range<usize>>,
) -> Option<TextSelectionWorkspace> {
    selection.as_ref().map(|range| TextSelectionWorkspace {
        start: range.start,
        end: range.end,
    })
}

fn selection_from_workspace(
    text: &str,
    selection: Option<&TextSelectionWorkspace>,
) -> Option<std::ops::Range<usize>> {
    let selection = selection?;
    let mut start = selection.start.min(text.len());
    let mut end = selection.end.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (start != end).then(|| start.min(end)..start.max(end))
}

impl PerseidApp {
    pub(super) fn decoder_view(&mut self, ui: &mut egui::Ui) {
        self.decoder.ui(ui);
    }

    pub(super) fn decoder_workspace(&self) -> Vec<DecoderPanelWorkspace> {
        self.decoder
            .panels
            .iter()
            .map(|panel| DecoderPanelWorkspace {
                id: panel.id,
                data: panel.data.clone(),
                spans: panel
                    .spans
                    .iter()
                    .map(|span| DecoderSpanWorkspace {
                        start: span.start,
                        end: span.end,
                        tag: tag_record(span.tag),
                    })
                    .collect(),
                operation: panel.operation.map(operation_record),
                view: match panel.view {
                    ViewMode::Text => DecoderViewRecord::Text,
                    ViewMode::Hex => DecoderViewRecord::Hex,
                },
                selection: selection_workspace(&panel.selection),
            })
            .collect()
    }

    pub(super) fn apply_decoder_workspace(&mut self, panels: &[DecoderPanelWorkspace]) {
        if panels.is_empty() {
            self.decoder.reset();
            return;
        }
        self.decoder.panels.clear();
        let mut next_id = 1;
        for workspace in panels {
            let id = workspace.id.max(next_id);
            next_id = id.saturating_add(1).max(next_id.saturating_add(1));
            let spans = workspace
                .spans
                .iter()
                .filter_map(|span| {
                    if span.start < span.end && span.end <= workspace.data.len() {
                        Some(engine::Span {
                            start: span.start,
                            end: span.end,
                            tag: tag_from_record(span.tag),
                        })
                    } else {
                        None
                    }
                })
                .collect();
            let operation = workspace.operation.map(operation_from_record);
            let mut panel = Panel::new(id, workspace.data.clone(), spans, operation);
            panel.view = match workspace.view {
                DecoderViewRecord::Text => ViewMode::Text,
                DecoderViewRecord::Hex => ViewMode::Hex,
            };
            panel.selection = selection_from_workspace(&panel.text, workspace.selection.as_ref());
            self.decoder.panels.push(panel);
        }
        self.decoder.next_id = next_id;
        self.decoder.scroll_to_end = false;
    }
}
