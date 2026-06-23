//! GPG support, shelling out to the system `gpg`. We keep a private keyring
//! under the cache dir so we never touch the user's keyring. All gpg output is
//! captured and distilled into one clean line, while staying fail-closed: a
//! bad signature, or a missing key, both stop the update.

use crate::config::Repo;
use crate::download;
use crate::repo;
use std::path::{Path, PathBuf};
use std::process::Command;

const GPG_KEY_FILE: &str = "GPG-KEY";

fn keyring_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("gpg")
}

/// Outcome of verifying a repo's checksums signature.
pub enum Verify {
    /// No signature file present (repo doesn't ship one).
    NoSignature,
    /// Verified good; carries the signer's name.
    Good(String),
}

/// Download the repo's GPG-KEY and import it into our private keyring.
/// Returns the number of keys imported (output is captured, not printed).
pub fn import_key(repo_: &Repo, cache_root: &Path) -> Result<(), String> {
    let dir = keyring_dir(cache_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    set_mode_700(&dir);

    let url = repo_.join_url(GPG_KEY_FILE);
    let key = download::get_bytes(&url).map_err(|e| format!("fetch {url}: {e}"))?;
    let key_path = dir.join(format!("{}-GPG-KEY", repo_.name));
    std::fs::write(&key_path, &key).map_err(|e| format!("write key: {e}"))?;

    let out = Command::new("gpg")
        .args(["--homedir", &dir.to_string_lossy(), "--batch", "--import"])
        .arg(&key_path)
        .output()
        .map_err(|e| format!("failed to run gpg: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("gpg --import failed for repo '{}'", repo_.name))
    }
}

/// Verify CHECKSUMS.md5 against CHECKSUMS.md5.asc using our keyring.
///
/// fail-closed: a BAD signature or a missing public key both return Err.
/// A repo that simply ships no signature returns Ok(NoSignature).
pub fn verify_checksums(repo_: &Repo, cache_root: &Path) -> Result<Verify, String> {
    let sig = repo::meta_path(repo_, cache_root, repo::CHECKSUMS_ASC);
    let data = repo::meta_path(repo_, cache_root, repo::CHECKSUMS);
    if !sig.exists() {
        return Ok(Verify::NoSignature);
    }
    let dir = keyring_dir(cache_root);
    let out = Command::new("gpg")
        .args(["--homedir", &dir.to_string_lossy(), "--batch", "--status-fd", "1", "--verify"])
        .arg(&sig)
        .arg(&data)
        .output()
        .map_err(|e| format!("failed to run gpg: {e}"))?;

    // gpg --status-fd 1 emits machine-readable lines on stdout.
    let status = String::from_utf8_lossy(&out.stdout);
    if status.lines().any(|l| l.starts_with("[GNUPG:] GOODSIG")) {
        let signer = status
            .lines()
            .find_map(|l| l.strip_prefix("[GNUPG:] GOODSIG "))
            .and_then(|rest| rest.splitn(2, ' ').nth(1))
            .map(|s| s.to_string())
            .unwrap_or_else(|| repo_.name.clone());
        return Ok(Verify::Good(signer));
    }
    if status.lines().any(|l| l.starts_with("[GNUPG:] BADSIG")) {
        return Err(format!(
            "BAD GPG signature for repo '{}' — refusing to continue (possible tampering)",
            repo_.name
        ));
    }
    if status.lines().any(|l| l.starts_with("[GNUPG:] NO_PUBKEY")) {
        return Err(format!(
            "no public key for repo '{}' — run `slacker update gpg` first",
            repo_.name
        ));
    }
    Err(format!("could not verify GPG signature for repo '{}'", repo_.name))
}

/// Verify an arbitrary file against a detached `.asc` signature using our
/// keyring. Same fail-closed contract as `verify_checksums`: a BAD signature or
/// a missing public key both return Err; a signature file that is simply not
/// present returns Ok(NoSignature) so the caller can fall back (e.g. to md5).
pub fn verify_detached(
    repo_: &Repo,
    cache_root: &Path,
    data: &Path,
    sig: &Path,
) -> Result<Verify, String> {
    if !sig.exists() {
        return Ok(Verify::NoSignature);
    }
    let dir = keyring_dir(cache_root);
    let out = Command::new("gpg")
        .args(["--homedir", &dir.to_string_lossy(), "--batch", "--status-fd", "1", "--verify"])
        .arg(sig)
        .arg(data)
        .output()
        .map_err(|e| format!("failed to run gpg: {e}"))?;

    let status = String::from_utf8_lossy(&out.stdout);
    if status.lines().any(|l| l.starts_with("[GNUPG:] GOODSIG")) {
        let signer = status
            .lines()
            .find_map(|l| l.strip_prefix("[GNUPG:] GOODSIG "))
            .and_then(|rest| rest.splitn(2, ' ').nth(1))
            .map(|s| s.to_string())
            .unwrap_or_else(|| repo_.name.clone());
        return Ok(Verify::Good(signer));
    }
    if status.lines().any(|l| l.starts_with("[GNUPG:] BADSIG")) {
        return Err(format!(
            "BAD GPG signature for {} (repo '{}') — refusing to install (possible tampering)",
            data.display(),
            repo_.name
        ));
    }
    if status.lines().any(|l| l.starts_with("[GNUPG:] NO_PUBKEY")) {
        return Err(format!(
            "no public key for repo '{}' — run `slacker update gpg` first",
            repo_.name
        ));
    }
    Err(format!("could not verify GPG signature for {}", data.display()))
}

#[cfg(unix)]
fn set_mode_700(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_mode_700(_dir: &Path) {}
