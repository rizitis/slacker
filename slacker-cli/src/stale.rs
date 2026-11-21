//! Running programs that still use files an upgrade replaced (check-restart).
//!
//! `installpkg` unpacks with `tar --unlink-first`: the old file is unlinked
//! and the new one written to a new inode. A process that had the old program
//! or library loaded keeps using it, and the kernel shows that mapping in
//! /proc/PID/maps (or the program in /proc/PID/exe) with " (deleted)".
//!
//! A deleted mapping is reported only when all of these hold:
//! - it is executable (`x`): a program or a shared library, not a data file;
//! - the process is in our mount namespace (a container has its own files);
//! - its path is in a pkgtools record: an installed package's FILE LIST, or,
//!   failing that, a record in removed_packages/ (the package was removed);
//! - what is loaded differs from the file at that path now (read through
//!   /proc/PID/map_files, root only), so a reinstall of the same build is not
//!   reported. A path with no file now is reported as removed.
//!
//! Read-only. It cannot see what is not mapped: scripts (python, perl,
//! shell) and files read once at start. Needs root (every process, and
//! map_files).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// One replaced file a process still uses.
#[derive(Debug, Clone, PartialEq)]
pub struct StaleFile {
    /// Absolute path as the process mapped it.
    pub path: String,
    /// The package record that lists it.
    pub package: String,
    /// True when that record is in removed_packages/ (no installed package
    /// has the file).
    pub removed: bool,
    /// The process's own program, not a library.
    pub exe: bool,
}

#[derive(Debug, Clone)]
pub struct StaleProc {
    pub pid: u32,
    pub uid: u32,
    pub comm: String,
    /// The full command line, arguments separated by spaces (empty when the
    /// process has none to show).
    pub cmdline: String,
    pub files: Vec<StaleFile>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub procs: Vec<StaleProc>,
    /// Processes in another mount namespace with deleted executable
    /// mappings: not checked.
    pub other_ns: usize,
}

/// A deleted, executable mapping from one line of /proc/PID/maps:
/// (address range, path). None for anything else.
pub fn parse_maps_line(line: &str) -> Option<(String, String)> {
    let mut it = line.splitn(6, ' ');
    let range = it.next()?;
    let perms = it.next()?;
    let _offset = it.next()?;
    let _dev = it.next()?;
    let inode = it.next()?;
    let path = it.next()?.trim_start();
    if !perms.contains('x') || inode == "0" || !path.starts_with('/') {
        return None;
    }
    let path = path.strip_suffix(" (deleted)")?;
    Some((range.to_string(), path.to_string()))
}

/// The record lines of a pkgtools FILE LIST, as absolute paths.
fn listed_paths(text: &str) -> impl Iterator<Item = String> + '_ {
    let mut lines = text.lines();
    let found = lines.by_ref().any(|l| l == "FILE LIST:");
    lines
        .filter(move |_| found)
        .filter(|l| !l.is_empty() && !l.ends_with('/'))
        .map(|l| format!("/{}", l.trim_start_matches("./")))
}

/// For each wanted path, the record in `dir` that lists it. With several,
/// the most recently changed record wins (the newest removal).
pub fn owners_in(dir: &Path, wanted: &HashSet<String>, skip_upgraded: bool) -> HashMap<String, String> {
    let mut best: HashMap<String, (String, std::time::SystemTime)> = HashMap::new();
    if wanted.is_empty() {
        return HashMap::new();
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return HashMap::new() };
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        if skip_upgraded && crate::system::is_removed_record(&name) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        for p in listed_paths(&text) {
            if wanted.contains(&p) && best.get(&p).is_none_or(|(_, t)| mtime > *t) {
                best.insert(p, (name.clone(), mtime));
            }
        }
    }
    best.into_iter().map(|(p, (n, _))| (p, n)).collect()
}

/// True when the two readers give the same bytes.
fn same_content(mut a: impl Read, mut b: impl Read) -> bool {
    let (mut x, mut y) = ([0u8; 65536], [0u8; 65536]);
    loop {
        let n = match a.read(&mut x) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let mut got = 0;
        while got < n {
            match b.read(&mut y[got..n]) {
                Ok(0) | Err(_) => return false,
                Ok(m) => got += m,
            }
        }
        if x[..n] != y[..n] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }
}

/// Is what the process has loaded (`loaded`, a /proc link) still what is on
/// disk at `path`? A missing file is never the same.
fn unchanged(loaded: &Path, path: &str) -> bool {
    let (Ok(a), Ok(b)) = (std::fs::File::open(loaded), std::fs::File::open(path)) else {
        return false;
    };
    let same_len = match (a.metadata(), b.metadata()) {
        (Ok(x), Ok(y)) => x.len() == y.len(),
        _ => false,
    };
    same_len && same_content(a, b)
}

/// /proc/PID/cmdline: arguments are NUL-separated.
fn cmdline_of(pid_dir: &Path) -> String {
    let raw = std::fs::read(pid_dir.join("cmdline")).unwrap_or_default();
    String::from_utf8_lossy(&raw)
        .split('\0')
        .filter(|a| !a.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn uid_of(pid_dir: &Path) -> u32 {
    std::fs::read_to_string(pid_dir.join("status"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|r| r.split_whitespace().next().and_then(|u| u.parse().ok()))
        })
        .unwrap_or(u32::MAX)
}

/// Scan every process. `pkg_db_dir` is the installed-package records,
/// `removed_dir` the removed ones.
pub fn scan(pkg_db_dir: &Path, removed_dir: &Path) -> Report {
    let mut report = Report::default();
    let me = std::process::id();
    let my_ns = std::fs::read_link("/proc/self/ns/mnt").ok();

    // 1. Deleted executable mappings, per process. (pid, comm, uid,
    //    [(loaded link, path, exe)])
    type Cand = (PathBuf, String, bool);
    let mut cands: Vec<(u32, String, String, u32, Vec<Cand>)> = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return report };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if pid == me {
            continue;
        }
        let dir = e.path();
        let mut found: Vec<Cand> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        if let Ok(exe) = std::fs::read_link(dir.join("exe")) {
            if let Some(p) = exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
                if seen.insert(p.to_string()) {
                    found.push((dir.join("exe"), p.to_string(), true));
                }
            }
        }
        if let Ok(maps) = std::fs::read_to_string(dir.join("maps")) {
            for (range, p) in maps.lines().filter_map(parse_maps_line) {
                if seen.insert(p.clone()) {
                    found.push((dir.join("map_files").join(range), p, false));
                }
            }
        }
        if found.is_empty() {
            continue;
        }
        // Another mount namespace sees other files under the same paths.
        if std::fs::read_link(dir.join("ns/mnt")).ok() != my_ns {
            report.other_ns += 1;
            continue;
        }
        let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default().trim().to_string();
        let cmdline = cmdline_of(&dir);
        cands.push((pid, comm, cmdline, uid_of(&dir), found));
    }
    if cands.is_empty() {
        return report;
    }

    // 2. Which package records list those paths: installed first, then the
    //    removed ones for what is left. Paths in no record are not packaged
    //    (memfd, /tmp files, ...) and are not reported.
    let wanted: HashSet<String> = cands.iter().flat_map(|c| c.4.iter().map(|f| f.1.clone())).collect();
    let installed = owners_in(pkg_db_dir, &wanted, true);
    let rest: HashSet<String> = wanted.iter().filter(|p| !installed.contains_key(*p)).cloned().collect();
    let removed = owners_in(removed_dir, &rest, false);

    // 3. Keep what is loaded and differs from the file now.
    for (pid, comm, cmdline, uid, found) in cands {
        let mut files = Vec::new();
        for (loaded, path, exe) in found {
            let (package, is_removed) = match (installed.get(&path), removed.get(&path)) {
                (Some(p), _) => (p.clone(), false),
                (None, Some(p)) => (crate::revert::strip_upgraded_suffix(p).to_string(), true),
                (None, None) => continue,
            };
            if unchanged(&loaded, &path) {
                continue;
            }
            files.push(StaleFile { path, package, removed: is_removed, exe });
        }
        if !files.is_empty() {
            report.procs.push(StaleProc { pid, uid, comm, cmdline, files });
        }
    }
    report.procs.sort_by_key(|p| p.pid);
    report
}

/// A user name for a uid, from /etc/passwd; the number when not found.
pub fn user_name(uid: u32) -> String {
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.len() > 2 && f[2] == uid.to_string()).then(|| f[0].to_string())
            })
        })
        .unwrap_or_else(|| uid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_deleted_executable_file_mappings() {
        let lib = "7f1c-7f1d r-xp 00002000 fe:00 902269                     /usr/lib64/libssl.so.3 (deleted)";
        assert_eq!(
            parse_maps_line(lib),
            Some(("7f1c-7f1d".to_string(), "/usr/lib64/libssl.so.3".to_string()))
        );
        // a path with a space in it
        let sp = "1-2 r-xp 0 fe:00 5      /opt/my app/lib.so (deleted)";
        assert_eq!(parse_maps_line(sp).unwrap().1, "/opt/my app/lib.so");
        // not executable: a data file (locale-archive after a glibc upgrade)
        assert!(parse_maps_line("1-2 r--p 0 fe:00 9  /usr/lib/locale/locale-archive (deleted)").is_none());
        // not deleted
        assert!(parse_maps_line("1-2 r-xp 0 fe:00 9  /usr/lib64/libc.so.6").is_none());
        // anonymous, memfd, special
        assert!(parse_maps_line("1-2 r-xp 0 00:00 0 ").is_none());
        assert!(parse_maps_line("1-2 r-xs 0 00:01 3  /memfd:x (deleted)").is_some_and(|(_, p)| p == "/memfd:x"));
        assert!(parse_maps_line("1-2 r-xp 0 00:00 0  [vdso]").is_none());
    }

    #[test]
    fn owners_come_from_pkgtools_file_lists() {
        let root = std::env::temp_dir().join("slacker_stale_owners");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("openssl-3.3.3-x86_64-1"),
            "PACKAGE NAME: openssl\nFILE LIST:\n./\nusr/\nusr/lib64/\nusr/lib64/libssl.so.3\n",
        )
        .unwrap();
        std::fs::write(
            root.join("openssl-3.3.2-x86_64-1-upgraded-2026-11-20,14:35:12"),
            "FILE LIST:\nusr/lib64/libssl.so.3\n",
        )
        .unwrap();
        let wanted: HashSet<String> =
            ["/usr/lib64/libssl.so.3".to_string(), "/tmp/scratch".to_string()].into_iter().collect();
        let got = owners_in(&root, &wanted, true);
        assert_eq!(got.get("/usr/lib64/libssl.so.3").map(String::as_str), Some("openssl-3.3.3-x86_64-1"));
        assert!(!got.contains_key("/tmp/scratch"));
        assert!(!got.contains_key("/usr/lib64/"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_content_compares_bytes() {
        assert!(same_content(&b"abc"[..], &b"abc"[..]));
        assert!(!same_content(&b"abc"[..], &b"abd"[..]));
        assert!(!same_content(&b"abc"[..], &b"ab"[..]));
    }
}
