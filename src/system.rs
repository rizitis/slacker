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
            if let Some(id) = PkgId::parse(name) {
                out.push(id);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
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

/// Remove an installed package by name (or tag): `removepkg`.
pub fn remove_package(name: &str) -> Result<(), String> {
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
        let pkgs = installed_packages(&dir).unwrap();
        assert_eq!(pkgs.len(), 2);
        assert!(is_installed(&pkgs, "bash"));
        assert!(!is_installed(&pkgs, "nope"));
    }

    #[test]
    fn missing_db_is_empty_not_error() {
        let dir = std::env::temp_dir().join("slacker_nonexistent_db_xyz2");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(installed_packages(&dir).unwrap().len(), 0);
    }
}
