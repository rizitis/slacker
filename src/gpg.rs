//! GPG support, shelling out to the system `gpg`. We keep a private keyring
//! under the cache dir so we never touch the user's keyring.
//!
//! Trust model (this is the security anchor of the whole package manager):
//! a repo serves its own GPG-KEY, so importing it proves nothing on its own —
//! a malicious or MITM'd repo would simply serve its own key and signature.
//! We therefore PIN the key fingerprint per repo on first import (trust on
//! first use) and show it to the user to verify out of band. Every later
//! verification requires the signature to come from that exact pinned
//! fingerprint, and a key that changes is refused. This binds signer→repo and
//! defeats both repo-supplied-key forgery (after the pin) and cross-repo
//! signature reuse (a key pinned for one repo cannot validate another).

use crate::config::Repo;
use crate::download;
use crate::repo;
use std::path::{Path, PathBuf};
use std::process::Command;

const GPG_KEY_FILE: &str = "GPG-KEY";

fn keyring_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("gpg")
}

/// Where the pinned fingerprint(s) for a repo are stored.
fn pin_path(cache_root: &Path, repo_name: &str) -> PathBuf {
    keyring_dir(cache_root).join(format!("{repo_name}.fpr"))
}

fn read_pin(cache_root: &Path, repo_name: &str) -> Vec<String> {
    match std::fs::read_to_string(pin_path(cache_root, repo_name)) {
        Ok(s) => s
            .lines()
            .map(|l| l.trim().to_uppercase())
            .filter(|l| l.len() == 40 && l.chars().all(|c| c.is_ascii_hexdigit()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Outcome of verifying a repo's checksums signature against the pinned key.
pub enum Verify {
    /// No signature file present (repo doesn't ship one).
    NoSignature,
    /// Verified good against the pinned key; carries the signer's name.
    Good(String),
    /// A signature is present but HOSTILE: a bad signature, or one made by a key
    /// that is not the pinned key (key-substitution). Always fatal.
    Tampered(String),
    /// A signature is present but cannot be checked (no public key, or no pin).
    /// Not necessarily hostile — under a best-effort policy we fall back to md5.
    Unverifiable(String),
}

/// Outcome of importing/pinning a repo's key.
pub enum ImportOutcome {
    /// First contact: this fingerprint was just pinned (user should verify it).
    NewlyPinned(String),
    /// The fetched key matches the already-pinned fingerprint.
    AlreadyTrusted,
}

/// Why importing/pinning a key failed.
pub enum ImportError {
    /// The repo now serves a DIFFERENT key than the one pinned — possible
    /// compromise/spoof. Always treated as hostile.
    KeyChanged(String),
    /// Anything else (key not served, unreadable, gpg error). Not by itself
    /// proof of tampering.
    Other(String),
}

impl ImportError {
    pub fn message(&self) -> &str {
        match self {
            ImportError::KeyChanged(m) | ImportError::Other(m) => m,
        }
    }
}

/// Read the fingerprint(s) contained in a key file WITHOUT importing it, using
/// gpg's show-only mode. Reliable even if the key is already in the keyring.
fn fingerprints_in_keyfile(dir: &Path, key_path: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("gpg")
        .args(["--homedir", &dir.to_string_lossy(), "--batch", "--with-colons"])
        .args(["--import-options", "show-only", "--import"])
        .arg(key_path)
        .output()
        .map_err(|e| format!("failed to run gpg: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fprs: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("fpr:") {
            // fpr:::::::::<FINGERPRINT>:
            let fpr = rest.trim_matches(':').to_uppercase();
            if fpr.len() == 40 && fpr.chars().all(|c| c.is_ascii_hexdigit()) && !fprs.contains(&fpr)
            {
                fprs.push(fpr);
            }
        }
    }
    if fprs.is_empty() {
        return Err("no usable public key found in the repo's GPG-KEY".into());
    }
    Ok(fprs)
}

/// Download the repo's GPG-KEY, read its fingerprint, and pin it (trust on
/// first use). The key is also imported into our private keyring so gpg can
/// later verify signatures with it. fail-closed: if a pin already exists and
/// the fetched key's fingerprint differs, this is refused as a possible attack.
pub fn import_key(repo_: &Repo, cache_root: &Path) -> Result<ImportOutcome, ImportError> {
    let dir = keyring_dir(cache_root);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ImportError::Other(format!("mkdir {}: {e}", dir.display())))?;
    set_mode_700(&dir);

    let url = repo_.join_url(GPG_KEY_FILE);
    // Cap the key download — a key is tiny; anything huge is hostile.
    let key = download::get_bytes_capped(&url, 1 << 20)
        .map_err(|e| ImportError::Other(format!("fetch {url}: {e}")))?;
    let key_path = dir.join(format!("{}-GPG-KEY", repo_.name));
    std::fs::write(&key_path, &key).map_err(|e| ImportError::Other(format!("write key: {e}")))?;

    let fetched = fingerprints_in_keyfile(&dir, &key_path).map_err(ImportError::Other)?;
    let pinned = read_pin(cache_root, &repo_.name);

    // If we already pinned a fingerprint, the fetched key MUST match it.
    if !pinned.is_empty() && pinned != fetched {
        return Err(ImportError::KeyChanged(format!(
            "the GPG key for repo '{}' CHANGED.\n  pinned:  {}\n  offered: {}\n\
             Refusing — this can mean the repo was compromised or is being spoofed.\n\
             If you are certain the new key is legitimate, remove the pin file\n  {}\n\
             and run `slacker update gpg` again to re-pin (at your own responsibility).",
            repo_.name,
            pinned.join(", "),
            fetched.join(", "),
            pin_path(cache_root, &repo_.name).display(),
        )));
    }

    // Import the key into the keyring for real (idempotent; gpg merges).
    let out = Command::new("gpg")
        .args(["--homedir", &dir.to_string_lossy(), "--batch", "--import"])
        .arg(&key_path)
        .output()
        .map_err(|e| ImportError::Other(format!("failed to run gpg: {e}")))?;
    if !out.status.success() {
        return Err(ImportError::Other(format!("gpg --import failed for repo '{}'", repo_.name)));
    }

    if pinned.is_empty() {
        // Trust on first use: record the pin.
        let _ = std::fs::write(pin_path(cache_root, &repo_.name), format!("{}\n", fetched.join("\n")));
        Ok(ImportOutcome::NewlyPinned(fetched.join(", ")))
    } else {
        Ok(ImportOutcome::AlreadyTrusted)
    }
}

/// Pull the VALIDSIG key fingerprints out of gpg's --status-fd output.
fn validsig_fprs(status: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in status
        .lines()
        .filter_map(|l| l.strip_prefix("[GNUPG:] VALIDSIG "))
        .flat_map(|rest| rest.split_whitespace())
        .map(|t| t.to_uppercase())
        .filter(|t| t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit()))
    {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Run `gpg --verify sig data` and interpret the result against the repo's
/// pinned fingerprint. Shared by both checksum and per-package verification.
///
/// fail-closed: a BAD signature, a missing public key, a signature from a key
/// that is NOT the pinned one, or the absence of a pin entirely, all return
/// Err. A signature file that is simply not present returns Ok(NoSignature) so
/// the caller may fall back (e.g. to md5) when policy allows.
fn verify_against_pin(
    repo_: &Repo,
    cache_root: &Path,
    data: &Path,
    sig: &Path,
    what: &str,
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

    if status.lines().any(|l| l.starts_with("[GNUPG:] BADSIG")) {
        return Ok(Verify::Tampered(format!(
            "BAD GPG signature for {what} (repo '{}')",
            repo_.name
        )));
    }
    if status.lines().any(|l| l.starts_with("[GNUPG:] NO_PUBKEY")) {
        return Ok(Verify::Unverifiable(format!(
            "no public key for repo '{}' — run `slacker update gpg`",
            repo_.name
        )));
    }
    let good = status.lines().any(|l| l.starts_with("[GNUPG:] GOODSIG"));
    if !good {
        return Ok(Verify::Unverifiable(format!(
            "could not verify GPG signature for {what} (repo '{}')",
            repo_.name
        )));
    }

    // GOODSIG alone is not enough: the signing key must be the PINNED key.
    let pinned = read_pin(cache_root, &repo_.name);
    if pinned.is_empty() {
        return Ok(Verify::Unverifiable(format!(
            "repo '{}' has a signature but no pinned key yet — run `slacker update gpg`",
            repo_.name
        )));
    }
    let signers = validsig_fprs(&status);
    if !signers.iter().any(|s| pinned.contains(s)) {
        return Ok(Verify::Tampered(format!(
            "signature for {what} (repo '{}') is from an UNPINNED key (got {}, pinned {}) — \
             possible key-substitution attack",
            repo_.name,
            if signers.is_empty() { "unknown".into() } else { signers.join(", ") },
            pinned.join(", "),
        )));
    }

    let signer = status
        .lines()
        .find_map(|l| l.strip_prefix("[GNUPG:] GOODSIG "))
        .and_then(|rest| rest.splitn(2, ' ').nth(1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| repo_.name.clone());
    Ok(Verify::Good(signer))
}

/// Verify CHECKSUMS.md5 against CHECKSUMS.md5.asc using the pinned key.
pub fn verify_checksums(repo_: &Repo, cache_root: &Path) -> Result<Verify, String> {
    let sig = repo::meta_path(repo_, cache_root, repo::CHECKSUMS_ASC);
    let data = repo::meta_path(repo_, cache_root, repo::CHECKSUMS);
    verify_against_pin(repo_, cache_root, &data, &sig, "CHECKSUMS.md5")
}

/// Verify an arbitrary file against a detached `.asc` using the pinned key.
pub fn verify_detached(
    repo_: &Repo,
    cache_root: &Path,
    data: &Path,
    sig: &Path,
) -> Result<Verify, String> {
    let what = data.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    verify_against_pin(repo_, cache_root, data, sig, &what)
}

#[cfg(unix)]
fn set_mode_700(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_mode_700(_dir: &Path) {}
