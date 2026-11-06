//! Full-message replay drafts: captured bytes are editable, never hidden.

use crate::project::BodyWorkspace;
use crate::proxy::{body_text, split_raw_request};

/// Import old header-only workspaces and newly captured requests without using
/// display previews. Never discard conflicting user-entered body text.
pub(super) fn inline(
    body: Option<&BodyWorkspace>,
    raw: &mut String,
    escaped: &mut bool,
    incomplete: &mut bool,
    intruder: bool,
) -> bool {
    let Some(body) = body else { return true };
    *incomplete = body.truncated || !body.complete;
    if !split_raw_request(raw).1.is_empty() || !(raw.ends_with("\r\n\r\n") || raw.ends_with("\n\n"))
    {
        // Legacy conflicting drafts remain blocked, not silently overwritten.
        *incomplete = true;
        return false;
    }
    // Literal section signs must not become payload-position markers.
    *escaped = std::str::from_utf8(&body.data).is_err()
        || (intruder && body.data.windows(2).any(|pair| pair == "§".as_bytes()));
    if *escaped {
        raw.push_str(&body_text::encode(&body.data, intruder));
    } else {
        raw.push_str(std::str::from_utf8(&body.data).expect("validated body"));
    }
    true
}

pub(super) fn bytes(raw: &str, escaped: bool, incomplete: bool) -> Result<Vec<u8>, String> {
    if incomplete {
        return Err("Captured body is incomplete. Edit or replace the body in the request editor before sending.".into());
    }
    body_text::decode(split_raw_request(raw).1, escaped).map(|bytes| bytes.into_owned())
}

/// Only a body edit accepts responsibility for a truncated capture. Editing
/// the target or headers must never unblock sending an incomplete body.
pub(super) fn edited(before: &str, after: &str, incomplete: &mut bool) {
    if split_raw_request(before).1 != split_raw_request(after).1 {
        *incomplete = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::CapturedBody;

    #[test]
    fn all_captured_bytes_are_visible_editable_and_lossless() {
        for data in [
            vec![],
            b"a=1\r\n".to_vec(),
            (0..=255).collect(),
            "中文\\xFF$$§§".as_bytes().to_vec(),
        ] {
            for intruder in [false, true] {
                let body = BodyWorkspace::from_body(&CapturedBody::from_bytes(data.clone()));
                let mut raw = "POST / HTTP/1.1\r\nContent-Encoding: gzip\r\n\r\n".to_owned();
                let (mut escaped, mut incomplete) = (false, false);
                inline(
                    Some(&body),
                    &mut raw,
                    &mut escaped,
                    &mut incomplete,
                    intruder,
                );
                assert_eq!(bytes(&raw, escaped, incomplete).unwrap(), data);
                assert!(raw.contains("Content-Encoding: gzip"));
                let before = raw.clone();
                raw.push_str("edited");
                edited(&before, &raw, &mut incomplete);
                assert_eq!(
                    bytes(&raw, escaped, incomplete).unwrap(),
                    [data.as_slice(), b"edited"].concat()
                );
            }
        }
    }

    #[test]
    fn incomplete_bytes_are_visible_but_only_body_edits_unblock_sending() {
        let body = BodyWorkspace {
            data: b"prefix".to_vec(),
            total_len: 100,
            truncated: true,
            complete: false,
        };
        let mut raw = "POST / HTTP/1.1\r\n\r\n".to_owned();
        let (mut escaped, mut incomplete) = (false, false);
        inline(Some(&body), &mut raw, &mut escaped, &mut incomplete, false);
        assert!(raw.ends_with("prefix"));
        assert!(bytes(&raw, escaped, incomplete).is_err());
        let changed_head = raw.replace("POST /", "POST /edited");
        edited(&raw, &changed_head, &mut incomplete);
        assert!(incomplete);
        let replacement = changed_head.replace("prefix", "replacement");
        edited(&changed_head, &replacement, &mut incomplete);
        assert_eq!(
            bytes(&replacement, escaped, incomplete).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn conflicting_legacy_edits_are_not_discarded_or_sent() {
        for draft in ["POST / HTTP/1.1\r\n\r\nreplacement", "POST / HTTP/1.1"] {
            let body = BodyWorkspace::from_body(&CapturedBody::from_bytes(vec![255]));
            let mut raw = draft.to_owned();
            let (mut escaped, mut incomplete) = (false, false);
            inline(Some(&body), &mut raw, &mut escaped, &mut incomplete, false);
            assert_eq!(raw, draft);
            assert!(bytes(&raw, escaped, incomplete).is_err());
        }
    }
}
