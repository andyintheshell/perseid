//! Local certificate authority and TLS interception support.
//!
//! Perseid mints per-host leaf certificates from a locally generated
//! certificate authority and terminates TLS itself. The CA private key never
//! leaves the machine running Perseid, and the proxy never installs it into a
//! trust store: the user trusts and removes the CA deliberately. See
//! `docs/tls.md` in the repository for the setup and removal steps.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use log::{debug, warn};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

/// Minted certificates kept in memory before the oldest is evicted.
///
/// A page load touches a handful of hosts; the bound keeps a long browsing
/// session from growing certificates without limit.
const CERT_CACHE_CAPACITY: usize = 256;

/// Failure to create, load, or use a certificate authority.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("could not generate the certificate authority: {0}")]
    Generate(#[source] rcgen::Error),
    #[error("could not parse a certificate from PEM: {0}")]
    Certificate(#[source] std::io::Error),
    #[error("the certificate PEM did not contain a certificate")]
    MissingCertificate,
    #[error("could not parse the private key from PEM: {0}")]
    Key(#[source] rcgen::Error),
    #[error("a certificate needs a non-empty host name")]
    MissingHost,
    #[error("refusing to mint a certificate for an invalid host name: {host}")]
    InvalidHost { host: String },
    #[error("could not mint a certificate for {host}: {source}")]
    Mint {
        host: String,
        #[source]
        source: rcgen::Error,
    },
    #[error("could not prepare the signing key for {host}: {source}")]
    SigningKey {
        host: String,
        #[source]
        source: rustls::Error,
    },
}

/// A local certificate authority used to sign interception certificates.
///
/// Generate one with [`CertificateAuthority::generate`] on first use, persist
/// the two PEM documents somewhere only the user can read, and pass it to
/// [`crate::proxy::ProxyConfig::tls`]. Loading an existing CA with
/// [`CertificateAuthority::from_pem`] lets the trust the user installed keep
/// working across restarts.
pub struct CertificateAuthority {
    params: CertificateParams,
    key: KeyPair,
    cert_der: CertificateDer<'static>,
    cert_pem: String,
    cache: Mutex<CertCache>,
}

impl std::fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateAuthority")
            .field("subject", &"Perseid CA")
            .field(
                "cached_certificates",
                &self.cache.lock().map_or(0, |c| c.len()),
            )
            .finish_non_exhaustive()
    }
}

impl CertificateAuthority {
    /// Create a fresh ECDSA P-256 certificate authority.
    pub fn generate() -> Result<Self, CaError> {
        let params = ca_params().map_err(CaError::Generate)?;
        let key = KeyPair::generate().map_err(CaError::Generate)?;
        let cert = params.self_signed(&key).map_err(CaError::Generate)?;
        Ok(Self {
            cert_der: cert.der().clone(),
            cert_pem: cert.pem(),
            params,
            key,
            cache: Mutex::new(CertCache::new()),
        })
    }

    /// Load a certificate authority previously saved with
    /// [`CertificateAuthority::cert_pem`] and [`CertificateAuthority::key_pem`].
    ///
    /// Only the first certificate in `cert_pem` is used. The CA profile is
    /// fixed, so the subject and key usage extensions are reconstructed the
    /// same way they were at generation time.
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, CaError> {
        let cert_der = rustls_pemfile::certs(&mut Cursor::new(cert_pem.as_bytes()))
            .next()
            .transpose()
            .map_err(CaError::Certificate)?
            .ok_or(CaError::MissingCertificate)?;
        let key = KeyPair::from_pem(key_pem).map_err(CaError::Key)?;
        let params = ca_params().map_err(CaError::Generate)?;
        Ok(Self {
            params,
            key,
            cert_der,
            cert_pem: cert_pem.to_string(),
            cache: Mutex::new(CertCache::new()),
        })
    }

    /// The CA certificate in PEM form, as shown to or imported by the user.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// The CA private key in PKCS#8 PEM form. Persist it owner-readable only.
    pub fn key_pem(&self) -> String {
        self.key.serialize_pem()
    }

    /// Mint (or reuse) a leaf certificate for `host` and return it ready for
    /// rustls. IP addresses and DNS names are both supported.
    pub fn mint(&self, host: &str) -> Result<Arc<CertifiedKey>, CaError> {
        let host = normalize_host(host);
        if host.is_empty() {
            return Err(CaError::MissingHost);
        }
        validate_host(&host)?;
        if let Ok(mut cache) = self.cache.lock()
            && let Some(entry) = cache.get(&host)
        {
            return Ok(entry);
        }

        let certified = self.mint_uncached(&host)?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(host, certified.clone());
        }
        Ok(certified)
    }

    fn mint_uncached(&self, host: &str) -> Result<Arc<CertifiedKey>, CaError> {
        let mut params =
            CertificateParams::new(vec![host.to_string()]).map_err(|source| CaError::Mint {
                host: host.to_string(),
                source,
            })?;
        params.is_ca = IsCa::NoCa;
        params.use_authority_key_identifier_extension = true;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params
            .distinguished_name
            .push(DnType::CommonName, host.to_string());

        let leaf_key = KeyPair::generate().map_err(|source| CaError::Mint {
            host: host.to_string(),
            source,
        })?;
        let issuer = Issuer::from_params(&self.params, &self.key);
        let cert = params
            .signed_by(&leaf_key, &issuer)
            .map_err(|source| CaError::Mint {
                host: host.to_string(),
                source,
            })?;

        let key_der = PrivatePkcs8KeyDer::from(leaf_key.serialize_der());
        let signing_key =
            rustls::crypto::ring::sign::any_supported_type(&PrivateKeyDer::Pkcs8(key_der))
                .map_err(|source| CaError::SigningKey {
                    host: host.to_string(),
                    source,
                })?;

        // Leaf first, issuer second: what clients expect on the wire.
        Ok(Arc::new(CertifiedKey::new(
            vec![cert.der().clone(), self.cert_der.clone()],
            signing_key,
        )))
    }

    /// A rustls server configuration that mints a certificate for every SNI
    /// name it sees. `fallback_host` covers clients that send no SNI, such as
    /// `openssl s_client` without `-servername`.
    pub fn server_config(self: &Arc<Self>, fallback_host: Option<String>) -> Arc<ServerConfig> {
        let resolver = Arc::new(CaResolver {
            ca: Arc::clone(self),
            fallback_host,
        });
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver);
        // Prefer HTTP/2 inside the tunnel while retaining HTTP/1.1 for clients
        // that do not offer it. The connection builder detects the protocol
        // from ALPN/the HTTP/2 preface.
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Arc::new(config)
    }
}

fn ca_params() -> Result<CertificateParams, rcgen::Error> {
    let mut params = CertificateParams::new(Vec::new())?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params
        .distinguished_name
        .push(DnType::CommonName, "Perseid CA");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Perseid");
    Ok(params)
}

fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    // CONNECT authorities keep the brackets around IPv6 literals; SANs do not.
    match host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        Some(inner) if inner.contains(':') => inner.to_string(),
        _ => host,
    }
}

/// Reject hosts that cannot become a certificate SAN.
///
/// rcgen happily signs whatever string it is given, so the shape is checked
/// with the same parser rustls uses for the client hello and server names.
fn validate_host(host: &str) -> Result<(), CaError> {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }
    if rustls::pki_types::ServerName::try_from(host.to_string()).is_ok() {
        return Ok(());
    }
    Err(CaError::InvalidHost {
        host: host.to_string(),
    })
}

#[derive(Debug)]
struct CaResolver {
    ca: Arc<CertificateAuthority>,
    fallback_host: Option<String>,
}

impl ResolvesServerCert for CaResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = hello
            .server_name()
            .map(str::to_string)
            .or_else(|| self.fallback_host.clone())?;
        match self.ca.mint(&host) {
            Ok(certified) => Some(certified),
            Err(err) => {
                warn!("TLS interception: {err}");
                None
            }
        }
    }
}

/// Failure to build the upstream (origin server) TLS client.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamTlsError {
    #[error("could not parse an upstream root certificate: {0}")]
    Root(#[source] std::io::Error),
    #[error("could not trust an upstream root certificate: {0}")]
    Trust(#[source] rustls::Error),
}

/// Build the TLS client configuration used for connections to origin servers.
///
/// Platform trust anchors are used so origin certificates validate exactly as
/// the user's browser would see them. `extra_roots_pem` adds trust anchors on
/// top of the platform store, which tests and internal deployments use for
/// their own certificate authorities.
pub fn upstream_client_config(
    extra_roots_pem: &[String],
) -> Result<Arc<rustls::ClientConfig>, UpstreamTlsError> {
    let mut roots = rustls::RootCertStore::empty();

    let native = rustls_native_certs::load_native_certs();
    if !native.errors.is_empty() {
        // Log only the count. The error details are not needed for normal
        // operation, and keeping them out of the log avoids treating
        // certificate-store diagnostics as sensitive or machine-specific.
        warn!(
            "could not load {} platform root certificate(s); continuing with the rest",
            native.errors.len()
        );
    }
    for cert in native.certs {
        if let Err(err) = roots.add(cert) {
            debug!("skipping an unusable platform root certificate: {err}");
        }
    }
    if roots.is_empty() {
        warn!("no platform trust anchors found; using the bundled webpki roots");
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    for pem in extra_roots_pem {
        for cert in rustls_pemfile::certs(&mut Cursor::new(pem.as_bytes())) {
            let cert = cert.map_err(UpstreamTlsError::Root)?;
            roots.add(cert).map_err(UpstreamTlsError::Trust)?;
        }
    }

    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    // Let the upstream client negotiate the best protocol the origin supports.
    // hyper-rustls reports the selected ALPN protocol to hyper-util, which
    // switches the pooled connection to HTTP/2 when the origin selects it.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

struct CertCache {
    entries: HashMap<String, Arc<CertifiedKey>>,
    order: VecDeque<String>,
}

impl CertCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&mut self, host: &str) -> Option<Arc<CertifiedKey>> {
        self.entries.get(host).cloned()
    }

    fn insert(&mut self, host: String, certified: Arc<CertifiedKey>) {
        if self.entries.insert(host.clone(), certified).is_none() {
            self.order.push_back(host);
        }
        while self.order.len() > CERT_CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_a_reusable_authority() {
        let ca = CertificateAuthority::generate().expect("generate");
        assert!(ca.cert_pem().starts_with("-----BEGIN CERTIFICATE-----"));

        let first = ca.mint("example.com").expect("mint");
        let second = ca.mint("Example.COM.").expect("mint normalized");
        assert!(
            Arc::ptr_eq(&first, &second),
            "host forms share a certificate"
        );
        assert_eq!(first.cert.len(), 2, "leaf plus issuer chain");

        let key = ca.key_pem();
        assert!(key.starts_with("-----BEGIN PRIVATE KEY-----"));

        let reloaded = CertificateAuthority::from_pem(ca.cert_pem(), &key).expect("reload");
        let minted = reloaded.mint("example.com").expect("mint after reload");
        assert_eq!(minted.cert.len(), 2);
    }

    #[test]
    fn mints_ip_address_certificates() {
        let ca = CertificateAuthority::generate().expect("generate");
        let certified = ca.mint("127.0.0.1").expect("mint ip");
        assert_eq!(certified.cert.len(), 2);

        // Bracketed IPv6 literals from CONNECT authorities share the plain form.
        let plain = ca.mint("::1").expect("mint ipv6");
        let bracketed = ca.mint("[::1]").expect("mint bracketed ipv6");
        assert!(Arc::ptr_eq(&plain, &bracketed));
    }

    #[test]
    fn rejects_missing_and_invalid_hosts() {
        let ca = CertificateAuthority::generate().expect("generate");
        assert!(matches!(ca.mint("  "), Err(CaError::MissingHost)));
        assert!(ca.mint("bad name").is_err());
    }

    #[test]
    fn cache_is_bounded() {
        let ca = CertificateAuthority::generate().expect("generate");
        for index in 0..(CERT_CACHE_CAPACITY + 10) {
            ca.mint(&format!("host{index}.test")).expect("mint");
        }
        let len = ca.cache.lock().expect("cache").len();
        assert_eq!(len, CERT_CACHE_CAPACITY);
    }
}
