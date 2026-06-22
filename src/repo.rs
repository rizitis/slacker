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
    pub repo: String,
}

impl AvailPkg {
    pub fn url(&self, repo: &Repo) -> String {
        repo.join_url(&format!("{}{}", self.location, self.filename))
    }
}

/// Names of the metadata files we keep per repo.
pub const PACKAGES_TXT: &str = "PACKAGES.TXT";
pub const PACKAGES_PREV: &str = "PACKAGES.TXT.prev";
pub const CHECKSUMS: &str = "CHECKSUMS.md5";
pub const CHECKSUMS_ASC: &str = "CHECKSUMS.md5.asc";
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
/// MANIFEST.bz2 is special: on official Slackware mirrors it lives not in the
/// repo root but inside the per-arch subdir (slackware64/, slackware/, ...),
/// while package LOCATIONs are relative to the root. We therefore look for it
/// both at the root and inside every top-level subdir referenced by the
/// package locations, decompress whatever we find, and concatenate it into one
/// plain-text MANIFEST that file-search can read directly.
pub fn update_repo(repo: &Repo, cache_root: &Path, fetch_changelog: bool) -> Result<(), String> {
    use std::io::Write;
    let dir = repo.cache_subdir(cache_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;

    let pkgs_path = dir.join(PACKAGES_TXT);
    if pkgs_path.exists() {
        let _ = std::fs::copy(&pkgs_path, dir.join(PACKAGES_PREV));
    }

    for fname in [PACKAGES_TXT, CHECKSUMS] {
        print!("  {fname} ... ");
        std::io::stdout().flush().ok();
        let url = repo.join_url(fname);
        let bytes = download::get_bytes(&url).map_err(|e| format!("fetch {url}: {e}"))?;
        std::fs::write(dir.join(fname), &bytes)
            .map_err(|e| format!("write {fname}: {e}"))?;
        println!("ok");
    }

    // The GPG signature is fetched for every repo (each is signed separately).
    // The ChangeLog is only tracked for the official repo (check-updates /
    // show-changelog use that one), so we skip it elsewhere — external repos
    // keep their ChangeLog in varying locations/formats and we don't chase them.
    let mut meta = vec![CHECKSUMS_ASC];
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

/// Ensure the decompressed MANIFEST exists for a repo, downloading it on
/// demand. Used by file-search. MANIFEST.bz2 is streamed to disk (not held in
/// memory) and decompressed straight into the MANIFEST file, concatenating the
/// root and every per-arch subdir referenced by the package locations.
pub fn ensure_manifest(repo: &Repo, cache_root: &Path) -> Result<bool, String> {
    use std::io::Write;
    let dir = repo.cache_subdir(cache_root);
    let dest = dir.join(MANIFEST);
    if dest.exists() {
        return Ok(true);
    }

    let mut subdirs: Vec<String> = vec![String::new()]; // "" == repo root
    if let Ok(text) = read_text_lossy(&dir.join(PACKAGES_TXT)) {
        for d in toplevel_location_dirs(&text) {
            if !subdirs.contains(&d) {
                subdirs.push(d);
            }
        }
    }

    print!("  fetching MANIFEST for '{}' (large, first time only) ... ", repo.name);
    std::io::stdout().flush().ok();

    let tmp_bz2 = dir.join("MANIFEST.bz2.part");
    let mut out = std::fs::File::create(&dest).map_err(|e| format!("create MANIFEST: {e}"))?;
    let mut wrote_any = false;

    for sub in &subdirs {
        let rel = if sub.is_empty() {
            MANIFEST_BZ2.to_string()
        } else {
            format!("{sub}/{MANIFEST_BZ2}")
        };
        let url = repo.join_url(&rel);
        if download::download_to(&url, &tmp_bz2).is_err() {
            continue;
        }
        // Decompress directly into the MANIFEST file (no big in-memory buffer).
        let status = std::process::Command::new("bzip2")
            .arg("-dc")
            .arg(&tmp_bz2)
            .stdout(std::process::Stdio::from(
                out.try_clone().map_err(|e| e.to_string())?,
            ))
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_file(&tmp_bz2);
        if matches!(status, Ok(s) if s.success()) {
            wrote_any = true;
            out.write_all(b"\n").ok();
        }
    }

    if wrote_any {
        println!("ok");
        Ok(true)
    } else {
        drop(out);
        let _ = std::fs::remove_file(&dest);
        println!("none available");
        Ok(false)
    }
}

/// Distinct top-level directory names from PACKAGE LOCATION lines.
/// "./slackware64/d" -> "slackware64".
fn toplevel_location_dirs(packages_txt: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for line in packages_txt.lines() {
        if let Some(rest) = line.strip_prefix("PACKAGE LOCATION:") {
            let loc = rest.trim().trim_start_matches("./").trim_matches('/');
            if let Some(top) = loc.split('/').next() {
                if !top.is_empty() && !seen.contains(&top.to_string()) {
                    seen.push(top.to_string());
                }
            }
        }
    }
    seen
}

/// True if a package of `pkg_arch` belongs in a repo for system `arch`.
///
/// Beyond the native arch and `noarch`, a standard Slackware repo legitimately
/// ships `fw` (firmware packages) and `x86` (32-bit headers such as
/// kernel-headers, present even on x86_64). These must not be filtered out, or
/// clean-system would wrongly flag them as foreign. Genuine 32-bit binary
/// arches (`i586`/`i686`) are still excluded on a 64-bit system.
fn arch_compatible(pkg_arch: &str, arch: &str) -> bool {
    pkg_arch == arch || pkg_arch == "noarch" || pkg_arch == "fw" || pkg_arch == "x86"
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

    let mut out = parse_packages_txt(&pkg_text, &repo.name);
    out.retain(|p| arch_compatible(&p.id.arch, arch));
    for p in out.iter_mut() {
        if let Some(m) = md5_map.get(&p.filename) {
            p.md5 = Some(m.clone());
        }
    }
    Ok(out)
}

/// Filenames present in a repo's *previous* PACKAGES snapshot (for diffing).
pub fn previous_filenames(
    repo: &Repo,
    cache_root: &Path,
) -> Option<std::collections::HashSet<String>> {
    let dir = repo.cache_subdir(cache_root);
    let text = read_text_lossy(&dir.join(PACKAGES_PREV)).ok()?;
    Some(
        text.lines()
            .filter_map(|l| l.strip_prefix("PACKAGE NAME:"))
            .map(|s| s.trim().to_string())
            .collect(),
    )
}

pub fn meta_path(repo: &Repo, cache_root: &Path, fname: &str) -> PathBuf {
    repo.cache_subdir(cache_root).join(fname)
}

/// Series from a location like "./slackware64/ap/" -> "ap".
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
    let mut in_desc = false;

    #[allow(clippy::too_many_arguments)]
    let flush = |out: &mut Vec<AvailPkg>,
                 name: &Option<String>,
                 loc: &str,
                 size: Option<u64>,
                 size_unc: Option<u64>,
                 summary: &str,
                 desc: &str| {
        if let Some(filename) = name {
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
                    repo: repo_name.to_string(),
                });
            }
        }
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("PACKAGE NAME:") {
            flush(&mut out, &cur_name, &cur_loc, cur_size, cur_size_unc, &cur_summary, &cur_desc);
            cur_name = Some(rest.trim().to_string());
            cur_loc.clear();
            cur_size = None;
            cur_size_unc = None;
            cur_summary.clear();
            cur_desc.clear();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("PACKAGE LOCATION:") {
            cur_loc = rest.trim().to_string();
            if !cur_loc.ends_with('/') {
                cur_loc.push('/');
            }
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
    flush(&mut out, &cur_name, &cur_loc, cur_size, cur_size_unc, &cur_summary, &cur_desc);
    out
}

fn parse_checksums(text: &str) -> HashMap<String, String> {
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
        if hash.len() != 32 {
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
    }

    #[test]
    fn series_extraction() {
        assert_eq!(series_from_location("./slackware64/ap/"), "ap");
        assert_eq!(series_from_location("./patches/packages/"), "packages");
        assert_eq!(series_from_location("./"), "");
    }

    #[test]
    fn toplevel_dirs_extracted() {
        let txt = "PACKAGE LOCATION:  ./slackware64/a\nPACKAGE LOCATION:  ./slackware64/n\nPACKAGE LOCATION:  ./extra/foo\n";
        assert_eq!(toplevel_location_dirs(txt), vec!["slackware64", "extra"]);
        // 32-bit naming works the same
        let txt32 = "PACKAGE LOCATION:  ./slackware/a\n";
        assert_eq!(toplevel_location_dirs(txt32), vec!["slackware"]);
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
    match crate::download::get_bytes(&url) {
        Ok(bytes) => parse_dep(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
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
