//! Local storage for the TLS interception certificate authority.
//!
//! The CA lives outside the proxy crate so the library stays free of
//! filesystem policy: the app decides where the certificate and private key
//! go, and tells the user how to trust — and remove — them. See
//! `docs/tls.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use perseid_proxy::CertificateAuthority;

/// Certificate file name inside the CA directory.
pub const CA_CERT_FILE: &str = "perseid-ca.pem";
/// Private key file name inside the CA directory.
pub const CA_KEY_FILE: &str = "perseid-ca-key.pem";

/// Where the CA is stored when `--ca-dir` is not given.
///
/// Linux and other Unix-like systems use the Perseid data directory under
/// `$XDG_DATA_HOME` (or `~/.local/share`), macOS uses
/// `~/Library/Application Support/perseid`, and Windows uses
/// `%LOCALAPPDATA%\perseid` (falling back to `%APPDATA%`). `PERSEID_CA_DIR`
/// overrides all of them, which keeps sandboxed runs and tests out of the
/// real home.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PERSEID_CA_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(platform_data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("perseid")
}

#[cfg(target_os = "macos")]
fn platform_data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
}

#[cfg(target_os = "windows")]
fn platform_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".local").join("share"))
}

pub fn cert_path(dir: &Path) -> PathBuf {
    dir.join(CA_CERT_FILE)
}

pub fn key_path(dir: &Path) -> PathBuf {
    dir.join(CA_KEY_FILE)
}

/// Load the CA when the directory already has one.
///
/// Missing files mean "no CA yet" and are not an error; half-present or
/// unreadable files are, because silently replacing a CA would invalidate the
/// trust the user installed.
pub fn load(dir: &Path) -> Result<Option<Arc<CertificateAuthority>>, String> {
    let cert = cert_path(dir);
    let key = key_path(dir);
    if !cert.exists() && !key.exists() {
        return Ok(None);
    }
    if !cert.exists() {
        return Err(format!(
            "{} is missing its certificate; generate a new CA to continue",
            key.display()
        ));
    }
    if !key.exists() {
        return Err(format!(
            "{} is missing its private key; generate a new CA to continue",
            cert.display()
        ));
    }
    let cert_pem = fs::read_to_string(&cert)
        .map_err(|err| format!("could not read {}: {err}", cert.display()))?;
    let key_pem = fs::read_to_string(&key)
        .map_err(|err| format!("could not read {}: {err}", key.display()))?;
    CertificateAuthority::from_pem(&cert_pem, &key_pem)
        .map(|ca| Some(Arc::new(ca)))
        .map_err(|err| format!("could not load the CA from {}: {err}", dir.display()))
}

/// Load the stored CA, creating and persisting one when the directory is
/// empty.
///
/// An existing CA is never replaced here: the certificate the browser trusts
/// stays stable across restarts and upgrades. A half-present or unreadable CA
/// is reported as an error instead of being overwritten silently.
///
/// Returns the CA and whether it was created by this call.
pub fn ensure(dir: &Path) -> Result<(Arc<CertificateAuthority>, bool), String> {
    match load(dir)? {
        Some(ca) => Ok((ca, false)),
        None => generate(dir).map(|ca| (ca, true)),
    }
}

/// Generate a fresh CA and persist it (directory `0700`, key `0600`, public
/// certificate `0644`).
pub fn generate(dir: &Path) -> Result<Arc<CertificateAuthority>, String> {
    let ca = CertificateAuthority::generate()
        .map_err(|err| format!("could not generate a certificate authority: {err}"))?;
    fs::create_dir_all(dir).map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    set_mode(dir, 0o700);
    let key = key_path(dir);
    let cert = cert_path(dir);
    if let Err(err) = fs::write(&key, ca.key_pem()) {
        return Err(format!("could not write {}: {err}", key.display()));
    }
    set_mode(&key, 0o600);
    if let Err(err) = fs::write(&cert, ca.cert_pem()) {
        // Do not leave a private key without its certificate behind; the next
        // launch would otherwise treat the pair as corrupt.
        let _ = fs::remove_file(&key);
        return Err(format!("could not write {}: {err}", cert.display()));
    }
    set_mode(&cert, 0o644);
    Ok(Arc::new(ca))
}

/// Remove the stored CA files, if present.
pub fn delete(dir: &Path) -> Result<(), String> {
    for path in [cert_path(dir), key_path(dir)] {
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|err| format!("could not remove {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("perseid-ca-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn generates_loads_and_deletes_a_ca() {
        let dir = temp_dir("roundtrip");
        assert!(load(&dir).expect("empty directory loads").is_none());

        let generated = generate(&dir).expect("generate");
        let loaded = load(&dir).expect("load").expect("present");
        assert_eq!(generated.cert_pem(), loaded.cert_pem());

        let key = key_path(&dir);
        assert!(key.exists());

        delete(&dir).expect("delete");
        assert!(load(&dir).expect("empty after delete").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_creates_once_and_reuses_the_same_ca() {
        let dir = temp_dir("ensure");
        let (first, created) = ensure(&dir).expect("first ensure");
        assert!(created);
        let (second, created) = ensure(&dir).expect("second ensure");
        assert!(!created);
        assert_eq!(first.cert_pem(), second.cert_pem());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_does_not_replace_a_half_present_ca() {
        let dir = temp_dir("ensure-half");
        generate(&dir).expect("generate");
        fs::remove_file(key_path(&dir)).expect("remove key");
        let error = ensure(&dir).expect_err("half pair is an error");
        assert!(error.contains("private key"), "{error}");
        assert!(cert_path(&dir).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn restricts_ca_file_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("modes");
        generate(&dir).expect("generate");
        let mode = |path: &Path| fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&key_path(&dir)), 0o600);
        assert_eq!(mode(&cert_path(&dir)), 0o644);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_half_of_the_pair_is_an_error() {
        let dir = temp_dir("half");
        generate(&dir).expect("generate");
        fs::remove_file(key_path(&dir)).expect("remove key");
        let error = load(&dir).expect_err("half pair is an error");
        assert!(error.contains("private key"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }
}
