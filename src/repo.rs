//! Repository metadata: fetching and parsing PACKAGES.TXT / CHECKSUMS.md5,
//! plus the auxiliary files needed for full slackpkg parity (MANIFEST.bz2 for
//! file-search, ChangeLog.txt for check-updates, CHECKSUMS.md5.asc for GPG).

use crate::config::Repo;
use crate::download;
use crate::pkg::PkgId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A single available package as advertised by a repo.
#[derive(Clone, Debug)]
pub struct AvailPkg {
    pub id: PkgId,
    pub filename: String,
    pub location: String,
    /// Slackware series derived from the location (a, ap, n, kde, xap, ...).
    pub series: String,
    pub size_k: Option<u64>,
    pub size_uncompressed_k: Option<u64>,
    pub summary: String,
    /// Full multi-line description (without the "name:" prefix).
    pub description: String,
    pub md5: Option<String>,
    /// SHA-256 from CHECKSUMS.sha256, if the repo ships one. None otherwise.
    pub sha: Option<String>,
    /// Dependency package names declared in PACKAGES.TXT via `PACKAGE REQUIRED:`
    /// (slapt-get / SlackBuilds-style repos). Empty for vanilla Slackware trees,
    /// which carry no auto-dependency metadata. Used as a fallback when a repo
    /// ships no per-package `.dep` file.
    pub required: Vec<String>,
    pub repo: String,
}

impl AvailPkg {
    pub fn url(&self, repo: &Repo) -> String {
        repo.join_download_url(&format!("{}{}", self.location, self.filename))
    }
}

/// Names of the metadata files we keep per repo.
pub const PACKAGES_TXT: &str = "PACKAGES.TXT";
pub const PACKAGES_PREV: &str = "PACKAGES.TXT.prev";
pub const CHECKSUMS: &str = "CHECKSUMS.md5";
pub const CHECKSUMS_ASC: &str = "CHECKSUMS.md5.asc";
/// Optional SHA-256 checksums file. No mainstream Slackware repo ships this
/// yet; support is here so that if one does, slacker uses it automatically.
pub const CHECKSUMS_SHA: &str = "CHECKSUMS.sha256";
/// Remote (compressed) manifest filename.
pub const MANIFEST_BZ2: &str = "MANIFEST.bz2";
/// Local, decompressed, possibly-concatenated manifest used by file-search.
pub const MANIFEST: &str = "MANIFEST";
pub const CHANGELOG: &str = "ChangeLog.txt";

/// Download metadata for a repo into its cache dir.
///
/// PACKAGES.TXT and CHECKSUMS.md5 are required. The rest (signature, MANIFEST,
/// ChangeLog) are best-effort. The previous PACKAGES.TXT is retained as
/// PACKAGES.TXT.prev so `install-new` can diff against it.
///
/// MANIFEST.bz2 is fetched lazily by `ensure_manifest` (not here): from the
/// repo root for third-party repos, or the per-arch subdir for the official
/// one. The cached plain-text MANIFEST is dropped on each update so the next
/// file-search re-fetches it.
pub fn update_repo(repo: &Repo, cache_root: &Path, fetch_changelog: bool) -> Result<(), String> {
    use std::io::Write;
    let dir = repo.cache_subdir(cache_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;

    let pkgs_path = dir.join(PACKAGES_TXT);
    if pkgs_path.exists() {
        let _ = std::fs::copy(&pkgs_path, dir.join(PACKAGES_PREV));
    }

    for fname in [PACKAGES_TXT] {
        print!("  {fname} ... ");
        std::io::stdout().flush().ok();
        let url = repo.join_url(fname);
        let bytes = download::get_bytes(&url).map_err(|e| format!("fetch {url}: {e}"))?;
        std::fs::write(dir.join(fname), &bytes)
            .map_err(|e| format!("write {fname}: {e}"))?;
        println!("ok");
    }

    // CHECKSUMS.md5 is best-effort like the signature: nearly every repo ships
    // it, but a repo could provide only sha. If none of the checksum files is
    // present, install/download will refuse later with a clear message, rather
    // than update failing here.
    let mut meta = vec![CHECKSUMS, CHECKSUMS_ASC, CHECKSUMS_SHA];
    if fetch_changelog {
        meta.push(CHANGELOG);
    }
    for fname in meta {
        print!("  {fname} ... ");
        std::io::stdout().flush().ok();
        let url = repo.join_url(fname);
        match download::get_bytes(&url) {
            Ok(bytes) => {
                let _ = std::fs::write(dir.join(fname), &bytes);
                println!("ok");
            }
            Err(_) => {
                let _ = std::fs::remove_file(dir.join(fname));
                println!("not present");
            }
        }
    }

    // NOTE: MANIFEST.bz2 is large (tens of MB for official Slackware) and only
    // needed by file-search, so we fetch it lazily on first use, not here.
    // Drop any cached (now stale) MANIFEST so the next file-search re-fetches.
    let _ = std::fs::remove_file(dir.join(MANIFEST));
    Ok(())
}

/// Fetch a repo's ChangeLog.txt fresh and return its text. With `cache = true`
/// the bytes are also written to the repo's cache (best-effort; the write needs
/// root, but the text is returned regardless so a non-root user can still read
/// it). `show-changelog` passes `cache = false` for the official repo so this
/// never clobbers the ChangeLog that `update` maintains as the check-updates
/// baseline; for other repos it refreshes their cached copy as an offline
/// fallback.
pub fn fetch_changelog_text(
    repo: &Repo,
    cache_root: &Path,
    cache: bool,
) -> Result<String, String> {
    let url = repo.join_url(CHANGELOG);
    let bytes = download::get_bytes(&url).map_err(|e| format!("fetch {url}: {e}"))?;
    if cache {
        let dir = repo.cache_subdir(cache_root);
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(dir.join(CHANGELOG), &bytes);
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Ensure the decompressed MANIFEST exists for a repo, downloading it on
/// demand. Used by file-search.
///
/// Third-party repos publish MANIFEST.bz2 at the repo root — the directory the
/// URL points at, which also holds that repo's PACKAGES.TXT and CHECKSUMS — so
/// we fetch `<url>/MANIFEST.bz2`. The official repo is the one exception: its
/// URL is the distribution root (carrying PACKAGES.TXT, ChangeLog, …), but the
/// MANIFEST.bz2 lives one level down in the per-arch package dir (`slackware64/`
/// on 64-bit, `slackware/` on 32-bit). For the official repo we therefore also
/// try that subdir. We never probe a third-party repo's location subdirs —
/// doing so was what stalled file-search at the network timeout.
pub fn ensure_manifest(repo: &Repo, cache_root: &Path) -> Result<bool, String> {
    let dir = repo.cache_subdir(cache_root);
    let dest = dir.join(MANIFEST);
    if dest.exists() {
        return Ok(true);
    }

    let mut candidates: Vec<String> = vec![String::new()]; // "" == repo root
    if repo.official {
        if let Some(arch_dir) = official_arch_subdir(&dir) {
            candidates.push(arch_dir);
        }
    }

    println!("  fetching MANIFEST for '{}' (large, first time only):", repo.name);
    let tmp_bz2 = dir.join("MANIFEST.bz2.part");

    for sub in &candidates {
        let (rel, label) = if sub.is_empty() {
            (MANIFEST_BZ2.to_string(), repo.name.clone())
        } else {
            (format!("{sub}/{MANIFEST_BZ2}"), format!("{}/{sub}", repo.name))
        };
        let url = repo.join_url(&rel);
        if download::download_to_progress(&url, &tmp_bz2, &label).is_err() {
            continue;
        }
        // Decompress straight into the MANIFEST file, but cap the output so a
        // bzip2 "decompression bomb" (a tiny .bz2 that expands to terabytes)
        // can't fill the disk. Stream gpg/bzip2 stdout through a capped copy.
        let mut child = match std::process::Command::new("bzip2")
            .arg("-dc")
            .arg(&tmp_bz2)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                let _ = std::fs::remove_file(&tmp_bz2);
                continue;
            }
        };
        let mut out = match std::fs::File::create(&dest) {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_bz2);
                return Err(format!("create MANIFEST: {e}"));
            }
        };
        let copied = child
            .stdout
            .take()
            .ok_or_else(|| "bzip2 produced no output".to_string())
            .and_then(|mut s| download::capped_copy(&mut s, &mut out, download::MAX_DOWNLOAD));
        let status = child.wait();
        let _ = std::fs::remove_file(&tmp_bz2);
        if copied.is_ok() && matches!(status, Ok(s) if s.success()) {
            println!("  MANIFEST for '{}' ready", repo.name);
            return Ok(true);
        }
        let _ = std::fs::remove_file(&dest);
    }

    let _ = std::fs::remove_file(&tmp_bz2);
    println!("  no MANIFEST available for '{}'", repo.name);
    Ok(false)
}

/// For the official Slackware repo only: the MANIFEST.bz2 sits in the per-arch
/// package dir (`slackware64/` on 64-bit, `slackware/` on 32-bit), one level
/// below the distribution-root URL. Recover that dir name from the cached
/// PACKAGES.TXT PACKAGE LOCATIONs (e.g. "./slackware64/l" -> "slackware64").
fn official_arch_subdir(cache_dir: &Path) -> Option<String> {
    let text = read_text_lossy(&cache_dir.join(PACKAGES_TXT)).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("PACKAGE LOCATION:") {
            let top = rest
                .trim()
                .trim_start_matches("./")
                .trim_matches('/')
                .split('/')
                .next()
                .unwrap_or("");
            if top == "slackware64" || top == "slackware" {
                return Some(top.to_string());
            }
        }
    }
    None
}

/// True if a package of `pkg_arch` belongs in a repo for system `arch`.
///
/// Beyond the native arch and `noarch`, a standard Slackware repo legitimately
/// ships `fw` (firmware packages) and `x86` (32-bit headers such as
/// kernel-headers, present even on x86_64). These must not be filtered out, or
/// clean-system would wrongly flag them as foreign. Genuine 32-bit binary
/// arches (`i586`/`i686`) are still excluded on a 64-bit system.
/// Is a package built for `pkg_arch` usable on a system of `arch`? `noarch`,
/// firmware (`fw`) and the kernel-headers `x86` arch ship on every Slackware
/// arch, so they are always kept. Otherwise the arch FAMILIES must match: all
/// 32-bit x86 variants (i386/i486/i586/i686) are one family, so an i686 package
/// loads on an i586-detected base and vice versa. Without this, the whole 32-bit
/// tree was dropped — the base detects as i586 while the kernel and most
/// packages are i686, and an exact-match filter discarded them.
fn arch_compatible(pkg_arch: &str, arch: &str) -> bool {
    pkg_arch == "noarch"
        || pkg_arch == "fw"
        || pkg_arch == "x86"
        || arch_family(pkg_arch) == arch_family(arch)
}

/// Arch family for the available-list filter: the 32-bit x86 variants collapse
/// to one group; every other token compares as-is.
fn arch_family(a: &str) -> &str {
    match a {
        "i386" | "i486" | "i586" | "i686" => "x86_32",
        other => other,
    }
}

/// Read a file as text, replacing any invalid UTF-8 bytes rather than failing.
/// Slackware PACKAGES.TXT/CHECKSUMS are not guaranteed to be valid UTF-8
/// (maintainer names/descriptions may carry Latin-1 bytes).
pub(crate) fn read_text_lossy(path: &std::path::Path) -> std::io::Result<String> {
    std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Load and parse a repo's cached metadata into a list of packages.
pub fn load_repo(repo: &Repo, cache_root: &Path, arch: &str) -> Result<Vec<AvailPkg>, String> {
    let dir = repo.cache_subdir(cache_root);
    let pkg_text = read_text_lossy(&dir.join(PACKAGES_TXT)).map_err(|e| {
        format!("missing metadata for repo '{}' ({e}); run `slacker update` first", repo.name)
    })?;

    let md5_map = match read_text_lossy(&dir.join(CHECKSUMS)) {
        Ok(s) => parse_checksums(&s),
        Err(_) => HashMap::new(),
    };
    // SHA-256 checksums are optional and absent from current repos; if a repo
    // ships CHECKSUMS.sha256, slacker picks it up here.
    let sha_map = match read_text_lossy(&dir.join(CHECKSUMS_SHA)) {
        Ok(s) => parse_checksums_len(&s, 64),
        Err(_) => HashMap::new(),
    };

    let mut out = parse_packages_txt(&pkg_text, &repo.name);
    out.retain(|p| arch_compatible(&p.id.arch, arch));
    for p in out.iter_mut() {
        if let Some(m) = md5_map.get(&p.filename) {
            p.md5 = Some(m.clone());
        }
        if let Some(s) = sha_map.get(&p.filename) {
            p.sha = Some(s.clone());
        }
    }
    Ok(out)
}

/// Package *names* present in a repo's *previous* PACKAGES snapshot.
///
/// `install-new` uses this to detect a genuinely new package — one whose *name*
/// did not exist before — rather than a new build or version of a package that
/// already existed (which only changes the filename and is an upgrade, not a
/// new package). Names are recovered by parsing each `PACKAGE NAME` filename.
// NOTE: as of the install-new behaviour change, `install-new` no longer diffs
// against PACKAGES.TXT.prev — it now offers every official package that is not
// installed (catching removed packages too), so this helper is currently unused.
// It is kept on purpose: PACKAGES.TXT/.prev is core to slacker and this name-diff
// may be wanted again (e.g. a "what did the last update add" report) or elsewhere.
#[allow(dead_code)]
pub fn previous_names(
    repo: &Repo,
    cache_root: &Path,
) -> Option<std::collections::HashSet<String>> {
    let dir = repo.cache_subdir(cache_root);
    let text = read_text_lossy(&dir.join(PACKAGES_PREV)).ok()?;
    Some(
        text.lines()
            .filter_map(|l| l.strip_prefix("PACKAGE NAME:"))
            .filter_map(|s| PkgId::parse(s.trim()).map(|id| id.name))
            .collect(),
    )
}

pub fn meta_path(repo: &Repo, cache_root: &Path, fname: &str) -> PathBuf {
    repo.cache_subdir(cache_root).join(fname)
}

/// Drop a repo's downloaded integrity metadata from the cache so unverified
/// data can never be used. Called when GPG verification fails: the repo is
/// treated as "not updated" until a later successful update (or relaxed
/// verification). PACKAGES.TXT goes too, so the repo's packages fall out of the
/// database entirely rather than risk being installed unverified.
pub fn invalidate_metadata(repo: &Repo, cache_root: &Path) {
    for f in [PACKAGES_TXT, CHECKSUMS, CHECKSUMS_ASC, CHECKSUMS_SHA] {
        let _ = std::fs::remove_file(meta_path(repo, cache_root, f));
    }
}

/// Directory holding repo-quarantine markers.
fn quarantine_dir(state_root: &Path) -> PathBuf {
    state_root.join("quarantine")
}

fn quarantine_path(state_root: &Path, repo_name: &str) -> PathBuf {
    quarantine_dir(state_root).join(repo_name)
}

/// True if a repo has been quarantined (failed safety vetting). A quarantined
/// repo is treated as an inert source: it provides no packages, until it is
/// cleared. SOFT quarantine (unreachable) is retried automatically by the next
/// update; HARD quarantine (malicious / bad signature / manual distrust) stays
/// until the user runs `trust-repo`.
pub fn is_quarantined(state_root: &Path, repo_name: &str) -> bool {
    quarantine_path(state_root, repo_name).exists()
}

/// Whether a quarantine is "soft" (unreachable, auto-retried) or "hard"
/// (actively distrusted, needs explicit `trust-repo`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum QuarantineKind {
    Soft,
    Hard,
}

/// Parse a quarantine marker: first line is the kind tag, the rest is the
/// human-readable reason. Markers without a recognised tag are treated as soft.
fn read_quarantine(state_root: &Path, repo_name: &str) -> Option<(QuarantineKind, String)> {
    let raw = std::fs::read_to_string(quarantine_path(state_root, repo_name)).ok()?;
    let mut it = raw.splitn(2, '\n');
    let kind = match it.next().unwrap_or("").trim() {
        "hard" => QuarantineKind::Hard,
        _ => QuarantineKind::Soft,
    };
    let reason = it.next().unwrap_or("").trim().to_string();
    Some((kind, reason))
}

/// True only for a HARD quarantine (the next update will NOT auto-retry it).
pub fn is_hard_quarantined(state_root: &Path, repo_name: &str) -> bool {
    matches!(read_quarantine(state_root, repo_name), Some((QuarantineKind::Hard, _)))
}

/// The recorded reason a repo was quarantined (for display), if any.
pub fn quarantine_reason(state_root: &Path, repo_name: &str) -> Option<String> {
    read_quarantine(state_root, repo_name).map(|(_, r)| r)
}

/// Quarantine a repo, recording the kind and why. Its cached integrity metadata
/// is dropped too, so nothing of it can be used while quarantined. A quarantined
/// repo is never simultaneously "trusted".
pub fn quarantine(
    repo: &Repo,
    cache_root: &Path,
    state_root: &Path,
    kind: QuarantineKind,
    reason: &str,
) -> Result<(), String> {
    let dir = quarantine_dir(state_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tag = match kind {
        QuarantineKind::Soft => "soft",
        QuarantineKind::Hard => "hard",
    };
    std::fs::write(quarantine_path(state_root, &repo.name), format!("{tag}\n{reason}"))
        .map_err(|e| format!("write quarantine marker: {e}"))?;
    unmark_trusted(state_root, &repo.name);
    // The quarantine MARKER is state; the metadata it invalidates is cache.
    invalidate_metadata(repo, cache_root);
    Ok(())
}

/// Lift a repo's quarantine.
pub fn clear_quarantine(state_root: &Path, repo_name: &str) {
    let _ = std::fs::remove_file(quarantine_path(state_root, repo_name));
}

/// Directory holding "vetted/trusted" markers. A repo is trusted once it has
/// passed vetting (or the user ran `trust-repo`): update then uses it normally
/// and a transient fetch failure does NOT quarantine it. Untrusted repos (newly
/// added, never vetted) are vetted on first update.
fn trusted_dir(state_root: &Path) -> PathBuf {
    state_root.join("trusted")
}

fn trusted_path(state_root: &Path, repo_name: &str) -> PathBuf {
    trusted_dir(state_root).join(repo_name)
}

pub fn is_trusted(state_root: &Path, repo_name: &str) -> bool {
    trusted_path(state_root, repo_name).exists()
}

pub fn mark_trusted(state_root: &Path, repo_name: &str) {
    let dir = trusted_dir(state_root);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(trusted_path(state_root, repo_name), "");
    }
}

pub fn unmark_trusted(state_root: &Path, repo_name: &str) {
    let _ = std::fs::remove_file(trusted_path(state_root, repo_name));
}


fn series_from_location(location: &str) -> String {
    location
        .trim_start_matches("./")
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn parse_packages_txt(text: &str, repo_name: &str) -> Vec<AvailPkg> {
    let mut out = Vec::new();
    let mut cur_name: Option<String> = None;
    let mut cur_loc = String::new();
    let mut cur_size: Option<u64> = None;
    let mut cur_size_unc: Option<u64> = None;
    let mut cur_summary = String::new();
    let mut cur_desc = String::new();
    let mut cur_required: Vec<String> = Vec::new();
    let mut in_desc = false;

    #[allow(clippy::too_many_arguments)]
    let flush = |out: &mut Vec<AvailPkg>,
                 name: &Option<String>,
                 loc: &str,
                 size: Option<u64>,
                 size_unc: Option<u64>,
                 summary: &str,
                 desc: &str,
                 required: &[String]| {
        if let Some(filename) = name {
            // Reject path-like filenames/locations from the repo before they can
            // ever reach a filesystem path or URL (see pkg::is_safe_filename).
            if !crate::pkg::is_safe_filename(filename) || !crate::pkg::is_safe_location(loc) {
                return;
            }
            if let Some(id) = PkgId::parse(filename) {
                out.push(AvailPkg {
                    id,
                    filename: filename.clone(),
                    location: if loc.is_empty() { "./".into() } else { loc.to_string() },
                    series: series_from_location(loc),
                    size_k: size,
                    size_uncompressed_k: size_unc,
                    summary: summary.trim().to_string(),
                    description: desc.trim_end().to_string(),
                    md5: None,
                    sha: None,
                    required: required.to_vec(),
                    repo: repo_name.to_string(),
                });
            }
        }
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("PACKAGE NAME:") {
            flush(
                &mut out, &cur_name, &cur_loc, cur_size, cur_size_unc, &cur_summary, &cur_desc,
                &cur_required,
            );
            cur_name = Some(rest.trim().to_string());
            cur_loc.clear();
            cur_size = None;
            cur_size_unc = None;
            cur_summary.clear();
            cur_desc.clear();
            cur_required.clear();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("PACKAGE LOCATION:") {
            cur_loc = rest.trim().to_string();
            if !cur_loc.ends_with('/') {
                cur_loc.push('/');
            }
        } else if let Some(rest) = line.strip_prefix("PACKAGE REQUIRED:") {
            // slapt-get / SlackBuilds repos declare deps here (vanilla Slackware
            // trees omit it). Additive: parsed only when present.
            cur_required = parse_required(rest);
        } else if let Some(rest) = line.strip_prefix("PACKAGE SIZE (compressed):") {
            cur_size = rest.trim().split_whitespace().next().and_then(|n| n.parse().ok());
        } else if let Some(rest) = line.strip_prefix("PACKAGE SIZE (uncompressed):") {
            cur_size_unc = rest.trim().split_whitespace().next().and_then(|n| n.parse().ok());
        } else if line.starts_with("PACKAGE DESCRIPTION:") {
            in_desc = true;
        } else if in_desc {
            // Description lines look like "name: text"; strip the prefix.
            let content = match line.find(':') {
                Some(idx) => line[idx + 1..].trim_start(),
                None => line,
            };
            if cur_summary.is_empty() && !content.is_empty() {
                cur_summary = content.to_string();
            }
            cur_desc.push_str(content);
            cur_desc.push('\n');
        }
    }
    flush(
        &mut out, &cur_name, &cur_loc, cur_size, cur_size_unc, &cur_summary, &cur_desc,
        &cur_required,
    );
    out
}

fn parse_checksums(text: &str) -> HashMap<String, String> {
    parse_checksums_len(text, 32)
}

/// True if two CHECKSUMS.md5 bodies list the same packages with the same md5s.
///
/// Compares only the per-package checksum entries, so it is immune to header
/// lines, generation timestamps, comments, blank lines, ordering and transport
/// or whitespace noise — any of which can make a byte-for-byte comparison
/// wrongly report a change (e.g. a mirror that regenerates the file with a fresh
/// header on every request). Used by `check-updates`.
pub fn checksums_equal(a: &str, b: &str) -> bool {
    parse_checksums(a) == parse_checksums(b)
}

/// Parse a CHECKSUMS-style file mapping filename -> hash, keeping only entries
/// whose hash is exactly `hexlen` characters (32 for md5, 64 for sha256).
fn parse_checksums_len(text: &str, hexlen: usize) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let (Some(hash), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        if hash.len() != hexlen {
            continue;
        }
        if let Some(fname) = path.trim().rsplit('/').next() {
            if fname.ends_with(".txz") || fname.ends_with(".tgz") {
                map.insert(fname.to_string(), hash.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
PACKAGE NAME:  bash-5.2.21-x86_64-3.txz
PACKAGE LOCATION:  ./slackware64/a
PACKAGE SIZE (compressed):  1820 K
PACKAGE DESCRIPTION:
bash: bash (sh-compatible shell)

PACKAGE NAME:  xfce4-panel-4.18.6-x86_64-1.txz
PACKAGE LOCATION:  ./slackware64/xfce
PACKAGE DESCRIPTION:
xfce4-panel: xfce4-panel (panel for Xfce)
";

    #[test]
    fn parses_blocks_with_series() {
        let pkgs = parse_packages_txt(SAMPLE, "slackware");
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].id.name, "bash");
        assert_eq!(pkgs[0].series, "a");
        assert_eq!(pkgs[1].series, "xfce");
        // Vanilla Slackware PACKAGES.TXT has no PACKAGE REQUIRED -> no auto-deps.
        assert!(pkgs[0].required.is_empty());
        assert!(pkgs[1].required.is_empty());
    }

    #[test]
    fn captures_package_required_when_present() {
        // slapt-get / SlackBuilds style metadata: deps declared in PACKAGES.TXT.
        let sample = "\
PACKAGE NAME:  flatpak-1.18.0-x86_64-1alien.txz
PACKAGE LOCATION:  ./
PACKAGE REQUIRED:  bubblewrap,libostree,xdg-dbus-proxy
PACKAGE DESCRIPTION:
flatpak: flatpak (application sandboxing)

PACKAGE NAME:  bubblewrap-0.11.2-x86_64-2alien.txz
PACKAGE LOCATION:  ./
PACKAGE DESCRIPTION:
bubblewrap: bubblewrap (sandboxing tool)
";
        let pkgs = parse_packages_txt(sample, "alienbob");
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].id.name, "flatpak");
        assert_eq!(pkgs[0].required, vec!["bubblewrap", "libostree", "xdg-dbus-proxy"]);
        // A package in the same repo without the field keeps empty deps.
        assert_eq!(pkgs[1].id.name, "bubblewrap");
        assert!(pkgs[1].required.is_empty());
    }

    #[test]
    fn series_extraction() {
        assert_eq!(series_from_location("./slackware64/ap/"), "ap");
        assert_eq!(series_from_location("./patches/packages/"), "packages");
        assert_eq!(series_from_location("./"), "");
    }

    #[test]
    fn official_arch_subdir_picks_arch_dir() {
        let dir = std::env::temp_dir().join("slacker_archsub_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 64-bit: locations under ./slackware64/, with extras present too
        std::fs::write(
            dir.join(PACKAGES_TXT),
            "PACKAGE LOCATION:  ./extra/foo\nPACKAGE LOCATION:  ./slackware64/a\n",
        )
        .unwrap();
        assert_eq!(official_arch_subdir(&dir), Some("slackware64".to_string()));
        // 32-bit naming
        std::fs::write(dir.join(PACKAGES_TXT), "PACKAGE LOCATION:  ./slackware/a\n").unwrap();
        assert_eq!(official_arch_subdir(&dir), Some("slackware".to_string()));
        // a flat repo (no slackware*/ toplevel) yields none
        std::fs::write(dir.join(PACKAGES_TXT), "PACKAGE LOCATION:  ./pkg/a\n").unwrap();
        assert_eq!(official_arch_subdir(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_checksums_ok() {
        let cs = "d41d8cd98f00b204e9800998ecf8427e  ./a/bash-5.2.21-x86_64-3.txz\nbad\n";
        let m = parse_checksums(cs);
        assert_eq!(m.len(), 1);
    }
}

/// Fetch and parse the `.dep` file that sits next to a package in the repo, if
/// present. Each non-comment line names one dependency package. The `.dep`
/// shares the package's stem (its `name-version-arch-build` tag) with a `.dep`
/// extension, in the same location. A missing file (404) yields no deps.
pub fn fetch_dep(repo: &Repo, avail: &AvailPkg) -> Vec<String> {
    let rel = format!("{}{}.dep", avail.location, avail.id.tag());
    let url = repo.join_url(&rel);
    let from_dep = match crate::download::get_bytes(&url) {
        Ok(bytes) => parse_dep(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    };
    // A per-package `.dep` is authoritative. Only when the repo ships none (the
    // fetch 404s or is empty) do we fall back to the names declared in
    // PACKAGES.TXT via `PACKAGE REQUIRED:` (slapt-get / SlackBuilds repos). For
    // vanilla Slackware trees both are empty -> no auto-deps, which is correct.
    if from_dep.is_empty() {
        avail.required.clone()
    } else {
        from_dep
    }
}

/// Parse a `PACKAGE REQUIRED:` value: a comma-separated list of dependency
/// package NAMES. On Slackware repos these are plain names; the slapt-get spec
/// also permits a trailing version constraint (`name >= 1.2`) and `a|b`
/// alternatives, so we defensively drop the constraint and take the first
/// alternative. `%README%` and blanks are skipped.
fn parse_required(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|tok| {
            let tok = tok.trim();
            // First alternative of `a|b|c`.
            let tok = tok.split('|').next().unwrap_or(tok).trim();
            // Drop a trailing version constraint: name>=1.2, name = 1.2, name<3 ...
            tok.split(|c: char| matches!(c, '>' | '<' | '=' | ' ' | '\t'))
                .next()
                .unwrap_or(tok)
                .trim()
        })
        .filter(|s| !s.is_empty() && *s != "%README%")
        .map(String::from)
        .collect()
}

/// Parse `.dep` contents: one dependency package name per line, `#` comments
/// and blank lines ignored.
pub fn parse_dep(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| match l.find('#') {
            Some(i) => &l[..i],
            None => l,
        })
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod dep_tests {
    use super::parse_dep;

    #[test]
    fn parses_names_and_skips_noise() {
        let text = "aubio\n# a comment\n\nliblo   \nlilv # inline\n";
        assert_eq!(parse_dep(text), vec!["aubio", "liblo", "lilv"]);
    }

    #[test]
    fn parse_required_is_format_tolerant() {
        use super::parse_required;
        // The common case on Slackware repos: plain, comma-separated names.
        assert_eq!(
            parse_required("bubblewrap,libostree,xdg-dbus-proxy"),
            vec!["bubblewrap", "libostree", "xdg-dbus-proxy"]
        );
        // Defensive against the slapt spec: version constraints dropped, the
        // first `a|b` alternative taken, %README% and blanks skipped, spaces ok.
        assert_eq!(
            parse_required("glibc >= 2.2, foo|bar, %README%, , baz<3"),
            vec!["glibc", "foo", "baz"]
        );
        assert!(parse_required("").is_empty());
    }
}

#[cfg(test)]
mod arch_tests {
    use super::arch_compatible;

    #[test]
    fn accepts_native_noarch_fw_x86() {
        assert!(arch_compatible("x86_64", "x86_64"));
        assert!(arch_compatible("noarch", "x86_64"));
        assert!(arch_compatible("fw", "x86_64"));   // firmware (ipw2200-fw, zd1211-firmware)
        assert!(arch_compatible("x86", "x86_64"));   // 32-bit headers (kernel-headers)
    }

    #[test]
    fn rejects_genuine_32bit_binaries_on_64() {
        assert!(!arch_compatible("i586", "x86_64"));
        assert!(!arch_compatible("i686", "x86_64"));
        assert!(!arch_compatible("aarch64", "x86_64"));
    }

    #[test]
    fn keeps_whole_32bit_family_on_32bit() {
        // The base detects as i586 but the kernel and many packages are i686;
        // both must stay in the available list (this was the 32-bit bug).
        assert!(arch_compatible("i586", "i586"));
        assert!(arch_compatible("i686", "i586")); // i686 kernel on i586 base
        assert!(arch_compatible("i586", "i686"));
        assert!(arch_compatible("noarch", "i586"));
        assert!(arch_compatible("x86", "i586")); // kernel-headers
        assert!(arch_compatible("fw", "i686")); // firmware
        // A genuine 64-bit package still does not belong on a 32-bit system.
        assert!(!arch_compatible("x86_64", "i586"));
    }
}

#[cfg(test)]
mod lossy_tests {
    use super::read_text_lossy;

    #[test]
    fn reads_non_utf8_without_failing() {
        // PACKAGES.TXT may carry Latin-1 bytes (e.g. 0xE9 = é in a name).
        let dir = std::env::temp_dir();
        let path = dir.join("slacker_lossy_test.txt");
        std::fs::write(&path, b"PACKAGE NAME:  caf\xe9-1.0-x86_64-1.txz\n").unwrap();
        let text = read_text_lossy(&path).unwrap();
        assert!(text.contains("PACKAGE NAME:"));
        assert!(text.contains("1.0-x86_64-1")); // parsed despite the bad byte
        let _ = std::fs::remove_file(&path);
    }
}
