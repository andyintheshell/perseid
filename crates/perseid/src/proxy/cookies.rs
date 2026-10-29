//! In-memory cookie state shared by direct replay tools.
//!
//! Values must never be logged or serialized. The store is deliberately not
//! connected to project persistence.

use std::sync::{Arc, Mutex};

use cookie_store::{Cookie, CookieStore};
use url::Url;

use super::events::HeaderList;

pub struct CookieJar {
    store: CookieStore,
    pub apply: bool,
    pub update: bool,
    pub update_proxy: bool,
}

impl Default for CookieJar {
    fn default() -> Self {
        Self {
            store: CookieStore::default(),
            apply: false,
            update: false,
            update_proxy: true,
        }
    }
}

pub type SharedCookieJar = Arc<Mutex<CookieJar>>;

impl CookieJar {
    /// Observe browser cookies. Request headers lack scope attributes, so new
    /// cookies are host-only and use the root path. Known matching cookies keep
    /// their existing scope and flags when their value changes.
    pub fn observe_proxy_request(&mut self, target: &str, headers: &HeaderList) {
        if !self.update_proxy {
            return;
        }
        let Ok(url) = Url::parse(target) else { return };
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return;
        }
        for (_, header) in headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("cookie"))
        {
            for pair in header.split(';').map(str::trim) {
                let Some((name, value)) = pair.split_once('=') else {
                    continue;
                };
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                let mut matches = self.store.matches(&url);
                matches.retain(|c| {
                    c.name() == name && (url.scheme() == "https" || !c.secure().unwrap_or(false))
                });
                matches.sort_by_key(|c| std::cmp::Reverse(String::from(&c.path).len()));
                if matches.first().is_some_and(|c| c.value() == value) {
                    continue;
                }
                let replacement = matches.first().map(|c| {
                    let mut raw = (***c).clone();
                    raw.set_value(value.to_owned());
                    raw.set_path(String::from(&c.path));
                    format!("{raw:#}")
                });
                let header = replacement.unwrap_or_else(|| format!("{name}={value}; Path=/"));
                let _ = self.insert(target, &header);
            }
        }
    }

    pub fn observe_proxy_response(&mut self, target: &str, headers: &HeaderList) {
        if !self.update_proxy {
            return;
        }
        for (_, value) in headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        {
            let _ = self.insert(target, value);
        }
    }
    /// Insert one Set-Cookie value relative to its response URL.
    pub fn insert(&mut self, target: &str, header: &str) -> Result<(), &'static str> {
        if header.len() > 4096 {
            return Err("Cookie exceeds 4096 bytes");
        }
        let url = Url::parse(target).map_err(|_| "Invalid URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("URL must be HTTP or HTTPS with a host");
        }
        let cookie = Cookie::parse(header, &url).map_err(|_| "Invalid cookie")?;
        if cookie.secure().unwrap_or(false) && url.scheme() != "https" {
            return Err("Secure cookies require HTTPS");
        }
        let expired: Vec<_> = self
            .store
            .iter_any()
            .filter(|c| c.is_expired())
            .map(|c| {
                (
                    String::from(&c.domain),
                    String::from(&c.path),
                    c.name().to_owned(),
                )
            })
            .collect();
        for (domain, path, name) in expired {
            self.store.remove(&domain, &path, &name);
        }
        if self.store.iter_any().count() >= 512
            && self
                .store
                .get(
                    &String::from(&cookie.domain),
                    &String::from(&cookie.path),
                    cookie.name(),
                )
                .is_none()
        {
            return Err("Cookie jar is full");
        }
        let domain = String::from(&cookie.domain);
        let path = String::from(&cookie.path);
        let used: usize = self
            .store
            .iter_unexpired()
            .filter(|c| {
                String::from(&c.domain) != domain
                    || String::from(&c.path) != path
                    || c.name() != cookie.name()
            })
            .map(|c| c.name().len() + c.value().len())
            .sum();
        if used
            .saturating_add(cookie.name().len())
            .saturating_add(cookie.value().len())
            > 32 * 1024
        {
            return Err("Cookie jar size limit reached");
        }
        // Reject public suffix domain cookies rather than allowing a response
        // from one registrable domain to inject cookies into another.
        if matches!(cookie.domain, cookie_store::CookieDomain::Suffix(_)) {
            let domain = String::from(&cookie.domain);
            if psl::suffix_str(&domain).is_some_and(|suffix| suffix == domain) {
                return Err("Cookie domain is a public suffix");
            }
        }
        self.store
            .insert(cookie.into_owned(), &url)
            .map_err(|_| "Cookie rejected")?;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.store.clear();
    }

    pub fn remove(&mut self, domain: &str, path: &str, name: &str) {
        self.store.remove(domain, path, name);
    }

    pub fn entries(&self) -> Vec<(String, String, String, String, String)> {
        let mut entries: Vec<_> = self
            .store
            .iter_unexpired()
            .map(|c| {
                (
                    String::from(&c.domain),
                    String::from(&c.path),
                    c.name().to_owned(),
                    c.value().to_owned(),
                    format!(
                        "{}{}{:?}",
                        if c.secure().unwrap_or(false) {
                            "Secure "
                        } else {
                            ""
                        },
                        if c.http_only().unwrap_or(false) {
                            "HttpOnly "
                        } else {
                            ""
                        },
                        c.expires
                    ),
                )
            })
            .collect();
        entries.sort();
        entries
    }

    pub fn expiration_label(&self, domain: &str, path: &str, name: &str) -> String {
        match self.store.get(domain, path, name).map(|c| &c.expires) {
            Some(cookie_store::CookieExpiration::AtUtc(date)) => date.date().to_string(),
            _ => "Session".to_owned(),
        }
    }

    pub fn edit_source(&self, domain: &str, path: &str, name: &str) -> Option<(String, String)> {
        let c = self.store.get(domain, path, name)?;
        let host = if domain.contains(':') && !domain.starts_with('[') {
            format!("[{domain}]")
        } else {
            domain.to_owned()
        };
        // A cookie with no explicit Path derives its path from the response URL.
        // Use a synthetic child URL so an unmodified edit keeps that path.
        let child = format!("{path}/edit");
        Some((format!("https://{host}{child}"), format!("{:#}", **c)))
    }

    /// Existing headers win for Intruder; for Repeater matching jar names
    /// replace captured names while unrelated manually supplied cookies remain.
    pub fn apply_to(&self, target: &str, headers: &mut HeaderList, preserve_existing: bool) {
        if !self.apply {
            return;
        }
        let Ok(url) = Url::parse(target) else { return };
        let mut matches = self.store.matches(&url);
        if url.scheme() != "https" {
            matches.retain(|c| !c.secure().unwrap_or(false));
        }
        matches.sort_by_key(|c| std::cmp::Reverse(String::from(&c.path).len()));
        let existing: Vec<String> = headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("cookie"))
            .flat_map(|(_, v)| {
                v.split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .collect();
        let names: Vec<&str> = matches.iter().map(|c| c.name()).collect();
        let mut values: Vec<String> = existing
            .iter()
            .filter(|pair| {
                let name = pair.split_once('=').map(|(n, _)| n.trim()).unwrap_or(pair);
                preserve_existing || !names.contains(&name)
            })
            .cloned()
            .collect();
        for cookie in matches {
            if preserve_existing
                && existing.iter().any(|p| {
                    p.split_once('=')
                        .is_some_and(|(n, _)| n.trim() == cookie.name())
                })
            {
                continue;
            }
            values.push(format!("{}={}", cookie.name(), cookie.value()));
        }
        if values.is_empty() {
            return;
        }
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case("cookie"));
        headers.push(("Cookie".into(), values.join("; ")));
    }

    pub fn update_from(&mut self, target: &str, headers: &HeaderList) {
        if !self.update {
            return;
        }
        for (_, value) in headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        {
            let _ = self.insert(target, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expiration_labels_are_readable() {
        let mut jar = CookieJar::default();
        jar.insert("https://example.test/", "session=1; Path=/")
            .unwrap();
        jar.insert(
            "https://example.test/",
            "persistent=1; Path=/; Expires=Wed, 01 Jan 2031 00:00:00 GMT",
        )
        .unwrap();
        assert_eq!(
            jar.expiration_label("example.test", "/", "session"),
            "Session"
        );
        assert_eq!(
            jar.expiration_label("example.test", "/", "persistent"),
            "2031-01-01"
        );
    }
    #[test]
    fn proxy_request_updates_known_cookie_and_does_not_broaden_scope() {
        let mut jar = CookieJar::default();
        assert!(jar.update_proxy);
        assert!(!jar.apply && !jar.update);
        jar.insert(
            "https://example.test/a/page",
            "sid=old; Path=/a; Secure; HttpOnly",
        )
        .unwrap();
        jar.observe_proxy_request(
            "https://example.test/a/page",
            &vec![("cookie".into(), "sid=new; other=value".into())],
        );
        let entries = jar.entries();
        assert!(entries.iter().any(|e| e.1 == "/a"
            && e.2 == "sid"
            && e.3 == "new"
            && e.4.contains("Secure")
            && e.4.contains("HttpOnly")));
        assert!(entries.iter().any(|e| e.1 == "/" && e.2 == "other"));
        assert!(!entries.iter().any(|e| e.1 == "/" && e.2 == "sid"));
    }
    #[test]
    fn matching_updates_and_precedence() {
        let mut jar = CookieJar {
            apply: true,
            update: true,
            ..Default::default()
        };
        jar.update_from(
            "https://app.example.com/a/x",
            &vec![(
                "Set-Cookie".into(),
                "sid=new; Domain=example.com; Path=/a; Secure".into(),
            )],
        );
        let mut h = vec![("Cookie".into(), "sid=old; theme=dark".into())];
        jar.apply_to("https://app.example.com/a/y", &mut h, false);
        assert_eq!(h[0].1, "theme=dark; sid=new");
        let mut h = vec![("Cookie".into(), "sid=payload".into())];
        jar.apply_to("https://app.example.com/a", &mut h, true);
        assert_eq!(h[0].1, "sid=payload");
        for url in [
            "http://app.example.com/a",
            "https://app.example.com/ab",
            "https://evil.com/a",
        ] {
            let mut h = vec![];
            jar.apply_to(url, &mut h, false);
            assert!(h.is_empty());
        }
        assert!(
            jar.insert("https://app.example.com/", "bad=x; Domain=com")
                .is_err()
        );
        jar.update_from(
            "https://app.example.com/a",
            &vec![(
                "set-cookie".into(),
                "sid=; Domain=example.com; Path=/a; Max-Age=0".into(),
            )],
        );
        assert!(jar.entries().is_empty());
    }

    #[test]
    fn host_only_defaults_secure_and_manual_edit() {
        let mut jar = CookieJar {
            apply: true,
            ..Default::default()
        };
        jar.insert(
            "https://example.test/dir/page",
            "one=1; HttpOnly; SameSite=Strict",
        )
        .unwrap();
        jar.insert("https://example.test/", "secret=s; Secure; Path=/")
            .unwrap();
        let mut other = vec![];
        jar.apply_to("https://sub.example.test/dir/", &mut other, false);
        assert!(other.is_empty());
        let mut http = vec![];
        jar.apply_to("http://example.test/dir/x", &mut http, false);
        assert_eq!(http[0].1, "one=1");
        assert!(jar.insert("http://localhost/", "x=y; Secure").is_err());
        let (url, text) = jar.edit_source("example.test", "/dir", "one").unwrap();
        jar.insert(&url, &text.replace("one=1", "one=2")).unwrap();
        let mut sent = vec![];
        jar.apply_to("https://example.test/dir/x", &mut sent, false);
        assert!(sent[0].1.contains("one=2"));
        assert_eq!(jar.entries().len(), 2);
        jar.remove("example.test", "/dir", "one");
        assert_eq!(jar.entries().len(), 1);
        jar.clear();
        assert!(jar.entries().is_empty());
    }

    #[test]
    fn disabled_options_leave_requests_and_responses_untouched() {
        let mut jar = CookieJar::default();
        jar.insert("https://example.test/", "a=b; Path=/").unwrap();
        let mut headers = vec![("Cookie".into(), "a=original".into())];
        jar.apply_to("https://example.test/", &mut headers, false);
        jar.update_from(
            "https://example.test/",
            &vec![("Set-Cookie".into(), "a=new".into())],
        );
        assert_eq!(headers[0].1, "a=original");
        assert_eq!(jar.entries()[0].3, "b");
    }
}
