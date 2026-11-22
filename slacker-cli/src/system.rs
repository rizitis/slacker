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
pub(crate) fn is_removed_record(name: &str) -> bool {
    match name.rsplit_once("-upgraded-") {
        Some((_, suffix)) => suffix.starts_with(|c: char| c.is_ascii_digit()),
        None => false,
    }
}

/// Names of `<pkg>-upgraded-<timestamp>` records still in the installed-package
/// DB dir, sorted. `upgradepkg` renames the old record this way before it
/// installs the new package, and its `removepkg` step moves it to
/// removed_packages/ once the new one is in. One left here means an upgrade
/// that did not finish (the first install pass failed, a crash, a power loss),
/// or an `upgradepkg` that is running right now. Read-only.
pub fn leftover_upgrade_records(db_dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = match std::fs::read_dir(db_dir) {
        Ok(e) => e
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| is_removed_record(n))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// True when a pkgtools record has no file list: no "FILE LIST:" line, or no
/// entry after it. installpkg writes the header first and appends the list
/// only after tar has extracted everything, so such a record is an install
/// that was cut off (or one running now). A complete record lists at least
/// `./`. An unreadable record is not reported. Reads only up to the first
/// entry of the list.
pub(crate) fn record_lacks_file_list(record: &Path) -> bool {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(record) else { return false };
    let mut lines = std::io::BufReader::new(f).lines();
    for l in lines.by_ref() {
        match l {
            Ok(l) if l == "FILE LIST:" => break,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    // Ended without "FILE LIST:" (the loop ran out), or the list is empty.
    !lines.any(|l| l.map(|l| !l.is_empty()).unwrap_or(false))
}

/// Names of installed records with no file list (see `record_lacks_file_list`),
/// sorted. `-upgraded-` records are left out, and so is every record of a
/// package that has one: that is an unfinished upgrade, which fix-upgrade
/// handles as such. Read-only.
pub fn incomplete_records(db_dir: &Path) -> Vec<String> {
    let upgrading: std::collections::HashSet<String> = leftover_upgrade_records(db_dir)
        .iter()
        .filter_map(|r| PkgId::parse(crate::revert::strip_upgraded_suffix(r)).map(|p| p.name))
        .collect();
    let mut out: Vec<String> = match std::fs::read_dir(db_dir) {
        Ok(e) => e
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| !is_removed_record(n))
            .filter(|n| PkgId::parse(n).is_some_and(|p| !upgrading.contains(&p.name)))
            .filter(|n| record_lacks_file_list(&db_dir.join(n)))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// The root the pkgtools install into, read from ADM_DIR: the part before
/// `var/adm`, `var/log` or `var/lib/pkgtools`, or `/` for any other ADM_DIR.
pub fn system_root(adm_dir: &Path) -> PathBuf {
    for tail in ["var/adm", "var/log", "var/lib/pkgtools"] {
        let mut root = adm_dir.to_path_buf();
        let mut ok = true;
        for part in tail.rsplit('/') {
            if root.file_name().and_then(|n| n.to_str()) == Some(part) {
                root.pop();
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            return if root.as_os_str().is_empty() { PathBuf::from("/") } else { root };
        }
    }
    PathBuf::from("/")
}

/// A directory installpkg left in the root. installpkg unpacks the package's
/// `install/` into `ROOT/installpkg-<mcookie>` (or `-<PID>` without mcookie),
/// runs doinst.sh from there, copies it to scripts/, and removes the directory.
/// One still there is an install cut off after the unpack.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallTmp {
    pub dir: PathBuf,
    /// The package name, from the `name:` lines of its slack-desc.
    pub package: Option<String>,
    /// Its doinst.sh, if it has one.
    pub doinst: Option<String>,
    /// Entries other than do*inst.sh and slack-* (installpkg removes only
    /// those, then rmdir).
    pub other: Vec<String>,
}

fn is_installpkg_tmp_name(n: &str) -> bool {
    let Some(id) = n.strip_prefix("installpkg-") else { return false };
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        || (!id.is_empty() && id.len() <= 10 && id.bytes().all(|b| b.is_ascii_digit()))
}

/// The package a slack-desc describes: the name before the first `:` of its
/// first `name: ...` line (comment and ruler lines are skipped).
fn slack_desc_name(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (name, _) = l.split_once(':')?;
        let ok = !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"+_.-".contains(&b));
        ok.then(|| name.to_string())
    })
}

/// The directories installpkg left in `root` (see `InstallTmp`), sorted.
/// Symlinks are not followed. Read-only.
pub fn installpkg_tmp_dirs(root: &Path) -> Vec<InstallTmp> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<InstallTmp> = rd
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(is_installpkg_tmp_name))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| {
            let dir = e.path();
            let package = std::fs::read_to_string(dir.join("slack-desc")).ok().and_then(|t| slack_desc_name(&t));
            let doinst = std::fs::read_to_string(dir.join("doinst.sh")).ok();
            let mut other: Vec<String> = std::fs::read_dir(&dir)
                .map(|r| {
                    r.flatten()
                        .filter_map(|x| x.file_name().to_str().map(str::to_string))
                        .filter(|n| !(n.starts_with("do") && n.ends_with("inst.sh")) && !n.starts_with("slack-"))
                        .collect()
                })
                .unwrap_or_default();
            other.sort();
            InstallTmp { dir, package, doinst, other }
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// Installed records whose install script did not run: a directory installpkg
/// left (see `InstallTmp`) holds a doinst.sh, its package has a complete
/// record, and no record of that package has the same script in scripts/
/// (installpkg copies it there after running it). Packages with a
/// `-upgraded-` record or a record with no file list are left out: they are
/// fixed as such. One (record, dir) per package; the record is the first
/// one of that name. Read-only.
pub fn scripts_not_run(db_dir: &Path, tmps: &[InstallTmp]) -> Vec<(String, PathBuf)> {
    let scripts = db_dir.parent().map(|p| p.join("scripts")).unwrap_or_default();
    let skip: std::collections::HashSet<String> = leftover_upgrade_records(db_dir)
        .iter()
        .map(|r| crate::revert::strip_upgraded_suffix(r).to_string())
        .chain(incomplete_records(db_dir))
        .filter_map(|r| PkgId::parse(&r).map(|p| p.name))
        .collect();
    let installed = installed_packages(db_dir).unwrap_or_default();
    let mut out = Vec::new();
    for t in tmps {
        let (Some(name), Some(doinst)) = (&t.package, &t.doinst) else { continue };
        if skip.contains(name) {
            continue;
        }
        let recs: Vec<String> = installed.iter().filter(|p| &p.name == name).map(|p| p.tag()).collect();
        let ran = recs
            .iter()
            .any(|r| std::fs::read_to_string(scripts.join(r)).is_ok_and(|s| &s == doinst));
        if let (false, Some(first)) = (ran, recs.first()) {
            out.push((first.clone(), t.dir.clone()));
        }
    }
    out
}

/// Running installpkg, upgradepkg or removepkg processes, as (pid, tool),
/// read from /proc. These are shell scripts, so the tool is the first or the
/// second word of the command line (`/bin/bash /sbin/upgradepkg ...`).
pub fn pkgtools_running() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if pid == me {
            continue;
        }
        let Ok(raw) = std::fs::read(e.path().join("cmdline")) else { continue };
        let words: Vec<&[u8]> = raw.split(|b| *b == 0).take(2).collect();
        for w in words {
            let base = w.rsplit(|b| *b == b'/').next().unwrap_or(w);
            if let Ok(t @ ("installpkg" | "upgradepkg" | "removepkg")) = std::str::from_utf8(base) {
                out.push((pid, t.to_string()));
                break;
            }
        }
    }
    out.sort();
    out
}

pub fn installed_by_name<'a>(installed: &'a [PkgId], name: &str) -> Option<&'a PkgId> {
    installed.iter().find(|p| p.name == name)
}

/// Like `installed_by_name`, but matching case-insensitively — the same rule
/// `PkgDb::search` uses on repo names. Only for REPORTING paths that need to
/// answer "is something by this name on the system?" (`search`'s not-in-any-repo
/// fallback). Mutating paths keep using the exact `installed_by_name`: there the
/// package id must match verbatim.
///
/// Without this, `search` applied two different rules in one command — repo
/// names matched case-insensitively while the installed-package fallback did
/// not — so an SBo/local build with capitals (`ORBit2`, `PyQt-builder`) was
/// reported as installed for the exact spelling and "No package named" for the
/// lowercase one. Those builds are precisely the ones this fallback exists for.
pub fn installed_by_name_ci<'a>(installed: &'a [PkgId], name: &str) -> Option<&'a PkgId> {
    let needle = name.to_lowercase();
    installed.iter().find(|p| p.name.to_lowercase() == needle)
}

pub fn is_installed(installed: &[PkgId], name: &str) -> bool {
    installed.iter().any(|p| p.name == name)
}

/// `--terse` first when asked for (TERSE=on), then the given arguments.
fn pkgtools_args<'a>(terse: bool, args: &[&'a str]) -> Vec<&'a str> {
    let mut v: Vec<&str> = Vec::with_capacity(args.len() + 1);
    if terse {
        v.push("--terse");
    }
    v.extend_from_slice(args);
    v
}

/// Upgrade only (do not install if absent): plain `upgradepkg`.
pub fn upgrade_only(pkg_file: &Path, terse: bool) -> Result<(), String> {
    let f = pkg_file.to_string_lossy();
    run("upgradepkg", &pkgtools_args(terse, &[&f]))
}

/// `upgradepkg --reinstall`: install this package file even when the same
/// build is already installed.
pub fn reinstall(pkg_file: &Path, terse: bool) -> Result<(), String> {
    let f = pkg_file.to_string_lossy();
    run("upgradepkg", &pkgtools_args(terse, &["--reinstall", &f]))
}

/// Reinstall if a package of this name is installed, install it if none is:
/// `upgradepkg --install-new --reinstall`. The first step of fix-upgrade.
pub fn reinstall_or_install(pkg_file: &Path, terse: bool) -> Result<(), String> {
    let f = pkg_file.to_string_lossy();
    run("upgradepkg", &pkgtools_args(terse, &["--install-new", "--reinstall", &f]))
}

/// Fresh install: `installpkg`.
pub fn install(pkg_file: &Path, terse: bool) -> Result<(), String> {
    let f = pkg_file.to_string_lossy();
    run("installpkg", &pkgtools_args(terse, &[&f]))
}

/// Upgrade, installing the package even if no matching package is currently
/// installed: `upgradepkg --install-new`. This is what a distribution upgrade
/// needs for the core set, because packages get renamed across releases
/// (e.g. 15.0 `glibc-solibs` becomes -current `aaa_glibc-solibs`): a plain
/// `upgradepkg` would skip the new name as "not installed", whereas
/// `--install-new` installs it. Mirrors the official Slackware upgrade path.
pub fn upgrade_install_new(pkg_file: &Path, terse: bool) -> Result<(), String> {
    let f = pkg_file.to_string_lossy();
    run("upgradepkg", &pkgtools_args(terse, &["--install-new", &f]))
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

/// `removepkg --skip-douninst`: removes a leftover `-upgraded-` record without
/// running the uninstall script of the package of that name, which stays
/// installed (removepkg would otherwise run it for the record).
pub fn remove_record_only(name: &str) -> Result<(), String> {
    if name.starts_with('-') || name.contains('/') {
        return Err(format!("refusing to remove suspicious package name {name:?}"));
    }
    run("removepkg", &["--skip-douninst", name])
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
    fn terse_goes_first_only_when_asked() {
        assert_eq!(pkgtools_args(false, &["--reinstall", "a.txz"]), vec!["--reinstall", "a.txz"]);
        assert_eq!(pkgtools_args(true, &["--reinstall", "a.txz"]), vec!["--terse", "--reinstall", "a.txz"]);
        assert_eq!(pkgtools_args(true, &["a.txz"]), vec!["--terse", "a.txz"]);
    }

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
    fn leftover_upgrade_records_are_listed() {
        let dir = std::env::temp_dir().join("slacker_test_leftover_upgraded");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in [
            "openssl-3.3.3-x86_64-1",
            "openssl-3.3.2-x86_64-1-upgraded-2026-11-20,14:35:12",
            "my-upgraded-tool-1.0-x86_64-1",
            "bash-5.2.21-x86_64-3-upgraded-2026-11-20,14:30:00",
        ] {
            std::fs::File::create(dir.join(n)).unwrap();
        }
        assert_eq!(
            leftover_upgrade_records(&dir),
            vec![
                "bash-5.2.21-x86_64-3-upgraded-2026-11-20,14:30:00".to_string(),
                "openssl-3.3.2-x86_64-1-upgraded-2026-11-20,14:35:12".to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert!(leftover_upgrade_records(&dir).is_empty());
    }

    #[test]
    fn records_without_a_file_list_are_listed() {
        let dir = std::env::temp_dir().join("slacker_test_incomplete_records");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let head = "PACKAGE NAME:     x\nPACKAGE DESCRIPTION:\nx: x\n";
        for (n, body) in [
            ("good-1-x86_64-1", format!("{head}FILE LIST:\n./\nusr/\n")),
            ("nolist-1-x86_64-1", head.to_string()),
            ("emptylist-1-x86_64-1_SBo", format!("{head}FILE LIST:\n")),
            // an unfinished upgrade: fix-upgrade handles it as one
            ("up-2-x86_64-1", head.to_string()),
            ("up-1-x86_64-1-upgraded-2026-11-20,14:35:12", head.to_string()),
        ] {
            std::fs::write(dir.join(n), body).unwrap();
        }
        assert_eq!(
            incomplete_records(&dir),
            vec!["emptylist-1-x86_64-1_SBo".to_string(), "nolist-1-x86_64-1".to_string()]
        );
        assert!(!record_lacks_file_list(&dir.join("good-1-x86_64-1")));
        assert!(!record_lacks_file_list(&dir.join("missing-1-x86_64-1")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_comes_from_adm_dir() {
        assert_eq!(system_root(Path::new("/var/adm")), PathBuf::from("/"));
        assert_eq!(system_root(Path::new("/var/log")), PathBuf::from("/"));
        assert_eq!(system_root(Path::new("/mnt/new/var/lib/pkgtools")), PathBuf::from("/mnt/new"));
        assert_eq!(system_root(Path::new("/srv/adm")), PathBuf::from("/"));
    }

    #[test]
    fn installpkg_dirs_and_scripts_that_did_not_run() {
        let root = std::env::temp_dir().join("slacker_test_installpkg_tmp");
        let _ = std::fs::remove_dir_all(&root);
        let db = root.join("var/lib/pkgtools/packages");
        let sc = root.join("var/lib/pkgtools/scripts");
        std::fs::create_dir_all(&db).unwrap();
        std::fs::create_dir_all(&sc).unwrap();
        let list = "PACKAGE NAME: x\nFILE LIST:\n./\n";
        std::fs::write(db.join("foo-2-x86_64-1"), list).unwrap();
        std::fs::write(db.join("bar-1-x86_64-1"), list).unwrap();
        let doinst = "( cd usr/lib ; ln -sf libfoo.so libfoo.so.1 )\n";
        let mk = |id: &str, pkg: &str, extra: bool| {
            let d = root.join(format!("installpkg-{id}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("slack-desc"), format!("# HOW TO EDIT\n       |-----handy-ruler---|\n{pkg}: {pkg} 2\n{pkg}:\n")).unwrap();
            std::fs::write(d.join("doinst.sh"), doinst).unwrap();
            if extra {
                std::fs::write(d.join("stray"), "").unwrap();
            }
        };
        mk("e954c3e29369efedbe34a3acd716a45e", "foo", false);
        mk("4242", "bar", true);
        std::fs::create_dir_all(root.join("installpkg-notacookie")).unwrap();
        // bar's script ran (it is in scripts/); foo's did not
        std::fs::write(sc.join("bar-1-x86_64-1"), doinst).unwrap();
        let t = installpkg_tmp_dirs(&root);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].package.as_deref(), Some("bar"));
        assert_eq!(t[0].other, vec!["stray".to_string()]);
        assert_eq!(t[1].package.as_deref(), Some("foo"));
        assert!(t[1].other.is_empty());
        assert_eq!(
            scripts_not_run(&db, &t),
            vec![("foo-2-x86_64-1".to_string(), root.join("installpkg-e954c3e29369efedbe34a3acd716a45e"))]
        );
        let _ = std::fs::remove_dir_all(&root);
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
