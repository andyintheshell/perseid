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
use blake2::digest::VariableOutput;
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
/// This covers the common Java and BouncyCastle digests. The long tail of
/// provider-specific names (SKEIN, TupleHash, OID aliases) is intentionally
/// omitted; the docs list that limitation.
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
}

impl HashAlgorithm {
    pub const ALL: [Self; 31] = [
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

fn normalize_selection(length: usize, selection: Option<Range<usize>>) -> Range<usize> {
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
    for pair in compact.chunks_exact(2) {
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
        H::Blake2b160 => blake2b(input, 20)?,
        H::Blake2b256 => blake2b(input, 32)?,
        H::Blake2b384 => blake2b(input, 48)?,
        H::Blake2b512 => blake2b(input, 64)?,
        H::Blake2s128 => blake2s(input, 16)?,
        H::Blake2s160 => blake2s(input, 20)?,
        H::Blake2s224 => blake2s(input, 28)?,
        H::Blake2s256 => blake2s(input, 32)?,
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
    };
    Ok(bytes)
}

fn digest<D: digest::Digest + Default>(input: &[u8]) -> Vec<u8> {
    let mut hasher = D::new();
    hasher.update(input);
    hasher.finalize().to_vec()
}

fn blake2b(input: &[u8], length: usize) -> Result<Vec<u8>> {
    variable_digest(blake2::Blake2bVar::new(length), input)
}

fn blake2s(input: &[u8], length: usize) -> Result<Vec<u8>> {
    variable_digest(blake2::Blake2sVar::new(length), input)
}

fn variable_digest<D>(
    hasher: std::result::Result<D, blake2::digest::InvalidOutputSize>,
    input: &[u8],
) -> Result<Vec<u8>>
where
    D: VariableOutput + blake2::digest::Update,
{
    let mut hasher = hasher.map_err(|_| TransformError::new("invalid BLAKE2 output length"))?;
    hasher.update(input);
    let mut output = vec![0u8; hasher.output_size()];
    hasher
        .finalize_variable(&mut output)
        .map_err(|_| TransformError::new("could not finalize BLAKE2 digest"))?;
    Ok(output)
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

fn hex_value(byte: u8) -> Option<u8> {
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
