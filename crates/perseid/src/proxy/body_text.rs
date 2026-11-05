//! Lossless text representation for byte-oriented replay drafts.
//! Ordinary UTF-8 stays readable; invalid bytes use `\xNN`, and backslashes
//! are escaped so literal text can never be confused with byte escapes.

use std::borrow::Cow;

pub(crate) fn encode(data: &[u8], escape_dollars: bool) -> String {
    let mut out = String::new();
    let mut rest = data;
    while !rest.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(rest) {
            Ok(_) => (rest.len(), 0),
            Err(error) => (
                error.valid_up_to(),
                error
                    .error_len()
                    .unwrap_or(rest.len() - error.valid_up_to()),
            ),
        };
        for ch in std::str::from_utf8(&rest[..valid])
            .expect("validated prefix")
            .chars()
        {
            match ch {
                '\\' => out.push_str("\\\\"),
                '$' if escape_dollars => out.push_str("\\x24"),
                _ => out.push(ch),
            }
        }
        for byte in &rest[valid..valid + invalid] {
            use std::fmt::Write;
            write!(out, "\\x{byte:02X}").expect("String write");
        }
        rest = &rest[valid + invalid..];
    }
    out
}

pub(crate) fn decode(text: &str, escaped: bool) -> Result<Cow<'_, [u8]>, String> {
    if !escaped {
        return Ok(Cow::Borrowed(text.as_bytes()));
    }
    let mut out = Vec::with_capacity(text.len());
    let mut cursor = 0;
    let bytes = text.as_bytes();
    while cursor < bytes.len() {
        if bytes[cursor] != b'\\' {
            out.push(bytes[cursor]);
            cursor += 1;
        } else if bytes.get(cursor + 1) == Some(&b'\\') {
            out.push(b'\\');
            cursor += 2;
        } else if bytes.get(cursor + 1) == Some(&b'x')
            && let Some(pair) = bytes.get(cursor + 2..cursor + 4)
            && let (Some(high), Some(low)) = (
                (pair[0] as char).to_digit(16),
                (pair[1] as char).to_digit(16),
            )
        {
            out.push((high * 16 + low) as u8);
            cursor += 4;
        } else {
            return Err("Invalid body byte escape: use \\xNN or \\\\".into());
        }
    }
    Ok(Cow::Owned(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_and_literal_escape_round_trip() {
        for data in [
            (0..=255).collect::<Vec<u8>>(),
            "中文\\xFF$$\r\n".as_bytes().to_vec(),
        ] {
            for dollars in [false, true] {
                let text = encode(&data, dollars);
                assert_eq!(decode(&text, true).unwrap().as_ref(), data);
                if dollars {
                    assert!(!text.contains("$$"));
                }
            }
        }
    }

    #[test]
    fn invalid_or_partial_escapes_are_not_sent_literally() {
        for text in ["\\", "\\x", "\\x0", "\\xGG", "\\n"] {
            assert!(decode(text, true).is_err());
            assert_eq!(decode(text, false).unwrap().as_ref(), text.as_bytes());
        }
    }
}
