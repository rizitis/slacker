//! Integration with the live Slackware system: reading the installed-package
//! database and shelling out to the native pkgtools.
//!
//! On current Slackware the installed-package database lives in
//! `/var/lib/pkgtools/packages` (configurable via PKG_DB_DIR).

use crate::pkg::PkgId;
use std::path::{Path, PathBuf};
use std::process::Command;

/// List currently installed packages from the package DB directory.
pub fn installed_packages(db_dir: &Path) -> Result<Vec<PkgId>, String> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(db_dir) {
        Ok(e) => e,
        Err(_) => return Ok(out), // no DB => treat as empty, don't fail
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            // Skip removed-package records: `upgradepkg` leaves entries named
            // `<pkg>-upgraded-<timestamp>` (the timestamp starts with a digit).
            // These are not live installed packages; if one is left behind in
            // the DB dir (e.g. after an interrupted/failed upgrade) it must not
            // be reported as installed, or it pollutes source attribution and
            // makes clean-system propose phantom removals.
            if is_removed_record(name) {
                continue;
            }
            if let Some(id) = PkgId::parse(name) {
                out.push(id);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// True if the installed package-database directory `pkg_db_dir` contains a
/// record for `record_name` (a full package id with no extension, e.g.
/// `aaa_glibc-solibs-2.42-x86_64-1`). Used as a post-install sanity check on the
/// dist core: `upgradepkg`/`installpkg` create this record on success, so its
/// absence after an install means the install did not take (e.g. the disk filled
/// during extraction) — letting the dist abort instead of marching on over a core
/// package that is not actually in place. This reads system state only; it does
/// not parse or second-guess pkgtools output.
pub fn record_present(pkg_db_dir: &Path, record_name: &str) -> bool {
    pkg_db_dir.join(record_name).exists()
}

/// True if `name` is an `upgradepkg` removed-package record
/// (`<pkg>-upgraded-<timestamp>`, the timestamp beginning with a digit), rather
/// than a live installed-package entry.
fn is_removed_record(name: &str) -> bool {
    match name.rsplit_once("-upgraded-") {
        Some((_, suffix)) => suffix.starts_with(|c: char| c.is_ascii_digit()),
        None => false,
    }
}

pub fn installed_by_name<'a>(installed: &'a [PkgId], name: &str) -> Option<&'a PkgId> {
    installed.iter().find(|p| p.name == name)
}

pub fn is_installed(installed: &[PkgId], name: &str) -> bool {
    installed.iter().any(|p| p.name == name)
}

/// Upgrade only (do not install if absent): plain `upgradepkg`.
pub fn upgrade_only(pkg_file: &Path) -> Result<(), String> {
    run("upgradepkg", &[&pkg_file.to_string_lossy()])
}

/// Reinstall the same version: `upgradepkg --reinstall`.
pub fn reinstall(pkg_file: &Path) -> Result<(), String> {
    run("upgradepkg", &["--reinstall", &pkg_file.to_string_lossy()])
}

/// Fresh install: `installpkg`.
pub fn install(pkg_file: &Path) -> Result<(), String> {
    run("installpkg", &[&pkg_file.to_string_lossy()])
}

/// Upgrade, installing the package even if no matching package is currently
/// installed: `upgradepkg --install-new`. This is what a distribution upgrade
/// needs for the core set, because packages get renamed across releases
/// (e.g. 15.0 `glibc-solibs` becomes -current `aaa_glibc-solibs`): a plain
/// `upgradepkg` would skip the new name as "not installed", whereas
/// `--install-new` installs it. Mirrors the official Slackware upgrade path.
pub fn upgrade_install_new(pkg_file: &Path) -> Result<(), String> {
    run(
        "upgradepkg",
        &["--install-new", &pkg_file.to_string_lossy()],
    )
}

/// Read `VERSION_CODENAME` from `/etc/os-release` (e.g. `current` on a -current
/// system). Returns None if the file is missing or the key is absent/empty;
/// callers treat that as "not -current" and refuse, i.e. fail-closed. The value
/// may be quoted or bare, so surrounding quotes are stripped.
pub fn version_codename() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VERSION_CODENAME=") {
            let v = rest.trim().trim_matches('"').trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Read `VERSION_ID` from `/etc/os-release` (e.g. `15.0` on Slackware 15.0).
/// Returns None if the file is missing or the key is absent/empty. Like
/// `version_codename`, the value may be quoted or bare, so surrounding quotes
/// are stripped. upgrade-dist uses this together with the codename to identify
/// the running release (codename `current` wins, since on -current VERSION_ID
/// may read like `15.0+`).
pub fn version_id() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VERSION_ID=") {
            let v = rest.trim().trim_matches('"').trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Remove an installed package by name (or tag): `removepkg`.
pub fn remove_package(name: &str) -> Result<(), String> {
    if name.starts_with('-') || name.contains('/') {
        return Err(format!("refusing to remove suspicious package name {name:?}"));
    }
    run("removepkg", &[name])
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("failed to launch {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

pub fn cached_pkg_path(cache_root: &Path, repo: &str, filename: &str) -> PathBuf {
    cache_root.join("packages").join(repo).join(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fake_installed_db() {
        let dir = std::env::temp_dir().join("slacker_test_pkgdb2");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for tag in ["bash-5.2.21-x86_64-3", "vim-9.1.0-x86_64-1"] {
            std::fs::File::create(dir.join(tag)).unwrap();
        }
        // A leftover removed-package record (from an interrupted upgradepkg)
        // must NOT be read as an installed package.
        std::fs::File::create(
            dir.join("glibc-2.33-x86_64-9_slack15.0-upgraded-2026-06-26,20:40:31"),
        )
        .unwrap();
        let pkgs = installed_packages(&dir).unwrap();
        assert_eq!(pkgs.len(), 2);
        assert!(is_installed(&pkgs, "bash"));
        assert!(!is_installed(&pkgs, "nope"));
        // the -upgraded- record did not sneak in
        assert!(!is_installed(&pkgs, "glibc"));
    }

    #[test]
    fn removed_records_are_detected() {
        assert!(is_removed_record(
            "tar-1.34-x86_64-2_slack15.0-upgraded-2026-06-26,20:41:01"
        ));
        assert!(is_removed_record("foo-1-x86_64-1-upgraded-2026-06-26,10:00:00"));
        // a real package name with no removed marker
        assert!(!is_removed_record("bash-5.2.21-x86_64-3"));
        // defensive: a literal "-upgraded-" not followed by a digit is not a record
        assert!(!is_removed_record("my-upgraded-tool-1.0-x86_64-1"));
    }

    #[test]
    fn missing_db_is_empty_not_error() {
        let dir = std::env::temp_dir().join("slacker_nonexistent_db_xyz2");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(installed_packages(&dir).unwrap().len(), 0);
    }

    #[test]
    fn record_present_detects_db_record() {
        let dir = std::env::temp_dir().join("slacker_test_record_present");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::File::create(dir.join("aaa_glibc-solibs-2.42-x86_64-1")).unwrap();
        // the record derived from a .txz filename (extension stripped) is found
        assert!(record_present(&dir, "aaa_glibc-solibs-2.42-x86_64-1"));
        // a package whose install did not land has no record
        assert!(!record_present(&dir, "pkgtools-15.1-noarch-32"));
    }
}
