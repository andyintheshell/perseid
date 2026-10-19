//! Bounded content-encoding decoding for the message views.
//!
//! Wiremoth forwards encoded bodies untouched. For inspection, Raw needs to
//! show what the browser would consume, so gzip, deflate, and Brotli bodies are
//! decoded into a bounded preview. The Hex view still exposes the wire bytes.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Read};

use brotli_decompressor::Decompressor;
use flate2::read::{DeflateDecoder, MultiGzDecoder, ZlibDecoder};

/// A decoded body preview, bounded by the caller's output limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedBody {
    pub bytes: Vec<u8>,
    /// The encoding names as they appeared in the header.
    pub encoding: String,
    /// The decoded preview hit the caller's output limit.
    pub truncated: bool,
    /// The decoder stopped before the stream ended, usually because the
    /// captured compressed prefix was truncated.
    pub incomplete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Unsupported(String),
    Invalid { encoding: String, detail: String },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(encoding) => {
                write!(f, "unsupported content-encoding `{encoding}`")
            }
            Self::Invalid { encoding, detail } => {
                write!(
                    f,
                    "could not decode content-encoding `{encoding}`: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decode a `Content-Encoding` body into at most `limit` bytes.
///
/// Returns `Ok(None)` when the header requests no transformation. Unsupported
/// encodings and malformed data are reported so callers can fall back to the
/// wire bytes instead of hiding the body.
pub fn decode_body(
    bytes: &[u8],
    content_encoding: &str,
    limit: usize,
) -> Result<Option<DecodedBody>, DecodeError> {
    let names: Vec<&str> = content_encoding
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return Ok(None);
    }

    let mut encodings = Vec::with_capacity(names.len());
    for name in &names {
        if name.eq_ignore_ascii_case("identity") {
            continue;
        }
        let encoding =
            Encoding::parse(name).ok_or_else(|| DecodeError::Unsupported((*name).to_string()))?;
        encodings.push(encoding);
    }
    if encodings.is_empty() {
        return Ok(None);
    }

    let mut current = Cow::Borrowed(bytes);
    let mut truncated = false;
    let mut incomplete = false;
    // Content-Encoding lists transformations in the order they were applied.
    for encoding in encodings.iter().rev() {
        let stage = decode_stage(current.as_ref(), *encoding, limit)?;
        current = Cow::Owned(stage.bytes);
        truncated |= stage.truncated;
        incomplete |= stage.incomplete;
        if truncated {
            // The next decoder would receive a partial stream, so keep the
            // visible prefix we already have instead of producing garbage.
            break;
        }
    }

    Ok(Some(DecodedBody {
        bytes: current.into_owned(),
        encoding: names.join(", "),
        truncated,
        incomplete,
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Gzip,
    Deflate,
    Brotli,
}

impl Encoding {
    fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
            Some(Self::Gzip)
        } else if name.eq_ignore_ascii_case("deflate") {
            Some(Self::Deflate)
        } else if name.eq_ignore_ascii_case("br") {
            Some(Self::Brotli)
        } else {
            None
        }
    }
}

struct Stage {
    bytes: Vec<u8>,
    truncated: bool,
    incomplete: bool,
}

fn decode_stage(input: &[u8], encoding: Encoding, limit: usize) -> Result<Stage, DecodeError> {
    match encoding {
        Encoding::Gzip => collect(MultiGzDecoder::new(input), encoding, limit),
        Encoding::Brotli => collect(Decompressor::new(input, 4096), encoding, limit),
        Encoding::Deflate => {
            // RFC 9110's deflate is zlib-wrapped, but some servers send a raw
            // deflate stream. Try the standard form first, then the raw form.
            match collect(ZlibDecoder::new(input), encoding, limit) {
                Ok(stage) => Ok(stage),
                Err(zlib_error) => match collect(DeflateDecoder::new(input), encoding, limit) {
                    Ok(stage) => Ok(stage),
                    Err(_) => Err(zlib_error),
                },
            }
        }
    }
}

fn collect(mut decoder: impl Read, encoding: Encoding, limit: usize) -> Result<Stage, DecodeError> {
    // Keep one byte of headroom so we can tell an exactly-full preview from a
    // genuinely truncated one.
    let cap = limit.saturating_add(1);
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0u8; 16 * 1024];
    let mut incomplete = false;

    loop {
        let remaining = cap.saturating_sub(bytes.len());
        if remaining == 0 {
            break;
        }
        let wanted = remaining.min(chunk.len());
        match decoder.read(&mut chunk[..wanted]) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                if bytes.is_empty() {
                    return Err(DecodeError::Invalid {
                        encoding: encoding.name().to_string(),
                        detail: err.to_string(),
                    });
                }
                incomplete = true;
                break;
            }
        }
    }

    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    Ok(Stage {
        bytes,
        truncated,
        incomplete,
    })
}

impl Encoding {
    fn name(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Brotli => "br",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &[u8] = b"<!DOCTYPE html><html><body><h1>Hello, Wiremoth</h1></body></html>";
    const GZIP: &[u8] = &[
        31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 179, 81, 116, 241, 119, 14, 137, 12, 112, 85, 200, 40,
        201, 205, 177, 179, 129, 144, 73, 249, 41, 149, 64, 182, 161, 157, 71, 106, 78, 78, 190,
        142, 66, 120, 102, 81, 106, 110, 126, 73, 134, 141, 62, 80, 204, 70, 31, 34, 173, 15, 86,
        11, 0, 140, 83, 160, 197, 65, 0, 0, 0,
    ];
    const ZLIB: &[u8] = &[
        120, 156, 179, 81, 116, 241, 119, 14, 137, 12, 112, 85, 200, 40, 201, 205, 177, 179, 129,
        144, 73, 249, 41, 149, 64, 182, 161, 157, 71, 106, 78, 78, 190, 142, 66, 120, 102, 81, 106,
        110, 126, 73, 134, 141, 62, 80, 204, 70, 31, 34, 173, 15, 86, 11, 0, 183, 229, 21, 121,
    ];
    const RAW_DEFLATE: &[u8] = &[
        179, 81, 116, 241, 119, 14, 137, 12, 112, 85, 200, 40, 201, 205, 177, 179, 129, 144, 73,
        249, 41, 149, 64, 182, 161, 157, 71, 106, 78, 78, 190, 142, 66, 120, 102, 81, 106, 110,
        126, 73, 134, 141, 62, 80, 204, 70, 31, 34, 173, 15, 86, 11, 0,
    ];
    const BROTLI: &[u8] = &[
        27, 64, 0, 232, 141, 211, 29, 243, 175, 107, 214, 8, 149, 158, 62, 104, 111, 29, 195, 211,
        70, 7, 54, 224, 128, 61, 78, 44, 178, 15, 171, 182, 249, 57, 2, 115, 129, 194, 100, 152,
        173, 226, 181, 250, 90, 13, 47,
    ];
    const GZIP_IN_BROTLI: &[u8] = &[
        11, 35, 128, 31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 179, 81, 116, 241, 119, 14, 137, 12, 112,
        85, 200, 40, 201, 205, 177, 179, 129, 144, 73, 249, 41, 149, 64, 182, 161, 157, 71, 106,
        78, 78, 190, 142, 66, 120, 102, 81, 106, 110, 126, 73, 134, 141, 62, 80, 204, 70, 31, 34,
        173, 15, 86, 11, 0, 140, 83, 160, 197, 65, 0, 0, 0, 3,
    ];

    #[test]
    fn decodes_gzip_zlib_and_brotli() {
        for (name, encoded) in [("gzip", GZIP), ("deflate", ZLIB), ("br", BROTLI)] {
            let decoded = decode_body(encoded, name, 64 * 1024)
                .unwrap()
                .expect("encoding should decode");
            assert_eq!(decoded.bytes, HTML, "{name}");
            assert_eq!(decoded.encoding, name);
            assert!(!decoded.truncated);
            assert!(!decoded.incomplete);
        }
    }

    #[test]
    fn decodes_raw_deflate_as_a_fallback() {
        let decoded = decode_body(RAW_DEFLATE, "deflate", 64 * 1024)
            .unwrap()
            .expect("raw deflate should decode");
        assert_eq!(decoded.bytes, HTML);
    }

    #[test]
    fn applies_multiple_encodings_in_reverse_order() {
        let decoded = decode_body(GZIP_IN_BROTLI, "gzip, br", 64 * 1024)
            .unwrap()
            .expect("stacked encodings should decode");
        assert_eq!(decoded.bytes, HTML);
        assert_eq!(decoded.encoding, "gzip, br");
    }

    #[test]
    fn bounds_the_decoded_output() {
        let decoded = decode_body(GZIP, "gzip", 10)
            .unwrap()
            .expect("gzip should decode");
        assert_eq!(decoded.bytes, &HTML[..10]);
        assert!(decoded.truncated);
    }

    #[test]
    fn reports_unsupported_and_invalid_encodings() {
        assert_eq!(
            decode_body(b"data", "zstd", 1024).unwrap_err(),
            DecodeError::Unsupported("zstd".to_string())
        );
        assert!(matches!(
            decode_body(b"not brotli", "br", 1024).unwrap_err(),
            DecodeError::Invalid { .. }
        ));
    }
}
