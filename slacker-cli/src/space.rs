//! Disk space before an install, upgrade or reinstall: an estimate of what the
//! plan writes, per filesystem, against the space free there. Asks only when
//! it may not fit; prints nothing when it does. upgrade-dist has its own
//! checks and does not use this.
//!
//! The estimate is the whole new package, file by file from the repo's
//! MANIFEST (each file rounded up to the filesystem block, each directory that
//! does not exist yet one block, the pkgtools record twice: the new one and the
//! renamed old one until removepkg), plus 10%. Measured on ext4 with real
//! upgradepkg runs, this is what an upgrade can take at its peak when running
//! programs keep the old files open (their blocks are freed only when the
//! programs exit); without that the peak is lower. A package that is in no
//! MANIFEST is counted as twice its PACKAGES.TXT uncompressed size, since that
//! figure was as low as half the real one for packages with many small files.
//! Packages not yet in the cache are counted, compressed, on the cache's
//! filesystem.

use crate::config::Config;
use crate::{manifest, repo, system, ui, PlanItem};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

const MARGIN_PERCENT: u64 = 10;

/// Mount points from /proc/self/mounts, longest first.
fn mount_points() -> Vec<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut v: Vec<PathBuf> = text
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|m| PathBuf::from(unescape_mount(m)))
        .collect();
    v.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    v.dedup();
    v
}

/// /proc/self/mounts writes space, tab, newline and backslash as \ooo.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The path with its deepest existing ancestor resolved (symlinks followed),
/// so `/var/adm/...` lands on the filesystem of what it points to.
fn resolve(p: &Path) -> PathBuf {
    let mut base = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = base.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (base.file_name().map(|f| f.to_os_string()), base.parent()) {
            (Some(f), Some(parent)) => {
                rest.push(f);
                base = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

fn mount_of(mounts: &[PathBuf], p: &Path) -> PathBuf {
    mounts
        .iter()
        .find(|m| p.starts_with(m))
        .cloned()
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Free 1K-blocks for unprivileged use (`df -Pk`), or None.
fn avail_bytes(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-Pk").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let kb: u64 = s.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb * 1024)
}

fn block_size(path: &Path) -> u64 {
    // `stat -f -c %S` is the fundamental block size; 4096 if unknown.
    std::process::Command::new("stat")
        .args(["-f", "-c", "%S"])
        .arg(path)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .filter(|b| *b >= 512)
        .unwrap_or(4096)
}

fn round_up(n: u64, bs: u64) -> u64 {
    n.div_ceil(bs) * bs
}

pub(crate) fn human(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1}G", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.1}M", b / (K * K))
    } else {
        format!("{:.0}K", (b / K).ceil())
    }
}

/// One filesystem that may be short.
#[derive(Debug, PartialEq)]
pub struct Short {
    pub mount: PathBuf,
    pub need: u64,
    pub free: u64,
    /// The cache's share of `need`.
    pub cache: u64,
}

/// What `estimate` found.
#[derive(Debug, Default)]
pub struct Estimate {
    /// Bytes per filesystem (mount point): installed files and cache.
    pub need: BTreeMap<PathBuf, u64>,
    pub cache: BTreeMap<PathBuf, u64>,
    /// Packages counted from PACKAGES.TXT (in no MANIFEST).
    pub rough: usize,
}

pub fn estimate(cfg: &Config, plan: &[PlanItem]) -> Estimate {
    let mounts = mount_points();
    let root = system::system_root(&cfg.adm_dir);
    let root_r = resolve(&root);
    let db_mount = mount_of(&mounts, &resolve(&cfg.pkg_db_dir));
    let mut bs_cache: HashMap<PathBuf, u64> = HashMap::new();
    let mut bs = |m: &Path| *bs_cache.entry(m.to_path_buf()).or_insert_with(|| block_size(m));
    let mut est = Estimate::default();
    let mut files: BTreeMap<PathBuf, u64> = BTreeMap::new();

    // The MANIFEST entries of the plan's packages, one pass per repo.
    let mut by_repo: HashMap<&str, HashSet<&str>> = HashMap::new();
    for it in plan {
        by_repo.entry(it.pkg.repo.as_str()).or_default().insert(it.pkg.filename.as_str());
    }
    let mut listed: HashMap<(String, String), Vec<manifest::Entry>> = HashMap::new();
    for (rname, wanted) in &by_repo {
        let Some(r) = cfg.repo_by_name(rname) else { continue };
        let Ok(text) = repo::read_text_lossy(&repo::meta_path(r, &cfg.cache_dir, repo::MANIFEST)) else {
            continue;
        };
        for (f, e) in manifest::entries_of(&text, wanted) {
            listed.insert((rname.to_string(), f), e);
        }
    }

    for it in plan {
        match listed.get(&(it.pkg.repo.clone(), it.pkg.filename.clone())) {
            Some(entries) => {
                let mut rec = 600u64;
                for e in entries {
                    rec += e.path.len() as u64 + 1;
                    if e.path.starts_with("install/") || e.path == "install" {
                        continue;
                    }
                    let full = root_r.join(&e.path);
                    let m = mount_of(&mounts, &full);
                    let add = match e.kind {
                        b'-' => round_up(e.size, bs(&m)),
                        b'd' if !full.exists() => bs(&m),
                        _ => 0,
                    };
                    *files.entry(m).or_default() += add;
                }
                *files.entry(db_mount.clone()).or_default() += 2 * round_up(rec, bs(&db_mount));
            }
            None => {
                est.rough += 1;
                let m = mount_of(&mounts, &root_r);
                *files.entry(m).or_default() += 2 * it.pkg.size_uncompressed_k.unwrap_or(0) * 1024;
            }
        }
    }
    for (m, b) in files {
        *est.need.entry(m).or_default() += b + b * MARGIN_PERCENT / 100;
    }

    // Packages still to download, on the cache's filesystem.
    for it in plan {
        let dest = system::cached_pkg_path(&cfg.cache_dir, &it.pkg.repo, &it.pkg.filename);
        if dest.exists() {
            continue;
        }
        let m = mount_of(&mounts, &resolve(&dest));
        let b = it.pkg.size_k.unwrap_or(0) * 1024;
        *est.need.entry(m.clone()).or_default() += b;
        *est.cache.entry(m).or_default() += b;
    }
    est
}

/// The filesystems where the estimate is above the free space. One whose free
/// space cannot be read is left out.
pub fn shortfalls(est: &Estimate) -> Vec<Short> {
    est.need
        .iter()
        .filter_map(|(m, need)| {
            let free = avail_bytes(m)?;
            (*need > free).then(|| Short {
                mount: m.clone(),
                need: *need,
                free,
                cache: est.cache.get(m).copied().unwrap_or(0),
            })
        })
        .collect()
}

/// Before the plan runs: if it may not fit, say so and ask (default no).
/// With `yes` it only warns; with `dry_run` it warns and does not ask. True
/// when the command goes on.
pub(crate) fn confirm_space(cfg: &Config, plan: &[PlanItem], yes: bool, dry_run: bool) -> bool {
    if plan.is_empty() {
        return true;
    }
    let est = estimate(cfg, plan);
    let short = shortfalls(&est);
    if short.is_empty() {
        return true;
    }
    println!("\n{}", ui::yellow("Disk space may not be enough:"));
    let w = short.iter().map(|s| s.mount.display().to_string().len()).max().unwrap_or(1);
    for s in &short {
        let cache = if s.cache > 0 { format!(" (of which {} for the package cache)", human(s.cache)) } else { String::new() };
        println!(
            "  {}  needs up to {}, free {}{cache}",
            ui::white(&format!("{:<w$}", s.mount.display())),
            human(s.need),
            human(s.free)
        );
    }
    println!(
        "{}",
        ui::dim("(the estimate counts the whole new packages, as if running programs keep the old files open)")
    );
    if est.rough > 0 {
        println!(
            "{}",
            ui::dim(&format!(
                "({} package(s) are in no cached MANIFEST: a rough estimate from PACKAGES.TXT)",
                est.rough
            ))
        );
    }
    if dry_run {
        return true;
    }
    if yes {
        println!("{}", ui::yellow("--yes: continuing anyway."));
        return true;
    }
    if crate::confirm("Continue anyway?", false) {
        return true;
    }
    println!("Nothing changed.");
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_names_are_unescaped() {
        assert_eq!(unescape_mount("/mnt/my\\040disk"), "/mnt/my disk");
        assert_eq!(unescape_mount("/plain"), "/plain");
        assert_eq!(unescape_mount("/end\\"), "/end\\");
    }

    #[test]
    fn longest_mount_wins() {
        let mut m = vec![PathBuf::from("/"), PathBuf::from("/usr"), PathBuf::from("/usr/local")];
        m.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
        assert_eq!(mount_of(&m, Path::new("/usr/local/bin/x")), PathBuf::from("/usr/local"));
        assert_eq!(mount_of(&m, Path::new("/usr/bin/x")), PathBuf::from("/usr"));
        assert_eq!(mount_of(&m, Path::new("/usrx/y")), PathBuf::from("/"));
        assert_eq!(mount_of(&m, Path::new("/etc/x")), PathBuf::from("/"));
    }

    #[test]
    fn sizes_round_and_read() {
        assert_eq!(round_up(1, 4096), 4096);
        assert_eq!(round_up(4096, 4096), 4096);
        assert_eq!(round_up(0, 4096), 0);
        assert_eq!(human(2 * 1024 * 1024 * 1024 + 400 * 1024 * 1024), "2.4G");
        assert_eq!(human(640 * 1024 * 1024), "640.0M");
        assert_eq!(human(1500), "2K");
    }
}
