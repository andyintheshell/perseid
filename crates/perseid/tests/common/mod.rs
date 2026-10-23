//! Helpers shared by the end-to-end proxy and Repeater tests.

use rcgen::{CertificateParams, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Read a request head plus its declared body.
pub async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        if let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..head_end]);
            let content_length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + content_length {
                break;
            }
        }
    }
    buf
}

/// Self-signed certificate for `name`, as a server config plus its PEM.
pub fn origin_certificate(name: &str) -> (ServerConfig, String) {
    let key = KeyPair::generate().expect("origin key");
    let params = CertificateParams::new(vec![name.to_string()]).expect("origin params");
    let cert = params.self_signed(&key).expect("origin certificate");
    let key_der = PrivatePkcs8KeyDer::from(key.serialize_der());
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], PrivateKeyDer::Pkcs8(key_der))
        .expect("origin server config");
    (config, cert.pem())
}

/// `origin_certificate` advertising HTTP/2 through ALPN.
pub fn origin_certificate_h2(name: &str) -> (ServerConfig, String) {
    let (mut config, pem) = origin_certificate(name);
    config.alpn_protocols = vec![b"h2".to_vec()];
    (config, pem)
}
