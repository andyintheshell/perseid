//! Decoder transforms: encoding, decoding, hashing, and smart decoding.
//!
//! The desktop Decoder keeps every transformation in its own panel. This
//! module is the engine behind those panels: it is deliberately independent of
//! egui so the byte-level behavior can be tested without a window. Transform
//! results carry byte ranges tagged with the operation that produced them; the
//! UI uses those ranges to color transformed data in both Text and Hex views.
//!
//! The numeric operations follow Burp's behavior rather than treating Hex,
//! Octal, and Binary as byte encodings. They convert maximal runs of digits:
//! for example, `Encode as > Hex` turns `port 1234` into `port 4d2`, and
//! `Decode as > Hex` turns `deadbeef` into `3735928559`. `ASCII hex` is the
//! byte-oriented hexadecimal encoding.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};
use std::io::{Read, Write};
use std::ops::Range;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use blake2::digest::consts::{U16, U20, U28, U32, U48, U64, U128};
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use num_bigint::BigUint;

/// Largest input a single transform will process.
///
/// This keeps pasted payloads and decompression bombs from turning the
/// utility tab into an unbounded allocation path. The UI reports the limit
/// instead of silently truncating transformed data.
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// Largest output a single transform may produce.
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Maximum number of layers `Smart decode` will apply.
pub const SMART_MAX_LAYERS: usize = 12;

/// A byte-oriented codec exposed by Decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    Url,
    Html,
    Base64,
    AsciiHex,
    Hex,
    Octal,
    Binary,
    Gzip,
}

impl Codec {
    /// Menu order used by Burp Decoder.
    pub const ALL: [Self; 8] = [
        Self::Url,
        Self::Html,
        Self::Base64,
        Self::AsciiHex,
        Self::Hex,
        Self::Octal,
        Self::Binary,
        Self::Gzip,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Url => "URL",
            Self::Html => "HTML",
            Self::Base64 => "Base64",
            Self::AsciiHex => "ASCII hex",
            Self::Hex => "Hex",
            Self::Octal => "Octal",
            Self::Binary => "Binary",
            Self::Gzip => "Gzip",
        }
    }
}

/// A transformation selected in a Decoder panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transform {
    Plain,
    Decode(Codec),
    Encode(Codec),
    Hash(HashAlgorithm),
}

impl Transform {
    pub fn label(self) -> String {
        match self {
            Self::Plain => "Plain".to_string(),
            Self::Decode(codec) => format!("{} decode", codec.label()),
            Self::Encode(codec) => format!("{} encode", codec.label()),
            Self::Hash(algorithm) => algorithm.label().to_string(),
        }
    }

    pub const fn color_tag(self) -> ColorTag {
        match self {
            Self::Plain => ColorTag::Plain,
            Self::Decode(codec) | Self::Encode(codec) => ColorTag::Codec(codec),
            Self::Hash(_) => ColorTag::Hash,
        }
    }
}

/// The color family used for a transformed run of bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorTag {
    Codec(Codec),
    Hash,
    Plain,
}

/// A byte range produced by one transformation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub tag: ColorTag,
}

/// Bytes and their transform provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transformed {
    pub data: Vec<u8>,
    pub spans: Vec<Span>,
}

impl Transformed {
    /// Color tag at a byte offset, if any.
    #[cfg(test)]
    pub fn tag_at(&self, offset: usize) -> Option<ColorTag> {
        self.spans
            .iter()
            .find(|span| span.start <= offset && offset < span.end)
            .map(|span| span.tag)
    }
}

/// One layer found by `Smart decode`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartStep {
    pub codec: Codec,
    pub data: Vec<u8>,
}

/// Hash algorithms available in the Decoder.
///
/// Fixed-output digest variants offered by Decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HashAlgorithm {
    Blake2b160,
    Blake2b256,
    Blake2b384,
    Blake2b512,
    Blake2s128,
    Blake2s160,
    Blake2s224,
    Blake2s256,
    Blake3,
    Keccak224,
    Keccak256,
    Keccak384,
    Keccak512,
    Md2,
    Md4,
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha512_224,
    Sha512_256,
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
    Sm3,
    Tiger,
    Whirlpool,
    Ripemd160,
    Skein256_128,
    Skein256_160,
    Skein256_224,
    Skein256_256,
    Skein512_128,
    Skein512_160,
    Skein512_224,
    Skein512_256,
    Skein512_384,
    Skein512_512,
    Skein1024_384,
    Skein1024_512,
    Skein1024_1024,
    TupleHash128,
    TupleHash256,
}

impl HashAlgorithm {
    /// Standard digest OIDs registered by common Java security providers.
    /// An alias selects the canonical algorithm and has the same output.
    pub const OID_ALIASES: &'static [(&'static str, Self)] = &[
        ("1.3.6.1.4.1.1722.12.2.1.5", Self::Blake2b160),
        ("1.3.6.1.4.1.1722.12.2.1.8", Self::Blake2b256),
        ("1.3.6.1.4.1.1722.12.2.1.12", Self::Blake2b384),
        ("1.3.6.1.4.1.1722.12.2.1.16", Self::Blake2b512),
        ("1.3.6.1.4.1.1722.12.2.2.4", Self::Blake2s128),
        ("1.3.6.1.4.1.1722.12.2.2.5", Self::Blake2s160),
        ("1.3.6.1.4.1.1722.12.2.2.7", Self::Blake2s224),
        ("1.3.6.1.4.1.1722.12.2.2.8", Self::Blake2s256),
        ("1.2.840.113549.2.2", Self::Md2),
        ("1.2.840.113549.2.4", Self::Md4),
        ("1.2.840.113549.2.5", Self::Md5),
        ("1.3.14.3.2.26", Self::Sha1),
        ("2.16.840.1.101.3.4.2.4", Self::Sha224),
        ("2.16.840.1.101.3.4.2.1", Self::Sha256),
        ("2.16.840.1.101.3.4.2.2", Self::Sha384),
        ("2.16.840.1.101.3.4.2.3", Self::Sha512),
        ("2.16.840.1.101.3.4.2.5", Self::Sha512_224),
        ("2.16.840.1.101.3.4.2.6", Self::Sha512_256),
        ("2.16.840.1.101.3.4.2.7", Self::Sha3_224),
        ("2.16.840.1.101.3.4.2.8", Self::Sha3_256),
        ("2.16.840.1.101.3.4.2.9", Self::Sha3_384),
        ("2.16.840.1.101.3.4.2.10", Self::Sha3_512),
        ("1.3.36.3.2.1", Self::Ripemd160),
        ("1.0.10118.3.0.49", Self::Ripemd160),
        ("1.0.10118.3.0.55", Self::Whirlpool),
        ("1.2.156.10197.1.401", Self::Sm3),
        ("1.2.156.197.1.401", Self::Sm3),
    ];
    pub const ALL: [Self; 46] = [
        Self::Blake2b160,
        Self::Blake2b256,
        Self::Blake2b384,
        Self::Blake2b512,
        Self::Blake2s128,
        Self::Blake2s160,
        Self::Blake2s224,
        Self::Blake2s256,
        Self::Blake3,
        Self::Keccak224,
        Self::Keccak256,
        Self::Keccak384,
        Self::Keccak512,
        Self::Md2,
        Self::Md4,
        Self::Md5,
        Self::Ripemd160,
        Self::Skein256_128,
        Self::Skein256_160,
        Self::Skein256_224,
        Self::Skein256_256,
        Self::Skein512_128,
        Self::Skein512_160,
        Self::Skein512_224,
        Self::Skein512_256,
        Self::Skein512_384,
        Self::Skein512_512,
        Self::Skein1024_384,
        Self::Skein1024_512,
        Self::Skein1024_1024,
        Self::TupleHash128,
        Self::TupleHash256,
        Self::Sha1,
        Self::Sha224,
        Self::Sha256,
        Self::Sha384,
        Self::Sha512,
        Self::Sha512_224,
        Self::Sha512_256,
        Self::Sha3_224,
        Self::Sha3_256,
        Self::Sha3_384,
        Self::Sha3_512,
        Self::Sm3,
        Self::Tiger,
        Self::Whirlpool,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Blake2b160 => "BLAKE2B-160",
            Self::Blake2b256 => "BLAKE2B-256",
            Self::Blake2b384 => "BLAKE2B-384",
            Self::Blake2b512 => "BLAKE2B-512",
            Self::Blake2s128 => "BLAKE2S-128",
            Self::Blake2s160 => "BLAKE2S-160",
            Self::Blake2s224 => "BLAKE2S-224",
            Self::Blake2s256 => "BLAKE2S-256",
            Self::Blake3 => "BLAKE3-256",
            Self::Keccak224 => "KECCAK-224",
            Self::Keccak256 => "KECCAK-256",
            Self::Keccak384 => "KECCAK-384",
            Self::Keccak512 => "KECCAK-512",
            Self::Md2 => "MD2",
            Self::Md4 => "MD4",
            Self::Md5 => "MD5",
            Self::Sha1 => "SHA-1",
            Self::Sha224 => "SHA-224",
            Self::Sha256 => "SHA-256",
            Self::Sha384 => "SHA-384",
            Self::Sha512 => "SHA-512",
            Self::Sha512_224 => "SHA-512/224",
            Self::Sha512_256 => "SHA-512/256",
            Self::Sha3_224 => "SHA3-224",
            Self::Sha3_256 => "SHA3-256",
            Self::Sha3_384 => "SHA3-384",
            Self::Sha3_512 => "SHA3-512",
            Self::Sm3 => "SM3",
            Self::Tiger => "Tiger",
            Self::Whirlpool => "Whirlpool",
            Self::Ripemd160 => "RIPEMD-160",
            Self::Skein256_128 => "SKEIN-256-128",
            Self::Skein256_160 => "SKEIN-256-160",
            Self::Skein256_224 => "SKEIN-256-224",
            Self::Skein256_256 => "SKEIN-256-256",
            Self::Skein512_128 => "SKEIN-512-128",
            Self::Skein512_160 => "SKEIN-512-160",
            Self::Skein512_224 => "SKEIN-512-224",
            Self::Skein512_256 => "SKEIN-512-256",
            Self::Skein512_384 => "SKEIN-512-384",
            Self::Skein512_512 => "SKEIN-512-512",
            Self::Skein1024_384 => "SKEIN-1024-384",
            Self::Skein1024_512 => "SKEIN-1024-512",
            Self::Skein1024_1024 => "SKEIN-1024-1024",
            Self::TupleHash128 => "TupleHash128-256",
            Self::TupleHash256 => "TupleHash256-512",
        }
    }
}

/// A user-facing transform failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformError {
    message: String,
}

impl TransformError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TransformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TransformError {}

pub type Result<T> = std::result::Result<T, TransformError>;

/// Apply a transform to all of `input`, or only to `selection`.
///
/// The selection is clamped to the input. An empty selection means the whole
/// input, matching Decoder's behavior when nothing is selected. Untransformed
/// prefix and suffix bytes are copied into the result verbatim.
pub fn transform(
    input: &[u8],
    selection: Option<Range<usize>>,
    transform: Transform,
) -> Result<Transformed> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(TransformError::new(format!(
            "input is {} bytes; Decoder transforms are limited to {} bytes",
            input.len(),
            MAX_INPUT_BYTES
        )));
    }
    let range = normalize_selection(input.len(), selection);
    let source = &input[range.clone()];
    let converted = match transform {
        Transform::Plain => source.to_vec(),
        Transform::Decode(codec) => decode(codec, source)?,
        Transform::Encode(codec) => encode(codec, source)?,
        Transform::Hash(algorithm) => hash_hex(algorithm, source)?.into_bytes(),
    };
    if converted.len() > MAX_OUTPUT_BYTES {
        return Err(TransformError::new(format!(
            "output is {} bytes; Decoder transforms are limited to {} bytes",
            converted.len(),
            MAX_OUTPUT_BYTES
        )));
    }

    let tag = transform.color_tag();
    if range.start == 0 && range.end == input.len() {
        return Ok(Transformed {
            spans: if converted.is_empty() {
                Vec::new()
            } else {
                vec![Span {
                    start: 0,
                    end: converted.len(),
                    tag,
                }]
            },
            data: converted,
        });
    }

    let prefix_len = range.start;
    let transformed_end = prefix_len + converted.len();
    let mut data = Vec::with_capacity(input.len() - source.len() + converted.len());
    data.extend_from_slice(&input[..range.start]);
    data.extend_from_slice(&converted);
    data.extend_from_slice(&input[range.end..]);
    if data.len() > MAX_OUTPUT_BYTES {
        return Err(TransformError::new(format!(
            "output is {} bytes; Decoder transforms are limited to {} bytes",
            data.len(),
            MAX_OUTPUT_BYTES
        )));
    }
    Ok(Transformed {
        data,
        spans: if transformed_end == prefix_len {
            Vec::new()
        } else {
            vec![Span {
                start: prefix_len,
                end: transformed_end,
                tag,
            }]
        },
    })
}

/// Apply a named encoding or decoding operation to the whole input.
#[cfg(test)]
pub fn apply(operation: Transform, input: &[u8]) -> Result<Vec<u8>> {
    Ok(transform(input, None, operation)?.data)
}

/// Clamp a byte selection to `length`, falling back to the whole input.
pub(crate) fn normalize_selection(length: usize, selection: Option<Range<usize>>) -> Range<usize> {
    match selection {
        Some(range) if range.start < range.end && range.start < length => {
            range.start.min(length)..range.end.min(length)
        }
        _ => 0..length,
    }
}

fn decode(codec: Codec, input: &[u8]) -> Result<Vec<u8>> {
    match codec {
        Codec::Url => Ok(percent_decode(input)),
        Codec::Html => Ok(html_decode(input)),
        Codec::Base64 => base64_decode(input),
        Codec::AsciiHex => ascii_hex_decode(input),
        Codec::Hex => numeric_decode(input, 16),
        Codec::Octal => numeric_decode(input, 8),
        Codec::Binary => numeric_decode(input, 2),
        Codec::Gzip => gzip_decode(input),
    }
}

fn encode(codec: Codec, input: &[u8]) -> Result<Vec<u8>> {
    match codec {
        Codec::Url => percent_encode(input),
        Codec::Html => html_encode(input),
        Codec::Base64 => Ok(STANDARD.encode(input).into_bytes()),
        Codec::AsciiHex => Ok(ascii_hex_encode(input)),
        Codec::Hex => numeric_encode(input, 16),
        Codec::Octal => numeric_encode(input, 8),
        Codec::Binary => numeric_encode(input, 2),
        Codec::Gzip => gzip_encode(input),
    }
}

fn percent_encode(input: &[u8]) -> Result<Vec<u8>> {
    let projected = input.len().saturating_mul(3);
    if projected > MAX_OUTPUT_BYTES {
        return Err(TransformError::new(format!(
            "URL encoding would produce {projected} bytes; the limit is {MAX_OUTPUT_BYTES} bytes"
        )));
    }
    let mut output = Vec::with_capacity(projected);
    for &byte in input {
        output.push(b'%');
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    Ok(output)
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%'
            && index + 2 < input.len()
            && let (Some(high), Some(low)) =
                (hex_value(input[index + 1]), hex_value(input[index + 2]))
        {
            output.push((high << 4) | low);
            index += 3;
            continue;
        }
        output.push(input[index]);
        index += 1;
    }
    output
}

fn html_encode(input: &[u8]) -> Result<Vec<u8>> {
    let mut output = String::with_capacity(input.len().min(MAX_OUTPUT_BYTES));
    if let Ok(text) = std::str::from_utf8(input) {
        for character in text.chars() {
            // Burp encodes every character, not just HTML metacharacters.
            push_html_entity(&mut output, character as u32)?;
        }
    } else {
        // Binary data has no Unicode characters to encode. Emit one numeric
        // entity per byte as a textual representation; decoding is byte-exact
        // for ASCII and widens higher bytes because entities denote Unicode
        // scalar values, not raw bytes.
        for &byte in input {
            push_html_entity(&mut output, u32::from(byte))?;
        }
    }
    Ok(output.into_bytes())
}

fn push_html_entity(output: &mut String, codepoint: u32) -> Result<()> {
    write!(output, "&#x{codepoint:x};").expect("writing to a String cannot fail");
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(TransformError::new(format!(
            "HTML encoding would exceed the {MAX_OUTPUT_BYTES}-byte output limit"
        )));
    }
    Ok(())
}

fn html_decode(input: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(input);
    html_escape::decode_html_entities(&text)
        .into_owned()
        .into_bytes()
}

fn base64_decode(input: &[u8]) -> Result<Vec<u8>> {
    let compact: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(decoded) = engine.decode(&compact) {
            return Ok(decoded);
        }
    }
    Err(TransformError::new(
        "invalid Base64 data; check the alphabet, padding, and whitespace",
    ))
}

fn ascii_hex_encode(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len().saturating_mul(2));
    for &byte in input {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

fn ascii_hex_decode(input: &[u8]) -> Result<Vec<u8>> {
    let compact: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    if !compact.len().is_multiple_of(2) {
        return Err(TransformError::new(
            "ASCII hex needs an even number of hexadecimal digits",
        ));
    }
    let mut output = Vec::with_capacity(compact.len() / 2);
    for pair in compact.as_chunks::<2>().0 {
        let (Some(high), Some(low)) = (hex_value(pair[0]), hex_value(pair[1])) else {
            return Err(TransformError::new(
                "ASCII hex contains a character that is not a hexadecimal digit",
            ));
        };
        output.push((high << 4) | low);
    }
    Ok(output)
}

fn numeric_encode(input: &[u8], to_radix: u32) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index].is_ascii_digit() {
            let start = index;
            while index < input.len() && input[index].is_ascii_digit() {
                index += 1;
            }
            let token = &input[start..index];
            if token.len() > 16_384 {
                return Err(TransformError::new(
                    "numeric run is too long to convert safely",
                ));
            }
            let value = BigUint::parse_bytes(token, 10)
                .ok_or_else(|| TransformError::new("could not parse decimal number"))?;
            let converted = value.to_str_radix(to_radix);
            if output.len().saturating_add(converted.len()) > MAX_OUTPUT_BYTES {
                return Err(TransformError::new(format!(
                    "numeric conversion would exceed the {MAX_OUTPUT_BYTES}-byte output limit"
                )));
            }
            output.extend_from_slice(converted.as_bytes());
        } else {
            output.push(input[index]);
            index += 1;
        }
    }
    Ok(output)
}

fn numeric_decode(input: &[u8], from_radix: u32) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if digit_value(input[index]).is_some_and(|value| value < from_radix) {
            let start = index;
            while index < input.len()
                && digit_value(input[index]).is_some_and(|value| value < from_radix)
            {
                index += 1;
            }
            let token = &input[start..index];
            if token.len() > 16_384 {
                return Err(TransformError::new(
                    "numeric run is too long to convert safely",
                ));
            }
            let value = BigUint::parse_bytes(token, from_radix)
                .ok_or_else(|| TransformError::new("could not parse numeric run"))?;
            let converted = value.to_str_radix(10);
            if output.len().saturating_add(converted.len()) > MAX_OUTPUT_BYTES {
                return Err(TransformError::new(format!(
                    "numeric conversion would exceed the {MAX_OUTPUT_BYTES}-byte output limit"
                )));
            }
            output.extend_from_slice(converted.as_bytes());
        } else {
            output.push(input[index]);
            index += 1;
        }
    }
    Ok(output)
}

fn gzip_encode(input: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|error| TransformError::new(format!("gzip encode failed: {error}")))?;
    let output = encoder
        .finish()
        .map_err(|error| TransformError::new(format!("gzip encode failed: {error}")))?;
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(TransformError::new(format!(
            "gzip output is {} bytes; the limit is {MAX_OUTPUT_BYTES} bytes",
            output.len()
        )));
    }
    Ok(output)
}

fn gzip_decode(input: &[u8]) -> Result<Vec<u8>> {
    let decoder = MultiGzDecoder::new(input);
    let mut output = Vec::new();
    decoder
        .take(MAX_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|error| TransformError::new(format!("gzip decode failed: {error}")))?;
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(TransformError::new(format!(
            "gzip output exceeds the {MAX_OUTPUT_BYTES}-byte limit"
        )));
    }
    Ok(output)
}

/// Hex digest as lowercase ASCII.
pub fn hash_hex(algorithm: HashAlgorithm, input: &[u8]) -> Result<String> {
    let digest = hash_bytes(algorithm, input)?;
    Ok(hex_lower(&digest))
}

fn hash_bytes(algorithm: HashAlgorithm, input: &[u8]) -> Result<Vec<u8>> {
    use HashAlgorithm as H;

    let bytes = match algorithm {
        H::Blake2b160 => digest::<blake2::Blake2b<U20>>(input),
        H::Blake2b256 => digest::<blake2::Blake2b<U32>>(input),
        H::Blake2b384 => digest::<blake2::Blake2b<U48>>(input),
        H::Blake2b512 => digest::<blake2::Blake2b<U64>>(input),
        H::Blake2s128 => digest::<blake2::Blake2s<U16>>(input),
        H::Blake2s160 => digest::<blake2::Blake2s<U20>>(input),
        H::Blake2s224 => digest::<blake2::Blake2s<U28>>(input),
        H::Blake2s256 => digest::<blake2::Blake2s<U32>>(input),
        H::Blake3 => blake3::hash(input).as_bytes().to_vec(),
        H::Keccak224 => digest::<sha3::Keccak224>(input),
        H::Keccak256 => digest::<sha3::Keccak256>(input),
        H::Keccak384 => digest::<sha3::Keccak384>(input),
        H::Keccak512 => digest::<sha3::Keccak512>(input),
        H::Md2 => digest::<md2::Md2>(input),
        H::Md4 => digest::<md4::Md4>(input),
        H::Md5 => digest::<md5::Md5>(input),
        H::Sha1 => digest::<sha1::Sha1>(input),
        H::Sha224 => digest::<sha2::Sha224>(input),
        H::Sha256 => digest::<sha2::Sha256>(input),
        H::Sha384 => digest::<sha2::Sha384>(input),
        H::Sha512 => digest::<sha2::Sha512>(input),
        H::Sha512_224 => digest::<sha2::Sha512_224>(input),
        H::Sha512_256 => digest::<sha2::Sha512_256>(input),
        H::Sha3_224 => digest::<sha3::Sha3_224>(input),
        H::Sha3_256 => digest::<sha3::Sha3_256>(input),
        H::Sha3_384 => digest::<sha3::Sha3_384>(input),
        H::Sha3_512 => digest::<sha3::Sha3_512>(input),
        H::Sm3 => digest::<sm3::Sm3>(input),
        H::Tiger => digest::<tiger::Tiger>(input),
        H::Whirlpool => digest::<whirlpool::Whirlpool>(input),
        H::Ripemd160 => digest::<ripemd::Ripemd160>(input),
        H::Skein256_128 => digest::<skein::Skein256<U16>>(input),
        H::Skein256_160 => digest::<skein::Skein256<U20>>(input),
        H::Skein256_224 => digest::<skein::Skein256<U28>>(input),
        H::Skein256_256 => digest::<skein::Skein256<U32>>(input),
        H::Skein512_128 => digest::<skein::Skein512<U16>>(input),
        H::Skein512_160 => digest::<skein::Skein512<U20>>(input),
        H::Skein512_224 => digest::<skein::Skein512<U28>>(input),
        H::Skein512_256 => digest::<skein::Skein512<U32>>(input),
        H::Skein512_384 => digest::<skein::Skein512<U48>>(input),
        H::Skein512_512 => digest::<skein::Skein512<U64>>(input),
        H::Skein1024_384 => digest::<skein::Skein1024<U48>>(input),
        H::Skein1024_512 => digest::<skein::Skein1024<U64>>(input),
        H::Skein1024_1024 => digest::<skein::Skein1024<U128>>(input),
        H::TupleHash128 => tuple_hash::<168>(&[input], 32),
        H::TupleHash256 => tuple_hash::<136>(&[input], 64),
    };
    Ok(bytes)
}

// SP 800-185 TupleHash: Decoder's input is one tuple element, not a
// delimiter-separated list. This also lets tests exercise multi-element vectors.
fn tuple_hash<const RATE: usize>(items: &[&[u8]], output_len: usize) -> Vec<u8> {
    use digest::{ExtendableOutput, Update, XofReader};
    fn encode(value: u64, right: bool) -> Vec<u8> {
        let bytes = value.to_be_bytes();
        let start = bytes.iter().position(|byte| *byte != 0).unwrap_or(7);
        let n = (8 - start) as u8;
        let mut encoded = Vec::with_capacity(n as usize + 1);
        if !right {
            encoded.push(n);
        }
        encoded.extend_from_slice(&bytes[start..]);
        if right {
            encoded.push(n);
        }
        encoded
    }
    let mut hasher = cshake::CShake::<RATE>::new_with_function_name(b"TupleHash", b"");
    for item in items {
        hasher.update(&encode((item.len() as u64) * 8, false));
        hasher.update(item);
    }
    hasher.update(&encode((output_len as u64) * 8, true));
    let mut output = vec![0; output_len];
    hasher.finalize_xof().read(&mut output);
    output
}

fn digest<D: digest::Digest + Default>(input: &[u8]) -> Vec<u8> {
    let mut hasher = D::new();
    hasher.update(input);
    hasher.finalize().to_vec()
}

/// Apply smart decoding until no further recognizable layer is found.
///
/// One entry is returned per layer. Numeric Hex/Octal/Binary conversions are
/// deliberately excluded: a plain decimal token should not be reinterpreted as
/// a hexadecimal number. This matches Burp's conservative smart-decode
/// behavior for values such as `%34%37`, which stops at `47`.
pub fn smart_decode(input: &[u8]) -> Vec<SmartStep> {
    let mut current = input.to_vec();
    let mut seen = BTreeSet::new();
    seen.insert(current.clone());
    let mut steps = Vec::new();
    for _ in 0..SMART_MAX_LAYERS {
        let Some((codec, next)) = next_smart_layer(&current) else {
            break;
        };
        if next.is_empty() || next == current || seen.contains(&next) {
            break;
        }
        seen.insert(next.clone());
        steps.push(SmartStep {
            codec,
            data: next.clone(),
        });
        current = next;
    }
    steps
}

fn next_smart_layer(input: &[u8]) -> Option<(Codec, Vec<u8>)> {
    if input.starts_with(&[0x1f, 0x8b])
        && let Ok(decoded) = gzip_decode(input)
        && !decoded.is_empty()
    {
        return Some((Codec::Gzip, decoded));
    }

    if has_url_escape(input) {
        let decoded = percent_decode(input);
        if decoded != input {
            return Some((Codec::Url, decoded));
        }
    }

    if has_html_entity(input) {
        let decoded = html_decode(input);
        if decoded != input {
            return Some((Codec::Html, decoded));
        }
    }

    if let Some(decoded) = plausible_base64(input) {
        return Some((Codec::Base64, decoded));
    }

    if let Some(decoded) = plausible_ascii_hex(input) {
        return Some((Codec::AsciiHex, decoded));
    }

    None
}

fn has_url_escape(input: &[u8]) -> bool {
    input.windows(3).any(|window| {
        window[0] == b'%' && hex_value(window[1]).is_some() && hex_value(window[2]).is_some()
    })
}

fn has_html_entity(input: &[u8]) -> bool {
    for (index, &byte) in input.iter().enumerate() {
        if byte != b'&' {
            continue;
        }
        let tail = &input[index + 1..];
        let Some(end) = tail.iter().position(|&candidate| candidate == b';') else {
            continue;
        };
        if end == 0 || end > 32 {
            continue;
        }
        let body = &tail[..end];
        let valid = if body[0] == b'#' {
            let digits = if body.get(1) == Some(&b'x') || body.get(1) == Some(&b'X') {
                &body[2..]
            } else {
                &body[1..]
            };
            !digits.is_empty()
                && digits
                    .iter()
                    .all(|digit| digit.is_ascii_hexdigit() || digit.is_ascii_digit())
        } else {
            body.iter()
                .all(|&character| character.is_ascii_alphanumeric())
        };
        if valid {
            return true;
        }
    }
    false
}

fn plausible_base64(input: &[u8]) -> Option<Vec<u8>> {
    if input.len() > MAX_INPUT_BYTES {
        return None;
    }
    let compact: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if compact.len() < 8 || compact.len() > MAX_OUTPUT_BYTES {
        return None;
    }
    let text = std::str::from_utf8(&compact).ok()?;
    if !text.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_')
    }) {
        return None;
    }
    let padding = text.bytes().rev().take_while(|&byte| byte == b'=').count();
    if padding > 2 || text[..text.len().saturating_sub(padding)].contains('=') {
        return None;
    }
    let has_upper = text.bytes().any(|byte| byte.is_ascii_uppercase());
    let has_lower = text.bytes().any(|byte| byte.is_ascii_lowercase());
    let has_digit = text.bytes().any(|byte| byte.is_ascii_digit());
    let classes = [has_upper, has_lower, has_digit]
        .into_iter()
        .filter(|present| *present)
        .count();
    if padding == 0 && classes < 2 {
        return None;
    }
    let decoded = base64_decode(&compact).ok()?;
    if decoded.is_empty() || decoded == input {
        return None;
    }
    if is_probably_text(&decoded)
        || decoded.starts_with(&[0x1f, 0x8b])
        || (decoded.starts_with(b"%") && has_url_escape(&decoded))
    {
        Some(decoded)
    } else {
        None
    }
}

fn plausible_ascii_hex(input: &[u8]) -> Option<Vec<u8>> {
    let compact: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if compact.len() < 8 || !compact.len().is_multiple_of(2) {
        return None;
    }
    if !compact.iter().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let decoded = ascii_hex_decode(&compact).ok()?;
    if decoded.is_empty() || !is_probably_text(&decoded) {
        return None;
    }
    Some(decoded)
}

/// Whether bytes look like a text payload rather than binary output.
///
/// Valid UTF-8 with at most ten percent control characters is treated as text.
/// Newlines, carriage returns, and tabs do not count as controls for this
/// check.
pub fn is_probably_text(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if text.is_empty() {
        return true;
    }
    let mut total = 0usize;
    let mut printable = 0usize;
    for character in text.chars() {
        total += 1;
        if !character.is_control() || matches!(character, '\n' | '\r' | '\t') {
            printable += 1;
        }
    }
    printable.saturating_mul(100) >= total.saturating_mul(90)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        10..=15 => b'a' + (value - 10),
        _ => b'0',
    }
}

pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn digit_value(byte: u8) -> Option<u32> {
    hex_value(byte).map(u32::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skein_golden_vectors() {
        // Skein 1.3 golden KAT, empty message for each state size.
        for (algorithm, expected) in [
            (
                HashAlgorithm::Skein256_256,
                "c8877087da56e072870daa843f176e9453115929094c3a40c463a196c29bf7ba",
            ),
            (
                HashAlgorithm::Skein512_512,
                concat!(
                    "bc5b4c50925519c290cc634277ae3d6257212395cba733bbad37a4af0fa06af4",
                    "1fca7903d06564fea7a2d3730dbdb80c1f85562dfcc070334ea4d1d9e72cba7a"
                ),
            ),
            (
                HashAlgorithm::Skein1024_1024,
                concat!(
                    "0fff9563bb3279289227ac77d319b6fff8d7e9f09da1247b72a0a265cd6d2a62",
                    "645ad547ed8193db48cff847c06494a03f55666d3b47eb4c20456c9373c86297",
                    "d630d5578ebd34cb40991578f9f52b18003efa35d3da6553ff35db91b81ab890",
                    "bec1b189b7f52cb2a783ebb7d823d725b0b4a71f6824e88f68f982eefc6d19c6"
                ),
            ),
        ] {
            assert_eq!(hash_hex(algorithm, b"").unwrap(), expected);
        }
    }

    #[test]
    fn tuple_hash_nist_vectors_and_single_input() {
        // NIST SP 800-185 TupleHash samples 1 and 4, empty customization.
        let items: &[&[u8]] = &[&[0, 1, 2], &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15]];
        assert_eq!(
            hex_lower(&tuple_hash::<168>(items, 32)),
            "c5d8786c1afb9b82111ab34b65b2c0048fa64e6d48e263264ce1707d3ffc8ed1"
        );
        assert_eq!(
            hex_lower(&tuple_hash::<136>(items, 64)),
            concat!(
                "cfb7058caca5e668f81a12a20a2195ce97a925f1dba3e7449a56f82201ec6073",
                "11ac2696b1ab5ea2352df1423bde7bd4bb78c9aed1a853c78672f9eb23bbe194"
            )
        );
        assert_eq!(
            hash_hex(HashAlgorithm::TupleHash128, b"abc").unwrap(),
            "873195cadfea6bc6a71cdd903da87afb49fd232d71db817c3abcad48ad8a7898"
        );
        assert_ne!(tuple_hash::<168>(&[b""], 32), tuple_hash::<168>(&[], 32));
        assert_ne!(
            tuple_hash::<168>(&[b"ab", b"c"], 32),
            tuple_hash::<168>(&[b"abc"], 32)
        );
    }

    #[test]
    fn new_hashes_have_expected_lengths_and_obey_input_limit() {
        use HashAlgorithm::*;
        for (algorithm, bytes) in [
            (Skein256_128, 16),
            (Skein256_160, 20),
            (Skein256_224, 28),
            (Skein256_256, 32),
            (Skein512_128, 16),
            (Skein512_160, 20),
            (Skein512_224, 28),
            (Skein512_256, 32),
            (Skein512_384, 48),
            (Skein512_512, 64),
            (Skein1024_384, 48),
            (Skein1024_512, 64),
            (Skein1024_1024, 128),
            (TupleHash128, 32),
            (TupleHash256, 64),
        ] {
            assert_eq!(
                hash_hex(algorithm, b"abc").unwrap().len(),
                bytes * 2,
                "{algorithm:?}"
            );
            assert!(
                transform(
                    &vec![0; MAX_INPUT_BYTES + 1],
                    None,
                    Transform::Hash(algorithm)
                )
                .is_err()
            );
        }
    }

    fn decode(codec: Codec, input: &[u8]) -> Vec<u8> {
        apply(Transform::Decode(codec), input).expect("decode should succeed")
    }

    fn encode(codec: Codec, input: &[u8]) -> Vec<u8> {
        apply(Transform::Encode(codec), input).expect("encode should succeed")
    }

    #[test]
    fn url_round_trips_all_bytes() {
        let encoded = encode(Codec::Url, b"raj chandel");
        assert_eq!(encoded, b"%72%61%6a%20%63%68%61%6e%64%65%6c");
        assert_eq!(decode(Codec::Url, &encoded), b"raj chandel");
        assert_eq!(decode(Codec::Url, b"100%"), b"100%");
        assert_eq!(decode(Codec::Url, b"%zz"), b"%zz");
    }

    #[test]
    fn html_encodes_every_character_and_decodes_entities() {
        let encoded = encode(Codec::Html, b"a & <b>");
        assert_eq!(encoded, b"&#x61;&#x20;&#x26;&#x20;&#x3c;&#x62;&#x3e;");
        assert_eq!(decode(Codec::Html, b"a &amp; &#x3c;b&#x3e;"), b"a & <b>");
    }

    #[test]
    fn base64_supports_standard_and_url_safe_alphabet() {
        let encoded = encode(Codec::Base64, b"hello world");
        assert_eq!(encoded, b"aGVsbG8gd29ybGQ=");
        assert_eq!(decode(Codec::Base64, &encoded), b"hello world");
        assert_eq!(decode(Codec::Base64, b"aGVsbG8gd29ybGQ"), b"hello world");
        assert_eq!(decode(Codec::Base64, b"SGVsbG8td29ybGQ_"), b"Hello-world?");
    }

    #[test]
    fn ascii_hex_is_continuous_and_whitespace_tolerant() {
        let encoded = encode(Codec::AsciiHex, b"raj chandel");
        assert_eq!(encoded, b"72616a206368616e64656c");
        assert_eq!(decode(Codec::AsciiHex, &encoded), b"raj chandel");
        assert_eq!(decode(Codec::AsciiHex, b"72 61 6a"), b"raj");
        assert!(apply(Transform::Decode(Codec::AsciiHex), b"abc").is_err());
    }

    #[test]
    fn numeric_encoders_match_burps_digit_run_behavior() {
        assert_eq!(
            encode(Codec::Hex, b"raj chandel 123456789"),
            b"raj chandel 75bcd15"
        );
        assert_eq!(
            decode(Codec::Hex, b"raj chandel 75bcd15"),
            b"r10j 12h10n222l 123456789"
        );
        assert_eq!(
            encode(Codec::Octal, b"raj chandel 123456789"),
            b"raj chandel 726746425"
        );
        assert_eq!(
            decode(Codec::Octal, b"raj chandel 726746425"),
            b"raj chandel 123456789"
        );
        assert_eq!(
            encode(Codec::Binary, b"raj chandel 123456789"),
            b"raj chandel 111010110111100110100010101"
        );
        assert_eq!(
            decode(Codec::Binary, b"raj chandel 111010110111100110100010101"),
            b"raj chandel 123456789"
        );
    }

    #[test]
    fn numeric_encoders_do_not_overflow_on_large_values() {
        let input = b"9999999999999999999999999999999999999999";
        let encoded = encode(Codec::Hex, input);
        assert!(!encoded.is_empty());
        assert_eq!(decode(Codec::Hex, &encoded), input);
    }

    #[test]
    fn gzip_round_trips_binary_and_text() {
        let payload = b"<!DOCTYPE html><html><body>Hello, Perseid</body></html>";
        let encoded = encode(Codec::Gzip, payload);
        assert_eq!(&encoded[..2], &[0x1f, 0x8b]);
        assert_eq!(decode(Codec::Gzip, &encoded), payload);
        assert!(apply(Transform::Decode(Codec::Gzip), b"not gzip").is_err());
    }

    #[test]
    fn hashes_match_known_vectors() {
        assert_eq!(
            hash_hex(HashAlgorithm::Md5, b"abc").unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            hash_hex(HashAlgorithm::Sha1, b"abc").unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hash_hex(HashAlgorithm::Sha256, b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_hex(HashAlgorithm::Sha3_256, b"abc").unwrap(),
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532"
        );
        assert_eq!(
            hash_hex(HashAlgorithm::Blake3, b"abc").unwrap(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
        assert_eq!(
            hash_hex(HashAlgorithm::Blake2b512, b"abc").unwrap(),
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
        assert_eq!(
            hash_hex(HashAlgorithm::Blake2s256, b"abc").unwrap(),
            "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982"
        );
        assert_eq!(
            hash_hex(HashAlgorithm::Keccak256, b"abc").unwrap(),
            "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"
        );
    }

    #[test]
    fn blake2_variable_lengths_match_known_vectors() {
        let vectors = [
            (
                HashAlgorithm::Blake2b160,
                "384264f676f39536840523f284921cdc68b6846b",
            ),
            (
                HashAlgorithm::Blake2b256,
                "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319",
            ),
            (
                HashAlgorithm::Blake2b384,
                "6f56a82c8e7ef526dfe182eb5212f7db9df1317e57815dbda46083fc30f54ee6c66ba83be64b302d7cba6ce15bb556f4",
            ),
            (
                HashAlgorithm::Blake2s128,
                "aa4938119b1dc7b87cbad0ffd200d0ae",
            ),
            (
                HashAlgorithm::Blake2s160,
                "5ae3b99be29b01834c3b508521ede60438f8de17",
            ),
            (
                HashAlgorithm::Blake2s224,
                "0b033fc226df7abde29f67a05d3dc62cf271ef3dfea4d387407fbd55",
            ),
        ];

        for (algorithm, expected) in vectors {
            assert_eq!(hash_hex(algorithm, b"abc").unwrap(), expected);
        }
    }

    #[test]
    fn transform_colors_only_the_selection() {
        let transformed =
            transform(b"hello world", Some(6..11), Transform::Encode(Codec::Url)).unwrap();
        assert_eq!(transformed.data, b"hello %77%6f%72%6c%64");
        assert_eq!(transformed.spans.len(), 1);
        assert_eq!(transformed.spans[0].start, 6);
        assert_eq!(transformed.spans[0].end, transformed.data.len());
        assert_eq!(transformed.spans[0].tag, ColorTag::Codec(Codec::Url));
        assert!(transformed.tag_at(0).is_none());
        assert_eq!(transformed.tag_at(6), Some(ColorTag::Codec(Codec::Url)));
    }

    #[test]
    fn smart_decode_layers_url_and_html_without_number_false_positives() {
        let steps = smart_decode(b"%34%37");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].codec, Codec::Url);
        assert_eq!(steps[0].data, b"47");

        let steps = smart_decode(b"%253Cb%253E");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].codec, Codec::Url);
        assert_eq!(steps[1].codec, Codec::Url);
        assert_eq!(steps[1].data, b"<b>");

        let steps = smart_decode(b"&#x41;&#x42;");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].codec, Codec::Html);
        assert_eq!(steps[0].data, b"AB");
    }

    #[test]
    fn smart_decode_recognizes_base64_ascii_hex_and_gzip() {
        let base64 = encode(Codec::Base64, b"SGVsbG8=");
        let steps = smart_decode(&base64);
        assert_eq!(steps[0].codec, Codec::Base64);
        assert_eq!(steps[0].data, b"SGVsbG8=");
        assert_eq!(steps[1].codec, Codec::Base64);
        assert_eq!(steps[1].data, b"Hello");

        let steps = smart_decode(b"48656c6c6f");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].codec, Codec::AsciiHex);
        assert_eq!(steps[0].data, b"Hello");

        let gzip = encode(Codec::Gzip, b"compressed text");
        let steps = smart_decode(&gzip);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].codec, Codec::Gzip);
        assert_eq!(steps[0].data, b"compressed text");
    }

    #[test]
    fn smart_decode_leaves_plain_text_alone() {
        assert!(smart_decode(b"hello world").is_empty());
        assert!(smart_decode(b"123456789").is_empty());
        assert!(smart_decode(b"testtest").is_empty());
    }

    #[test]
    fn oversized_inputs_are_rejected() {
        let input = vec![b'a'; MAX_INPUT_BYTES + 1];
        assert!(apply(Transform::Encode(Codec::Base64), &input).is_err());
    }

    #[test]
    fn oversized_html_output_is_rejected() {
        let input = vec![b'a'; 3 * 1024 * 1024];
        let error = apply(Transform::Encode(Codec::Html), &input).unwrap_err();
        assert!(error.to_string().contains("output limit"));
    }
}
