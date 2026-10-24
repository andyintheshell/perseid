//! Token parsing, encoding detection, and decoding for Sequencer samples.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD_PAD_INDIFFERENT, URL_SAFE_PAD_INDIFFERENT};

use super::{MAX_TOKEN_CHARS, MAX_TOKENS};

/// How the raw token text should be interpreted for analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TokenFormat {
    /// Detect hexadecimal first, then Base64, then fall back to text.
    #[default]
    Auto,
    /// Hexadecimal bytes, with optional `0x` prefixes.
    Hex,
    /// Standard or URL-safe Base64, with or without padding.
    Base64,
    /// The token's UTF-8 bytes.
    Text,
}

impl TokenFormat {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Hex, Self::Base64, Self::Text];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto detect",
            Self::Hex => "Hex",
            Self::Base64 => "Base64",
            Self::Text => "Text",
        }
    }
}

/// Split a manual-load buffer into tokens.
///
/// One token per line is the canonical form. A single line containing commas
/// or semicolons is split as a convenience, and surrounding whitespace is
/// ignored. Text tokens may contain spaces when they occupy their own line.
pub fn parse_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let comma_separated =
            !line.chars().any(char::is_whitespace) && (line.contains(',') || line.contains(';'));
        if comma_separated {
            for part in line.split([',', ';']) {
                push_token(&mut tokens, part);
            }
        } else {
            push_token(&mut tokens, line);
        }
        if tokens.len() >= MAX_TOKENS {
            break;
        }
    }
    tokens
}

fn push_token(tokens: &mut Vec<String>, token: &str) {
    if tokens.len() >= MAX_TOKENS {
        return;
    }
    let token = token.trim();
    if token.is_empty() || token.chars().count() > MAX_TOKEN_CHARS {
        return;
    }
    tokens.push(token.to_string());
}

/// Extract every regex match from a response body or head.
pub fn extract_tokens(text: &str, regex: &regex::Regex) -> Vec<String> {
    let mut tokens = Vec::new();
    for found in regex.find_iter(text) {
        let token = found.as_str().trim();
        if token.is_empty() || token.chars().count() > MAX_TOKEN_CHARS {
            continue;
        }
        tokens.push(token.to_string());
        if tokens.len() >= MAX_TOKENS {
            break;
        }
    }
    tokens
}

/// Detect the most likely representation for a token set.
pub fn detect_format(tokens: &[String]) -> TokenFormat {
    if !tokens.is_empty() && tokens.iter().all(|token| is_hex_token(token)) {
        return TokenFormat::Hex;
    }
    if !tokens.is_empty()
        && tokens
            .iter()
            .all(|token| decode_base64(token).is_some_and(|decoded| !decoded.is_empty()))
    {
        return TokenFormat::Base64;
    }
    TokenFormat::Text
}

/// Decode every token into the bytes used by the bit-level tests.
pub fn decode_tokens(tokens: &[String], format: TokenFormat) -> Result<Vec<Vec<u8>>, String> {
    if tokens.is_empty() {
        return Err("load at least two tokens before analyzing".to_string());
    }
    let format = if format == TokenFormat::Auto {
        detect_format(tokens)
    } else {
        format
    };
    let mut decoded = Vec::with_capacity(tokens.len());
    for (index, token) in tokens.iter().enumerate() {
        let bytes = match format {
            TokenFormat::Hex => decode_hex(token)
                .ok_or_else(|| format!("token {} is not valid hexadecimal", index + 1))?,
            TokenFormat::Base64 => decode_base64(token)
                .ok_or_else(|| format!("token {} is not valid Base64", index + 1))?,
            TokenFormat::Text => token.as_bytes().to_vec(),
            TokenFormat::Auto => unreachable!("auto is resolved above"),
        };
        if bytes.is_empty() {
            return Err(format!("token {} decodes to no bytes", index + 1));
        }
        decoded.push(bytes);
    }
    Ok(decoded)
}

fn is_hex_token(token: &str) -> bool {
    let digits = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
        .unwrap_or(token);
    digits.len() >= 2
        && digits.len().is_multiple_of(2)
        && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decode_hex(token: &str) -> Option<Vec<u8>> {
    let digits = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
        .unwrap_or(token);
    if digits.len() < 2 || !digits.len().is_multiple_of(2) {
        return None;
    }
    let bytes = digits.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        decoded.push(((high << 4) | low) as u8);
    }
    Some(decoded)
}

fn decode_base64(token: &str) -> Option<Vec<u8>> {
    STANDARD_PAD_INDIFFERENT
        .decode(token)
        .or_else(|_| URL_SAFE_PAD_INDIFFERENT.decode(token))
        .ok()
}

/// Number of Unicode scalar values in a token, used for the length summary.
pub fn token_length(token: &str) -> usize {
    token.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines_and_comma_lists() {
        let tokens = parse_tokens("  one  \n\ntwo\r\nthree,four;five\n");
        assert_eq!(tokens, ["one", "two", "three", "four", "five"]);
        assert!(parse_tokens("\n \r\n").is_empty());
    }

    #[test]
    fn detects_hex_before_base64() {
        let tokens = vec!["deadbeef".to_string(), "00112233".to_string()];
        assert_eq!(detect_format(&tokens), TokenFormat::Hex);
        let tokens = vec!["SGVsbG8=".to_string(), "d29ybGQ=".to_string()];
        assert_eq!(detect_format(&tokens), TokenFormat::Base64);
        let tokens = vec!["hello".to_string(), "world".to_string()];
        assert_eq!(detect_format(&tokens), TokenFormat::Text);
    }

    #[test]
    fn decodes_hex_base64_and_text() {
        let hex = vec!["0xdead".to_string(), "beef".to_string()];
        assert_eq!(
            decode_tokens(&hex, TokenFormat::Hex).unwrap(),
            [vec![0xde, 0xad], vec![0xbe, 0xef]]
        );
        let base64 = vec!["SGVsbG8=".to_string(), "d29ybGQ=".to_string()];
        assert_eq!(
            decode_tokens(&base64, TokenFormat::Base64).unwrap(),
            [b"Hello".to_vec(), b"world".to_vec()]
        );
        let text = vec!["hello".to_string()];
        assert_eq!(
            decode_tokens(&text, TokenFormat::Text).unwrap(),
            [b"hello".to_vec()]
        );
    }

    #[test]
    fn reports_invalid_explicit_encodings() {
        let tokens = vec!["not-hex".to_string()];
        let error = decode_tokens(&tokens, TokenFormat::Hex).unwrap_err();
        assert!(error.contains("token 1"));
        let error = decode_tokens(&tokens, TokenFormat::Base64).unwrap_err();
        assert!(error.contains("token 1"));
    }

    #[test]
    fn auto_detection_round_trips_url_safe_base64() {
        let tokens = vec![
            "aGVsbG8td29ybGQ_".to_string(),
            "YW5vdGhlci10b2tlbg".to_string(),
        ];
        assert_eq!(detect_format(&tokens), TokenFormat::Base64);
        let decoded = decode_tokens(&tokens, TokenFormat::Auto).unwrap();
        assert_eq!(decoded[0], b"hello-world?");
    }

    #[test]
    fn extracts_regex_matches_and_skips_empty_matches() {
        let regex = regex::Regex::new(r"[0-9a-f]{8}").unwrap();
        let tokens = extract_tokens("a deadbeef b 00112233 c", &regex);
        assert_eq!(tokens, ["deadbeef", "00112233"]);
    }

    #[test]
    fn token_length_counts_characters() {
        assert_eq!(token_length("abc"), 3);
        assert_eq!(token_length("é"), 1);
    }
}
