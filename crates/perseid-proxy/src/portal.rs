//! Local certificate-download page.
//!
//! Browsers configured to use Perseid can open the proxy address directly and
//! download the CA certificate, similar to Burp's in-browser interface. The page
//! is also reachable when the browser bypasses the proxy for loopback: the
//! proxy recognises its own listener address as the portal instead of trying
//! to forward the request to itself.

use std::net::{IpAddr, SocketAddr};

use http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, HOST};
use http::{Method, Request, Response, StatusCode};
use hyper::body::Incoming;

use crate::server::Ctx;
use crate::util::{ProxyBody, text_body, text_response};

const CERT_PATH: &str = "/perseid-ca.pem";
const PORTAL_HOST: &str = "perseid";
const PORTAL_LOCAL_SUFFIX: &str = "perseid.localhost";

/// Whether this request is for the built-in certificate page.
pub(crate) fn matches(req: &Request<Incoming>, listen: SocketAddr) -> bool {
    if req.uri().scheme().is_some_and(|scheme| scheme != "http") {
        return false;
    }
    let Some(authority) = request_authority(req) else {
        return false;
    };
    let host = authority.host().to_ascii_lowercase();
    if host == PORTAL_HOST || host == PORTAL_LOCAL_SUFFIX {
        return true;
    }
    let port = authority.port_u16().unwrap_or(80);
    port == listen.port() && host_matches_listener(&host, listen.ip())
}

/// Serve the certificate page or the certificate itself.
pub(crate) fn handle(ctx: &Ctx, req: Request<Incoming>) -> Response<ProxyBody> {
    if req.method() != Method::GET {
        return text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "the certificate page is GET-only",
        );
    }

    match req.uri().path() {
        "/" => page(ctx),
        CERT_PATH => certificate(ctx),
        _ => text_response(StatusCode::NOT_FOUND, "no such page"),
    }
}

fn page(ctx: &Ctx) -> Response<ProxyBody> {
    let has_ca = ctx.tls_authority().is_some();
    let mut response = Response::new(text_body(page_html(has_ca, ctx.listen)));
    *response.status_mut() = if has_ca {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    response.headers_mut().insert(
        CONTENT_TYPE,
        http::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, http::HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        http::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        "content-security-policy",
        http::HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    response
}

fn certificate(ctx: &Ctx) -> Response<ProxyBody> {
    let Some(ca) = ctx.tls_authority() else {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no certificate authority is configured",
        );
    };
    let mut response = Response::new(text_body(ca.cert_pem().to_string()));
    response.headers_mut().insert(
        CONTENT_TYPE,
        http::HeaderValue::from_static("application/x-pem-file"),
    );
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        http::HeaderValue::from_static("attachment; filename=\"perseid-ca.pem\""),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, http::HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        http::HeaderValue::from_static("nosniff"),
    );
    response
}

fn request_authority(req: &Request<Incoming>) -> Option<http::uri::Authority> {
    if let Some(authority) = req.uri().authority() {
        return Some(authority.clone());
    }
    let host = req.headers().get(HOST)?.to_str().ok()?;
    host.parse().ok()
}

fn host_matches_listener(host: &str, listen_ip: IpAddr) -> bool {
    let host_ip = if host.eq_ignore_ascii_case("localhost") {
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    } else {
        let Ok(ip) = host.parse::<IpAddr>() else {
            return false;
        };
        ip
    };
    host_ip == listen_ip
        || (listen_ip.is_unspecified() && host_ip.is_loopback())
        || (listen_ip.is_loopback() && host_ip.is_loopback())
}

fn page_html(has_ca: bool, listen: SocketAddr) -> String {
    let proxy = display_listen(listen);
    if !has_ca {
        return r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Perseid certificate authority</title>
<style>
body{margin:0;background:#fbf8f4;color:#2e2a26;font:16px/1.55 system-ui,-apple-system,sans-serif}
main{max-width:720px;margin:0 auto;padding:48px 24px}h1{font-size:30px;line-height:1.15;margin:0 0 12px}
p{margin:0 0 14px}.card{background:#fffdfa;border:1px solid #e8ddd1;border-radius:10px;padding:24px}
.eyebrow{color:#c93a1c;font-size:13px;font-weight:700;letter-spacing:.12em;text-transform:uppercase}
code{background:#f8f1e9;border-radius:4px;padding:2px 5px;font-size:14px}
</style></head>
<body><main><div class="card">
<p class="eyebrow">Perseid</p>
<h1>Certificate authority unavailable</h1>
<p>Perseid could not load or create its local certificate authority. Open <strong>Proxy &gt; TLS</strong> in Perseid to see the error and retry.</p>
</div></main></body></html>"#
            .to_string();
    }

    r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Perseid certificate authority</title>
<style>
body{margin:0;background:#fbf8f4;color:#2e2a26;font:16px/1.55 system-ui,-apple-system,sans-serif}
main{max-width:760px;margin:0 auto;padding:48px 24px}
h1{font-size:30px;line-height:1.15;margin:0 0 12px}h2{font-size:18px;margin:28px 0 8px}
p{margin:0 0 14px}.card{background:#fffdfa;border:1px solid #e8ddd1;border-radius:10px;padding:28px}
.eyebrow{color:#c93a1c;font-size:13px;font-weight:700;letter-spacing:.12em;text-transform:uppercase}
.button{display:inline-block;background:#ff4d2b;color:#fff;text-decoration:none;font-weight:700;border-radius:6px;padding:10px 16px;margin:6px 0 14px}
.note{background:#f8f1e9;border-left:3px solid #ff4d2b;padding:12px 14px;border-radius:4px;color:#7c7065}
code{background:#f8f1e9;border-radius:4px;padding:2px 5px;font-size:14px}
ul{padding-left:22px}li{margin:4px 0}
</style></head>
<body><main><div class="card">
<p class="eyebrow">Perseid</p>
<h1>Trust Perseid's HTTPS certificate</h1>
<p>Perseid created a local certificate authority (CA) on first launch. Install its certificate in the browser or client you use for authorized testing. Perseid issues a separate certificate for each host from this CA.</p>
<p><a class="button" href="/perseid-ca.pem">Download Perseid CA certificate</a></p>
<p class="note">Trust this certificate only in a browser profile used for testing. Anyone with the private key can impersonate HTTPS sites to a client that trusts it.</p>
<h2>Firefox</h2>
<ol><li>Open <strong>Settings &gt; Privacy &amp; Security &gt; Certificates &gt; View Certificates</strong>.</li><li>On <strong>Authorities</strong>, click <strong>Import</strong> and choose the downloaded file.</li><li>Check <strong>Trust this CA to identify websites</strong>, then click <strong>OK</strong>.</li></ol>
<h2>Chrome or Chromium on Linux</h2>
<p>Import the downloaded certificate into the NSS database, then restart the browser:</p>
<p><code>certutil -d sql:"$HOME/.pki/nssdb" -A -t "C,," -n "Perseid CA" -i perseid-ca.pem</code></p>
<p>On macOS and Windows, Chrome uses the system certificate store. Import the certificate with the platform certificate manager.</p>
<h2>Point your browser at Perseid</h2>
<p>Set the HTTP and HTTPS proxy to <code>__PROXY__</code>, or the address shown in Perseid. HTTPS interception is on by default; decrypted requests then appear in <strong>Proxy &gt; HTTP history</strong>.</p>
<p>To remove trust, delete the Perseid authority from the browser's certificate manager and remove the CA files from <strong>Proxy &gt; TLS</strong>.</p>
</div></main></body></html>"#
        .replace("__PROXY__", &proxy)
}

fn display_listen(listen: SocketAddr) -> String {
    match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => format!("127.0.0.1:{}", listen.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => format!("[::1]:{}", listen.port()),
        IpAddr::V4(ip) => format!("{ip}:{}", listen.port()),
        IpAddr::V6(ip) => format!("[{ip}]:{}", listen.port()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_portal_hosts_and_the_listener_address() {
        let listen: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert!(host_matches_listener("127.0.0.1", listen.ip()));
        assert!(host_matches_listener("localhost", listen.ip()));
        assert!(host_matches_listener("::1", listen.ip()));
        assert!(!host_matches_listener("example.com", listen.ip()));

        let wildcard: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert!(host_matches_listener("127.0.0.1", wildcard.ip()));
        assert!(host_matches_listener("::1", wildcard.ip()));
        assert!(!host_matches_listener("192.0.2.1", wildcard.ip()));
    }

    #[test]
    fn portal_page_contains_the_download_link() {
        let listen: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let html = page_html(true, listen);
        assert!(html.contains("/perseid-ca.pem"));
        assert!(html.contains("Firefox"));
        assert!(html.contains("127.0.0.1:8080"));
        assert!(page_html(false, listen).contains("unavailable"));
    }
}
