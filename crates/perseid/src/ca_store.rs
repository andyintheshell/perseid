//! Local storage for the TLS interception certificate authority.
//!
//! The CA lives outside the proxy module so the proxy stays free of filesystem
//! policy: the app decides where the certificate and private key go, and tells
//! the user how to trust — and remove — them. See `docs/tls.md`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::proxy::CertificateAuthority;

/// Certificate file name inside the CA directory.
pub const CA_CERT_FILE: &str = "perseid-ca.pem";
/// Private key file name inside the CA directory.
pub const CA_KEY_FILE: &str = "perseid-ca-key.pem";
const CURRENT_FILE: &str = ".perseid-ca-current";
const LOCK_FILE: &str = ".perseid-ca.lock";
const GENERATION_PREFIX: &str = ".perseid-ca-";

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
    // Best-effort paths for display only; load() reports pointer corruption
    // rather than ever silently falling back to an older authority.
    active_dir(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .join(CA_CERT_FILE)
}

pub fn key_path(dir: &Path) -> PathBuf {
    active_dir(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .join(CA_KEY_FILE)
}

/// One atomic pointer selects an immutable certificate/key generation. Both
/// files are staged and synced before the pointer is replaced. Legacy pairs
/// directly under `dir` remain readable until an explicit regeneration.
fn active_dir(dir: &Path) -> Result<PathBuf, String> {
    let path = dir.join(CURRENT_FILE);
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(dir.to_path_buf()),
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
    };
    let mut name = String::new();
    file.take(129)
        .read_to_string(&mut name)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    if name.len() > 128 || !is_generation_name(&name) {
        return Err(format!(
            "invalid CA generation pointer at {}",
            path.display()
        ));
    }
    Ok(dir.join(name))
}

fn is_generation_name(name: &str) -> bool {
    name.strip_prefix(GENERATION_PREFIX).is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    })
}

fn lock_store(dir: &Path) -> Result<fs::File, String> {
    fs::create_dir_all(dir).map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    set_mode(dir, 0o700)
        .map_err(|err| format!("could not restrict permissions on {}: {err}", dir.display()))?;
    let path = dir.join(LOCK_FILE);
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|err| format!("could not open {}: {err}", path.display()))?;
    file.lock()
        .map_err(|err| format!("could not lock {}: {err}", dir.display()))?;
    // The file is deliberately never unlinked: otherwise another process could
    // lock a new inode while a previous caller still held the old lock.
    Ok(file)
}

/// Load the CA when the directory already has one.
///
/// Missing files mean "no CA yet" and are not an error; half-present or
/// unreadable files are, because silently replacing a CA would invalidate the
/// trust the user installed.
#[cfg(test)]
fn load(dir: &Path) -> Result<Option<Arc<CertificateAuthority>>, String> {
    if !dir.exists() {
        return Ok(None);
    }
    let _lock = lock_store(dir)?;
    load_locked(dir)
}

fn load_locked(dir: &Path) -> Result<Option<Arc<CertificateAuthority>>, String> {
    let active = active_dir(dir)?;
    let cert = active.join(CA_CERT_FILE);
    let key = active.join(CA_KEY_FILE);
    if !cert.exists() && !key.exists() {
        if active != dir {
            return Err(format!(
                "the active CA generation is missing at {}",
                active.display()
            ));
        }
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
    let _lock = lock_store(dir)?;
    match load_locked(dir)? {
        Some(ca) => Ok((ca, false)),
        None => generate_locked(dir).map(|ca| (ca, true)),
    }
}

/// Generate a fresh CA and persist it (directory `0700`, key `0600`, public
/// certificate `0644`).
pub fn generate(dir: &Path) -> Result<Arc<CertificateAuthority>, String> {
    let _lock = lock_store(dir)?;
    generate_locked(dir)
}

fn generate_locked(dir: &Path) -> Result<Arc<CertificateAuthority>, String> {
    let ca = CertificateAuthority::generate()
        .map_err(|err| format!("could not generate a certificate authority: {err}"))?;
    persist(dir, &ca, |_| Ok(()))?;
    Ok(Arc::new(ca))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersistStage {
    Key,
    Certificate,
    Pointer,
}

/// The hook makes disk-failure paths deterministic in tests, without changing
/// process-wide permissions or depending on whether tests run as root.
fn persist(
    dir: &Path,
    ca: &CertificateAuthority,
    mut after_write: impl FnMut(PersistStage) -> Result<(), String>,
) -> Result<(), String> {
    static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let name = format!(
        "{GENERATION_PREFIX}{:x}-{stamp:x}-{sequence:x}",
        std::process::id()
    );
    let generation = dir.join(&name);
    fs::create_dir(&generation)
        .map_err(|err| format!("could not create {}: {err}", generation.display()))?;
    let pointer = generation.join(CURRENT_FILE);
    let result = (|| {
        set_mode(&generation, 0o700)
            .map_err(|err| format!("could not restrict {}: {err}", generation.display()))?;
        write_file(&generation.join(CA_KEY_FILE), &ca.key_pem(), 0o600)?;
        after_write(PersistStage::Key)?;
        write_file(&generation.join(CA_CERT_FILE), ca.cert_pem(), 0o644)?;
        after_write(PersistStage::Certificate)?;
        // Validate the bytes on disk, not merely the original in-memory pair.
        let cert =
            fs::read_to_string(generation.join(CA_CERT_FILE)).map_err(|err| err.to_string())?;
        let key =
            fs::read_to_string(generation.join(CA_KEY_FILE)).map_err(|err| err.to_string())?;
        CertificateAuthority::from_pem(&cert, &key).map_err(|err| err.to_string())?;
        write_file(&pointer, &name, 0o600)?;
        sync_dir(&generation).map_err(|err| format!("could not sync CA generation: {err}"))?;
        sync_dir(dir).map_err(|err| format!("could not sync CA directory: {err}"))?;
        after_write(PersistStage::Pointer)?;
        fs::rename(&pointer, dir.join(CURRENT_FILE))
            .map_err(|err| format!("could not commit the CA generation: {err}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = remove_generation(&generation);
        return result;
    }
    // The commit is now visible. A durability or cleanup failure must not be
    // reported as a failed regeneration: callers must use the committed CA.
    if let Err(err) = sync_dir(dir) {
        log::warn!("CA generation committed, but directory sync failed: {err}");
        // Keep the previous files in case a crash rolls back an undurable
        // pointer rename. The next successful commit can clean them up.
        return Ok(());
    }
    if let Err(err) = prune_generations(dir, Some(&generation)) {
        log::warn!("CA generation committed, but old CA cleanup failed: {err}");
    }
    Ok(())
}

fn remove_generation(dir: &Path) -> Result<(), String> {
    for name in [CA_KEY_FILE, CA_CERT_FILE, CURRENT_FILE] {
        remove_file_if_present(&dir.join(name))?;
    }
    fs::remove_dir(dir).map_err(|err| format!("could not remove {}: {err}", dir.display()))
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("could not remove {}: {err}", path.display())),
    }
}

fn prune_generations(dir: &Path, keep: Option<&Path>) -> Result<(), String> {
    // Delete only known CA files; never recursively remove unexpected content.
    for name in [CA_KEY_FILE, CA_CERT_FILE] {
        remove_file_if_present(&dir.join(name))?;
    }
    for entry in fs::read_dir(dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if Some(path.as_path()) != keep
            && entry.file_type().map_err(|err| err.to_string())?.is_dir()
            && entry.file_name().to_str().is_some_and(is_generation_name)
        {
            remove_generation(&path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Write a CA file with its final permissions from the start.
///
/// `OpenOptionsExt::mode` applies when the file is created, so a new private
/// key is never briefly readable by other local users. Staged files must be
/// new: an existing key is never truncated or overwritten. Errors are returned
/// instead of being ignored: a key that cannot be made owner-only must not be used.
fn write_file(path: &Path, contents: &str, mode: u32) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("could not open {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|err| {
                format!(
                    "could not restrict permissions on {} to {mode:o}: {err}",
                    path.display()
                )
            })?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    file.write_all(contents.as_bytes())
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    file.sync_all()
        .map_err(|err| format!("could not sync {}: {err}", path.display()))?;
    Ok(())
}

/// Remove the stored CA files, if present.
pub fn delete(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let _lock = lock_store(dir)?;
    prune_generations(dir, None)?;
    remove_file_if_present(&dir.join(CURRENT_FILE))?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

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
        assert_eq!(mode(key_path(&dir).parent().unwrap()), 0o700);
        assert_eq!(mode(&dir.join(CURRENT_FILE)), 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn regeneration_restores_owner_only_key_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("regenerate-modes");
        generate(&dir).expect("generate");
        let key = key_path(&dir);
        fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).expect("loosen key mode");

        generate(&dir).expect("regenerate");

        let mode = fs::metadata(key_path(&dir))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
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

    #[test]
    fn failed_staging_preserves_the_previous_pair_at_every_step() {
        let dir = temp_dir("failed-regeneration");
        let original = generate(&dir).unwrap();
        let original_key = fs::read(key_path(&dir)).unwrap();
        for fail_at in [
            PersistStage::Key,
            PersistStage::Certificate,
            PersistStage::Pointer,
        ] {
            let replacement = CertificateAuthority::generate().unwrap();
            let lock = lock_store(&dir).unwrap();
            let error = persist(&dir, &replacement, |stage| {
                if stage == fail_at {
                    Err("injected disk failure".to_string())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            drop(lock);
            assert!(error.contains("injected disk failure"));
            assert_eq!(load(&dir).unwrap().unwrap().cert_pem(), original.cert_pem());
            assert_eq!(fs::read(key_path(&dir)).unwrap(), original_key);
            assert_eq!(
                fs::read_dir(&dir)
                    .unwrap()
                    .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                    .count(),
                1
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_first_generation_leaves_no_half_pair() {
        let dir = temp_dir("failed-first-generation");
        let ca = CertificateAuthority::generate().unwrap();
        for fail_at in [
            PersistStage::Key,
            PersistStage::Certificate,
            PersistStage::Pointer,
        ] {
            let lock = lock_store(&dir).unwrap();
            assert!(
                persist(&dir, &ca, |stage| {
                    if stage == fail_at {
                        Err("injected disk failure".to_string())
                    } else {
                        Ok(())
                    }
                })
                .is_err()
            );
            drop(lock);
            assert!(load(&dir).unwrap().is_none());
            assert!(!dir.join(CURRENT_FILE).exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_ca_is_reused_and_regeneration_commits_a_complete_generation() {
        let dir = temp_dir("legacy");
        fs::create_dir_all(&dir).unwrap();
        let original = CertificateAuthority::generate().unwrap();
        write_file(&dir.join(CA_CERT_FILE), original.cert_pem(), 0o644).unwrap();
        write_file(&dir.join(CA_KEY_FILE), &original.key_pem(), 0o600).unwrap();
        let (loaded, created) = ensure(&dir).unwrap();
        assert!(!created);
        assert_eq!(loaded.cert_pem(), original.cert_pem());
        assert_eq!(cert_path(&dir), dir.join(CA_CERT_FILE));

        let replacement = generate(&dir).unwrap();
        assert_ne!(replacement.cert_pem(), original.cert_pem());
        assert_eq!(
            load(&dir).unwrap().unwrap().cert_pem(),
            replacement.cert_pem()
        );
        assert!(!dir.join(CA_CERT_FILE).exists());
        assert!(!dir.join(CA_KEY_FILE).exists());
        assert!(cert_path(&dir).exists() && key_path(&dir).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_first_launches_share_one_authority() {
        let dir = temp_dir("concurrent-ensure");
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let (ca, created) = ensure(&dir).unwrap();
                    (ca.cert_pem().to_string(), created)
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|(_, created)| *created).count(), 1);
        assert!(results.iter().all(|(cert, _)| cert == &results[0].0));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn uncommitted_generations_do_not_replace_the_active_ca() {
        let dir = temp_dir("interrupted-generation");
        let original = generate(&dir).unwrap();
        let staged = dir.join(format!("{GENERATION_PREFIX}dead-beef"));
        fs::create_dir(&staged).unwrap();
        write_file(&staged.join(CA_KEY_FILE), "incomplete key", 0o600).unwrap();
        assert_eq!(load(&dir).unwrap().unwrap().cert_pem(), original.cert_pem());
        generate(&dir).unwrap();
        assert!(!staged.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_generation_pointers_never_fall_back_or_escape_the_store() {
        let dir = temp_dir("invalid-pointer");
        fs::create_dir_all(&dir).unwrap();
        write_file(&dir.join(CURRENT_FILE), "../outside", 0o600).unwrap();
        assert!(
            load(&dir)
                .unwrap_err()
                .contains("invalid CA generation pointer")
        );
        assert!(ensure(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_readers_never_observe_a_half_replaced_pair() {
        let dir = temp_dir("concurrent-readers");
        generate(&dir).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..8 {
                    generate(&dir).unwrap();
                }
            });
            for _ in 0..4 {
                let dir = &dir;
                scope.spawn(move || {
                    for _ in 0..16 {
                        assert!(load(dir).unwrap().is_some());
                    }
                });
            }
        });
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn read_only_old_certificate_is_never_truncated_during_regeneration() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("read-only-certificate");
        generate(&dir).unwrap();
        fs::set_permissions(cert_path(&dir), fs::Permissions::from_mode(0o400)).unwrap();
        let replacement = generate(&dir).unwrap();
        assert_eq!(
            load(&dir).unwrap().unwrap().cert_pem(),
            replacement.cert_pem()
        );
        assert!(key_path(&dir).exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
