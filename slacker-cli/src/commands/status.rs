//! status, list-repos, check-updates.

use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{
    arch_family, bl_installed, current_uid, humanize_lag, migrate_state, mirror_is_stale,
    osuosl_tree_prefix, parse_packages_date, repo_release_token, system_release,
    unverified_repo_names, warn_unverified_repos, Outcome, FRESHNESS_TIMEOUT_SECS,
};
use crate::{changelog, config, dist, download, gpg, newconfig, pkg, repo, stockdb, system, ui};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Repos fetched over plaintext http:// — a network attacker can rewrite their
/// metadata and packages in transit, which (without a pinned GPG key) is the
/// same as a malicious repo.
fn insecure_http_repos(cfg: &Config) -> Vec<String> {
    cfg.repos
        .iter()
        .filter(|r| r.url.to_ascii_lowercase().starts_with("http://"))
        .map(|r| r.name.clone())
        .collect()
}

/// Short label for an effective verify policy, e.g. "all", "none", "gpg,md5".
fn verify_label(p: &config::VerifyPolicy) -> String {
    match p {
        config::VerifyPolicy::All => "all".to_string(),
        config::VerifyPolicy::None => "none".to_string(),
        config::VerifyPolicy::Required(v) => {
            v.iter().map(|c| c.label()).collect::<Vec<_>>().join(",")
        }
    }
}

/// Attribute each installed package to a source by its build tag (pure core).
/// An empty tag is the official repo; a tag the `resolve_repo` closure maps to a
/// binary repo goes there; a tag matching a declared tag-priority rule (e.g.
/// `_SBo`) maps to that rule's name; any other tag is itself the source. An
/// installed package is never "untracked" — its build tag is the user's own
/// choice and a legitimate source. Returns (per-repo, per-tag-rule-by-name,
/// per-other-tag).
fn attribute_tags(
    official_repo: Option<&str>,
    tag_priorities: &[config::TagPriority],
    resolve_repo: impl Fn(&str) -> Option<String>,
    installed: &[pkg::PkgId],
) -> (
    HashMap<String, usize>,
    HashMap<String, usize>,
    HashMap<String, usize>,
) {
    let mut per_repo: HashMap<String, usize> = HashMap::new();
    let mut per_rule: HashMap<String, usize> = HashMap::new();
    let mut per_tag: HashMap<String, usize> = HashMap::new();
    for p in installed {
        let tag = p.build_tag();
        let repo = if tag.is_empty() {
            official_repo.map(|s| s.to_string())
        } else {
            resolve_repo(tag)
        };
        if let Some(r) = repo {
            *per_repo.entry(r).or_default() += 1;
        } else if tag.is_empty() {
            // Empty tag but no official repo configured: a real edge.
            *per_tag.entry("(no official repo)".to_string()).or_default() += 1;
        } else if let Some(tp) = tag_priorities.iter().find(|tp| tp.tag == tag) {
            *per_rule.entry(tp.name.clone()).or_default() += 1;
        } else {
            *per_tag.entry(tag.to_string()).or_default() += 1;
        }
    }
    (per_repo, per_rule, per_tag)
}

/// Attribute installed packages using the available-package DB to resolve which
/// repo serves a build tag. With no DB, all maps are empty.
fn installed_attribution(
    cfg: &Config,
    db: Option<&PkgDb>,
    installed: &[pkg::PkgId],
) -> (
    HashMap<String, usize>,
    HashMap<String, usize>,
    HashMap<String, usize>,
) {
    match db {
        Some(db) => attribute_tags(
            cfg.official_repo_name(),
            &cfg.tag_priorities,
            |t| db.repo_for_tag(t).map(|s| s.to_string()),
            installed,
        ),
        None => (HashMap::new(), HashMap::new(), HashMap::new()),
    }
}

/// `list-repos`: show every configured repository with its priority, effective
/// verify policy, URL, and how many installed packages came from it; then the
/// build-tag priority lines and a grand total. Per-repo counts need the package
/// DB (from `update`); without it the repo list still prints, counts as `?`.
pub(crate) fn cmd_list_repos(cfg: &Config) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (db, missing) = PkgDb::load_available(cfg);
    let (per_repo, per_rule, per_other) = installed_attribution(cfg, Some(&db), &installed);

    let repos = cfg.repos_by_priority();
    // Only a repo whose metadata is missing shows `?`; every other repo gets its
    // real count. This isolates an un-updated / wrong-URL repo instead of blanking
    // the whole column.
    let count_str = |name: &str| -> String {
        if missing.iter().any(|m| m == name) {
            "?".to_string()
        } else {
            per_repo.get(name).copied().unwrap_or(0).to_string()
        }
    };
    let once = std::iter::once;
    let wn = repos.iter().map(|r| r.name.len()).chain(once(4)).max().unwrap();
    let wi = repos.iter().map(|r| count_str(&r.name).len()).chain(once(4)).max().unwrap();
    let wv = repos
        .iter()
        .map(|r| verify_label(r.verify_policy(&cfg.verify)).len())
        .chain(once(6))
        .max()
        .unwrap();
    let sep = ui::dim(" | ");

    println!("{}", ui::blue("Configured repositories (highest priority first):"));
    println!(
        "  {}{}{}{}{}{}{}{}{}",
        ui::blue(&format!("{:>4}", "Pri")),
        sep,
        ui::blue(&format!("{:<wn$}", "Name")),
        sep,
        ui::blue(&format!("{:>wi$}", "Inst")),
        sep,
        ui::blue(&format!("{:<wv$}", "Verify")),
        sep,
        ui::blue("URL"),
    );
    println!(
        "  {}",
        ui::dim(&format!(
            "{}-+-{}-+-{}-+-{}-+-{}",
            "-".repeat(4),
            "-".repeat(wn),
            "-".repeat(wi),
            "-".repeat(wv),
            "-".repeat(3)
        ))
    );
    for r in &repos {
        let pol = r.verify_policy(&cfg.verify);
        let vcolor: fn(&str) -> String = match pol {
            config::VerifyPolicy::None => ui::red,
            config::VerifyPolicy::All => ui::green,
            config::VerifyPolicy::Required(_) => ui::yellow,
        };
        let mut line = format!(
            "  {}{}{}{}{}{}{}{}{}",
            ui::dim(&format!("{:>4}", r.priority)),
            sep,
            ui::white(&format!("{:<wn$}", r.name)),
            sep,
            ui::cyan(&format!("{:>wi$}", count_str(&r.name))),
            sep,
            vcolor(&format!("{:<wv$}", verify_label(pol))),
            sep,
            ui::dim(&r.url),
        );
        if r.official {
            line.push_str(&ui::cyan("  (official)"));
        }
        if r.immutable {
            line.push_str(&ui::cyan("  (immutable)"));
        }
        if r.subtree {
            line.push_str(&ui::cyan("  (subtree)"));
        }
        if let Some(name) = &r.credentials {
            line.push_str(&ui::cyan(&format!("  (credentials={name})")));
        }
        if r.insecure {
            line.push_str(&ui::yellow("  (insecure)"));
        }
        if repo::is_hard_quarantined(&cfg.state_dir, &r.name) {
            line.push_str(&ui::red("  [FROZEN]"));
        } else if repo::is_quarantined(&cfg.state_dir, &r.name) {
            line.push_str(&ui::yellow("  [unreachable — retrying]"));
        }
        println!("{line}");
    }

    if !cfg.tag_priorities.is_empty() {
        println!();
        println!("{}", ui::blue("Build-tag priorities:"));
        let rule_inst = |t: &config::TagPriority| per_rule.get(&t.name).copied().unwrap_or(0);
        let wtn = cfg.tag_priorities.iter().map(|t| t.name.len()).chain(once(4)).max().unwrap();
        let wtt = cfg.tag_priorities.iter().map(|t| t.tag.len()).chain(once(3)).max().unwrap();
        let wti = cfg
            .tag_priorities
            .iter()
            .map(|t| rule_inst(t).to_string().len())
            .chain(once(4))
            .max()
            .unwrap();
        println!(
            "  {}{}{}{}{}{}{}",
            ui::blue(&format!("{:>4}", "Pri")),
            sep,
            ui::blue(&format!("{:<wtn$}", "Name")),
            sep,
            ui::blue(&format!("{:<wtt$}", "Tag")),
            sep,
            ui::blue(&format!("{:>wti$}", "Inst")),
        );
        for t in &cfg.tag_priorities {
            let inst = rule_inst(t);
            let mut line = format!(
                "  {}{}{}{}{}{}{}",
                ui::dim(&format!("{:>4}", t.priority)),
                sep,
                ui::white(&format!("{:<wtn$}", t.name)),
                sep,
                ui::cyan(&format!("{:<wtt$}", t.tag)),
                sep,
                ui::cyan(&format!("{:>wti$}", inst)),
            );
            // A declared rule that matches no installed package is worth a quiet
            // heads-up (often a typo in the tag), but it is not an error.
            if inst == 0 {
                line.push_str(&ui::yellow("  (declared, no installed package)"));
            }
            println!("{line}");
        }
    }

    println!();
    println!(
        "{} {}",
        ui::blue("Total installed packages:"),
        ui::white(&installed.len().to_string())
    );
    if !per_other.is_empty() {
        let total_other: usize = per_other.values().sum();
        let mut items: Vec<(&String, &usize)> = per_other.iter().collect();
        items.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let breakdown =
            items.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ");
        println!(
            "{} {} {}",
            ui::blue("Installed under other build tags:"),
            ui::white(&total_other.to_string()),
            ui::dim(&format!("[{breakdown}]")),
        );
    }
    if !missing.is_empty() {
        println!(
            "{}",
            ui::yellow(&format!(
                "no metadata yet for: {} (shown as ?) — run `slacker update`, or check the repo URL",
                missing.join(", ")
            ))
        );
    }
    Ok(Outcome::Ok)
}

/// Human-friendly "time since" for a file mtime, e.g. "3m", "2h", "5d".
fn ago(t: std::time::SystemTime) -> String {
    match t.elapsed() {
        Ok(d) => {
            let s = d.as_secs();
            if s < 90 {
                format!("{s}s")
            } else if s < 90 * 60 {
                format!("{}m", s / 60)
            } else if s < 36 * 3600 {
                format!("{}h", s / 3600)
            } else {
                format!("{}d", s / 86400)
            }
        }
        Err(_) => "just now".to_string(),
    }
}

/// Tally the downloaded-package cache: (file count, total bytes, most-recent
/// mtime). Only CACHE_DIR/packages/<repo>/<file> regular files are counted —
/// repo metadata and GPG keys live elsewhere and are never touched by
/// clean-cache. A missing/unreadable cache reads as empty. Never recurses below
/// the per-repo directory and never follows symlinks.
fn scan_package_cache(pkg_root: &Path) -> (u64, u64, Option<std::time::SystemTime>) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut newest: Option<std::time::SystemTime> = None;
    let Ok(repos) = std::fs::read_dir(pkg_root) else {
        return (0, 0, None);
    };
    for repo_ent in repos.flatten() {
        let dir = repo_ent.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(inner) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in inner.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(ent.path()) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            files += 1;
            bytes += meta.len();
            if let Ok(m) = meta.modified() {
                newest = Some(match newest {
                    Some(n) if n >= m => n,
                    _ => m,
                });
            }
        }
    }
    (files, bytes, newest)
}

/// Human-readable byte size (B / KiB / MiB / GiB), one decimal from MiB up.
fn humanize_bytes(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1} GiB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.1} MiB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KiB", b / K)
    } else {
        format!("{bytes} B")
    }
}

/// `status`: a one-shot health check of the whole setup. Every line reports a
/// real, verifiable fact (config is already structurally validated at load, so
/// here we check initialisation and reachability) and ends with a truthful
/// verdict plus concrete next steps. Read-only; safe to run any time.
/// True if an executable `name` is found in any of `dirs`. Split out from
/// [`tool_on_path`] so it can be unit-tested without touching the real `$PATH`.
fn tool_in_dirs(name: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| {
        let p = dir.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            p.is_file()
        }
    })
}

/// True if `name` resolves to an executable on the current `$PATH` — i.e. a
/// `Command::new(name)` would find it. Used by `status` to flag a missing
/// external tool (pkgtools, gpg, bzip2, ...) up front, before its absence
/// surfaces as a confusing failure in the middle of an operation.
fn tool_on_path(name: &str) -> bool {
    match std::env::var_os("PATH") {
        Some(path) => tool_in_dirs(name, &std::env::split_paths(&path).collect::<Vec<_>>()),
        None => false,
    }
}

/// Where Slackware's pkgtools package installs installpkg/upgradepkg/removepkg.
const PKGTOOLS_DIR: &str = "/sbin";

/// True if a pkgtools command is on `$PATH` or in `sbin`. Slackware's
/// /etc/profile puts /sbin on the PATH of root only, so a plain user's
/// `slacker status` (slacker-gui runs it that way) would otherwise report the
/// pkgtools missing. slacker only runs them as root, where /sbin is on the PATH.
fn pkgtool_present(name: &str, sbin: &Path) -> bool {
    tool_on_path(name) || tool_in_dirs(name, &[sbin.to_path_buf()])
}

/// What auditing slacker's OWN files turned up. Counts are totals across the
/// whole tree; the `sample_*` vectors keep up to `AUDIT_SAMPLE` offenders each
/// so the report can name a few without flooding the screen.
#[derive(Default)]
struct PathAudit {
    checked: usize,
    not_root: usize,
    world_writable: usize,
    group_writable: usize,
    symlinks: usize,
    unreadable: usize,
    sample_not_root: Vec<PathBuf>,
    sample_world_writable: Vec<PathBuf>,
    sample_group_writable: Vec<PathBuf>,
    sample_symlinks: Vec<(PathBuf, PathBuf)>,
}

const AUDIT_SAMPLE: usize = 3;

fn push_capped<T>(v: &mut Vec<T>, item: T) {
    if v.len() < AUDIT_SAMPLE {
        v.push(item);
    }
}

impl PathAudit {
    /// No ownership/permission/symlink problems at all.
    fn clean(&self) -> bool {
        self.not_root == 0
            && self.world_writable == 0
            && self.group_writable == 0
            && self.symlinks == 0
    }
    /// A problem that lets a non-root user redirect or rewrite slacker's state —
    /// i.e. it undermines the trust model, not just tidiness.
    fn severe(&self) -> bool {
        self.world_writable > 0 || self.symlinks > 0
    }
}

#[cfg(unix)]
fn check_perms(a: &mut PathAudit, path: &Path, meta: &std::fs::Metadata) {
    use std::os::unix::fs::MetadataExt;
    if meta.uid() != 0 {
        a.not_root += 1;
        push_capped(&mut a.sample_not_root, path.to_path_buf());
    }
    let mode = meta.mode();
    if mode & 0o002 != 0 {
        a.world_writable += 1;
        push_capped(&mut a.sample_world_writable, path.to_path_buf());
    } else if mode & 0o020 != 0 {
        a.group_writable += 1;
        push_capped(&mut a.sample_group_writable, path.to_path_buf());
    }
}
#[cfg(not(unix))]
fn check_perms(_a: &mut PathAudit, _path: &Path, _meta: &std::fs::Metadata) {}

/// Audit slacker's OWN files (config + cache + state) for tampering and unsafe
/// permissions. On Slackware these are all owned by root and not writable by
/// anyone else; the trust state in the cache (pinned GPG fingerprints,
/// trusted/quarantine markers) is security-critical, so a world-writable file
/// or an unexpected symlink there is a red flag — slacker itself creates no
/// symlinks under these paths. The walk uses lstat and NEVER follows a symlink,
/// so a planted link cannot lure it outside the tree. Only the roots are
/// followed, since an admin may legitimately mount one via a symlink.
fn audit_owned_paths(roots: &[&Path]) -> PathAudit {
    let mut a = PathAudit::default();
    let mut stack: Vec<PathBuf> = Vec::new();
    for r in roots {
        // Follow the root (it may be a legitimate symlink to a real directory);
        // entries inside are what we scrutinise for stray links.
        if let Ok(meta) = std::fs::metadata(r) {
            a.checked += 1;
            check_perms(&mut a, r, &meta);
            if meta.is_dir() {
                stack.push(r.to_path_buf());
            }
        }
    }
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                // Almost always a root-only (0700) subdir we cannot enter as a
                // non-root user; report it so the user can re-run under sudo.
                a.unreadable += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            a.checked += 1;
            if meta.file_type().is_symlink() {
                a.symlinks += 1;
                let target = std::fs::read_link(&path).unwrap_or_default();
                push_capped(&mut a.sample_symlinks, (path, target));
                continue; // never follow a symlink
            }
            check_perms(&mut a, &path, &meta);
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    a
}

/// Format up to `AUDIT_SAMPLE` offending paths, with a "(+N more)" tail.
fn fmt_path_sample(sample: &[PathBuf], total: usize) -> String {
    let mut s = sample.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
    if total > sample.len() {
        s.push_str(&format!(" (+{} more)", total - sample.len()));
    }
    s
}

/// Same, for symlinks shown as `link -> target`.
fn fmt_symlink_sample(sample: &[(PathBuf, PathBuf)], total: usize) -> String {
    let mut s = sample
        .iter()
        .map(|(l, t)| format!("{} -> {}", l.display(), t.display()))
        .collect::<Vec<_>>()
        .join(", ");
    if total > sample.len() {
        s.push_str(&format!(" (+{} more)", total - sample.len()));
    }
    s
}

/// `status`, the setup doctor. It deliberately does its OWN, resilient loading
/// (it is dispatched in `run` BEFORE the strict `Config::load_dir`): a broken
/// configuration is the very thing it must diagnose, so it first checks the
/// environment and validates the config files, and only then — if they are
/// sound — runs the full health report against a loaded `Config`.
/// The official Slackware locate database (mlocate, a/ series). A single fixed
/// path — plocate lives only on SBo and is not what the official system ships.
const MLOCATE_DB: &str = "/var/lib/mlocate/mlocate.db";

/// How old the locate database may be before `status` flags it. updatedb runs
/// from /etc/cron.daily, so one missed daily slot (machine down at ~04:40) is
/// tolerated; past this the daily job is probably not running.
const LOCATE_DB_MAX_AGE_HOURS: u64 = 36;

/// The verdict for the locate-database freshness check.
#[derive(Debug, PartialEq, Eq)]
enum LocateDb {
    /// Database present and refreshed within the age budget; carries its age in hours.
    Fresh(u64),
    /// Database present but older than the budget; carries its age in hours.
    Stale(u64),
    /// No database file at all (updatedb has never run).
    Missing,
    /// The file exists but its metadata could not be read (root:slocate 0640,
    /// so a non-root run cannot stat it) — reported as "not checked", never as a
    /// problem.
    Unreadable,
}

/// Pure verdict from the database's modified-time and the current time, both as
/// seconds since the Unix epoch. Split out so the age/threshold logic is
/// unit-testable without touching the filesystem or the clock. A modified time
/// in the future (clock skew) is treated as age 0 (Fresh), never as underflow.
fn locate_db_verdict(now_secs: u64, mtime_secs: u64) -> LocateDb {
    let age_h = now_secs.saturating_sub(mtime_secs) / 3600;
    if age_h <= LOCATE_DB_MAX_AGE_HOURS {
        LocateDb::Fresh(age_h)
    } else {
        LocateDb::Stale(age_h)
    }
}

/// Read the mlocate database's age, mapping the IO outcomes onto [`LocateDb`]:
/// a missing file is `Missing`; a present file whose metadata/mtime cannot be
/// read (permission denied for non-root, since it is root:slocate 0640) is
/// `Unreadable`; otherwise the pure [`locate_db_verdict`] decides Fresh/Stale.
fn locate_db_mtime_age() -> LocateDb {
    let path = std::path::Path::new(MLOCATE_DB);
    let md = match std::fs::metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LocateDb::Missing,
        Err(_) => return LocateDb::Unreadable, // permission denied &c.
    };
    let mtime = match md.modified().ok().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
    }) {
        Some(s) => s,
        None => return LocateDb::Unreadable,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(mtime); // clock unreadable -> treat as fresh, never panic
    locate_db_verdict(now, mtime)
}

pub(crate) fn cmd_status(config_dir: &std::path::Path) -> Result<Outcome, String> {
    let ok = ui::green("\u{2713}"); // ✓
    let bad = ui::red("\u{2717}"); // ✗
    let warn = ui::yellow("!");
    let info_env = ui::blue("\u{00B7}"); // ·
    let srow = |mark: &str, label: &str, detail: &str| {
        println!("  {} {} {}", mark, ui::white(&format!("{:<10}", label)), detail);
    };

    println!("{}", ui::blue("Environment"));

    // The config directory must exist before anything else can be read.
    if !config_dir.exists() {
        srow(&bad, "Config dir", &ui::red(&format!("{} does not exist", config_dir.display())));
        println!(
            "\n{}",
            ui::yellow("! create it with a `repos` file (see `man slacker`), then re-run `slacker status`.")
        );
        return Err("configuration directory is missing".into());
    }

    // External tools slacker shells out to. The pkgtools are essential — without
    // them install/upgrade/remove cannot run at all; the rest each degrade one
    // feature when absent. Looked up on $PATH, not executed; the pkgtools also in
    // /sbin, which is not on a plain user's PATH.
    let missing_pt: Vec<&str> = ["installpkg", "upgradepkg", "removepkg"]
        .into_iter()
        .filter(|&t| !pkgtool_present(t, Path::new(PKGTOOLS_DIR)))
        .collect();
    if missing_pt.is_empty() {
        srow(&ok, "Pkgtools", &ui::dim("installpkg, upgradepkg, removepkg present"));
    } else {
        srow(
            &bad,
            "Pkgtools",
            &ui::red(&format!(
                "MISSING: {} — install/upgrade/remove cannot run",
                missing_pt.join(", ")
            )),
        );
    }
    let missing_aux: Vec<String> = [
        ("gpg", "signature verification"),
        ("bzip2", "file-search (MANIFEST)"),
        ("sha256sum", "sha256 checks"),
        ("diff", "new-config diffs"),
    ]
    .into_iter()
    .filter(|&(t, _)| !tool_on_path(t))
    .map(|(t, why)| format!("{t} ({why})"))
    .collect();
    if missing_aux.is_empty() {
        srow(&ok, "Tools", &ui::dim("gpg, bzip2, sha256sum, diff present"));
    } else {
        srow(&warn, "Tools", &ui::yellow(&format!("missing: {}", missing_aux.join(", "))));
    }

    // The locate(1) database. Slackware ships mlocate (a/ series); its updatedb
    // runs from /etc/cron.daily, so a database older than a day means the daily
    // job has not run (cron off, or the machine was down at ~04:40) and locate
    // results are stale. This is advisory only — mlocate is not needed by
    // slacker itself. The DB is root:slocate 0640, so a non-root run usually
    // cannot stat it: report that honestly ("needs root") rather than guessing,
    // exactly as elsewhere. Only checked when mlocate is actually installed.
    if tool_on_path("updatedb") || tool_on_path("locate") {
        match locate_db_mtime_age() {
            LocateDb::Fresh(age_h) => {
                srow(&ok, "Locate DB", &ui::dim(&format!("mlocate database is current ({age_h}h old)")));
            }
            LocateDb::Stale(age_h) => {
                srow(
                    &warn,
                    "Locate DB",
                    &ui::yellow(&format!(
                        "mlocate database is {age_h}h old — the daily updatedb may not be running; refresh it with `updatedb` as root"
                    )),
                );
            }
            LocateDb::Missing => {
                srow(
                    &warn,
                    "Locate DB",
                    &ui::yellow("mlocate database not built yet — create it with `updatedb` as root"),
                );
            }
            LocateDb::Unreadable => {
                srow(
                    &info_env,
                    "Locate DB",
                    &ui::dim("not checked (the mlocate database is readable only by root — re-run as root)"),
                );
            }
        }
    }

    // Config syntax + cross-checks, via the SAME validator the `add-repo`/`add-tag`
    // editors use. This catches a line that does not start with a priority number,
    // duplicate priorities, a second `official`, a bad `verify=`, a `mirror` line
    // with no active mirror, duplicate names/tags, and an empty repo set — the
    // problems that otherwise abort every other command before it can start.
    let repos_text = std::fs::read_to_string(config_dir.join("repos")).unwrap_or_default();
    match config::validate_repos_text(config_dir, &repos_text) {
        Ok(()) => srow(&ok, "Config", &ui::dim("repos and mirrors parse and cross-check cleanly")),
        Err(e) => {
            srow(&bad, "Config", &ui::red(&e));
            println!(
                "\n{}",
                ui::yellow("! fix the `repos`/`mirrors` problem above, then re-run `slacker status`.")
            );
            return Err("configuration is invalid".into());
        }
    }

    // Everything needed to load is present and valid — run the full report.
    let cfg = Config::load_dir(config_dir)?;
    migrate_state(&cfg);
    println!();
    status_full(&cfg)
}

/// osuosl reference PACKAGES.TXT for the FRESHNESS check. On -current the main
/// tree moves, so we read the release root; on STABLE the root is a frozen
/// snapshot and the `patches/` subtree is what actually changes when security
/// updates land, so we read patches/PACKAGES.TXT. `dir` is the
/// `slackware{64}-{release}` segment.
fn osuosl_freshness_url(dir: &str, is_current: bool) -> String {
    let sub = if is_current { "" } else { "/patches" };
    format!("http://ftp.osuosl.org/pub/slackware/{dir}{sub}/PACKAGES.TXT")
}

/// The (upstream, your-mirror) PACKAGES.TXT pair the freshness check compares,
/// or None if the release/arch can't be determined, osuosl carries no tree for
/// this arch (Slackware ARM), or there is no official mirror. -current → the
/// main-tree root on both sides; STABLE → the `patches/` tree on both sides
/// (the frozen root never moves; patches/ is where the server
/// publishes security updates, so that is the timestamp that signals freshness).
/// On stable the "your" side prefers a configured `patches` subtree repo, falling
/// back to deriving patches/ from the official mirror base.
fn freshness_urls(cfg: &Config) -> Option<(String, String)> {
    let official = cfg.repos.iter().find(|r| r.official)?;
    // Freshness checks YOUR MIRROR, so key everything off the mirror's own
    // release (read from its URL), not the running system's os-release — the two
    // can disagree mid dist-upgrade (mirror already re-pointed to -current while
    // os-release still says 15.0), and comparing across releases is meaningless.
    let mirror_release = dist::parse_release_from_url(&official.url)?;
    let is_current = matches!(mirror_release, dist::Release::Current);
    let prefix = osuosl_tree_prefix(&cfg.arch)?;
    let dir = format!("{prefix}-{}", dist::release_suffix(&mirror_release));
    let upstream = osuosl_freshness_url(&dir, is_current);

    let mine = if is_current {
        official.join_url(repo::PACKAGES_TXT)
    } else {
        match cfg.repos.iter().find(|r| r.subtree && r.url.contains("/patches")) {
            Some(p) => p.join_url(repo::PACKAGES_TXT),
            None => format!(
                "{}/patches/{}",
                official.url.trim_end_matches('/'),
                repo::PACKAGES_TXT
            ),
        }
    };
    Some((upstream, mine))
}

/// The downloaded-package cache is reported in `status` as worth cleaning when it
/// grows past this size, or when nothing has been fetched into it for this many
/// days (large, or simply forgotten). `clean-cache` is the remedy.
const CACHE_WARN_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB
const CACHE_STALE_DAYS: u64 = 30;

fn status_full(cfg: &Config) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let (db, missing_meta) = PkgDb::load_available(cfg);
    let repos = cfg.repos_by_priority();
    // State flags feeding the ordered "next steps" recipe at the end.
    let mut gpg_missing = false;
    let mut metadata_incomplete = false;
    let mut pending = false;
    let mut unreachable = false;
    let mut tampered: Vec<String> = Vec::new();
    let mut security_problem = false;

    let ok = ui::green("\u{2713}"); // ✓
    let bad = ui::red("\u{2717}"); // ✗
    let warn = ui::yellow("!");
    let info = ui::blue("\u{00B7}"); // ·
    let srow = |mark: &str, label: &str, detail: &str| {
        println!("  {} {} {}", mark, ui::white(&format!("{:<10}", label)), detail);
    };

    // ---------- Setup ----------
    println!("{}", ui::blue("Setup"));

    match cfg.repos.iter().find(|r| r.official) {
        Some(r) => srow(&ok, "Mirror", &ui::dim(&r.url)),
        None => srow(&info, "Mirror", &ui::dim("no official repo configured (third-party only)")),
    }

    // Mirror freshness vs upstream. Every other row reads local state; this is
    // the one that touches the network. On -current we compare the main-tree
    // PACKAGES.TXT; on STABLE the release root is frozen, so we compare the
    // patches/ PACKAGES.TXT — that is the timestamp that moves when the server
    // publishes security updates. Short timeout, FAILS OPEN: an unreachable or
    // unparseable side never yields a false "stale" verdict and never blocks.
    if let Some((up_url, mine_url)) = freshness_urls(cfg) {
        let t = std::time::Duration::from_secs(FRESHNESS_TIMEOUT_SECS);
        let up = download::first_line(&up_url, t)
            .ok()
            .and_then(|l| parse_packages_date(&l));
        let mine = download::first_line(&mine_url, t)
            .ok()
            .and_then(|l| parse_packages_date(&l));
        match (up, mine) {
            (Some(up), Some(mine)) if mirror_is_stale(up, mine) => srow(
                &warn,
                "Freshness",
                &ui::yellow(&format!(
                    "your mirror is {} behind upstream — run `slacker find-mirror`",
                    humanize_lag(up - mine)
                )),
            ),
            (Some(_), Some(_)) => {
                srow(&ok, "Freshness", &ui::dim("mirror is in sync with upstream"))
            }
            _ => srow(
                &info,
                "Freshness",
                &ui::dim("could not check (upstream or mirror unreachable)"),
            ),
        }
    } else if osuosl_tree_prefix(&cfg.arch).is_none() && cfg.repos.iter().any(|r| r.official) {
        // Slackware ARM: the reference mirror carries no tree for this arch, so
        // there is nothing honest to compare against. Say so rather than stay
        // silent, and never compare against another architecture's tree.
        srow(
            &info,
            "Freshness",
            &ui::dim(&format!("not checked (no upstream reference for {})", cfg.arch)),
        );
    }

    srow(&ok, "Repos", &ui::dim(&format!("{} configured, priorities distinct", repos.len())));

    // Downloaded-package cache. It is never pruned automatically (only
    // upgrade-dist deletes as it installs), so it grows and is easy to forget.
    // Warn when it is large (>= CACHE_WARN_BYTES) OR stale (nothing fetched into
    // it for CACHE_STALE_DAYS), pointing at clean-cache. Reads only local state.
    {
        let (files, bytes, newest) = scan_package_cache(&cfg.cache_dir.join("packages"));
        if files == 0 {
            srow(&ok, "Cache", &ui::dim("empty"));
        } else {
            let size = humanize_bytes(bytes);
            let idle_secs = newest.and_then(|t| t.elapsed().ok()).map(|e| e.as_secs());
            let too_big = bytes >= CACHE_WARN_BYTES;
            let too_old = idle_secs.is_some_and(|s| s >= CACHE_STALE_DAYS * 86_400);
            if too_big || too_old {
                let age = match newest {
                    Some(t) if too_old => format!(", untouched {}", ago(t)),
                    _ => String::new(),
                };
                srow(
                    &warn,
                    "Cache",
                    &ui::yellow(&format!(
                        "{size} in {files} file(s){age} — run `slacker clean-cache`"
                    )),
                );
            } else {
                srow(&ok, "Cache", &ui::dim(&format!("{size} in {files} file(s)")));
            }
        }
    }

    if !cfg.tag_priorities.is_empty() {
        let tags =
            cfg.tag_priorities.iter().map(|t| t.tag.as_str()).collect::<Vec<_>>().join(", ");
        srow(&ok, "Tag rules", &ui::dim(&format!("{} ({tags})", cfg.tag_priorities.len())));
    }

    // Verification policy
    let unverified = unverified_repo_names(cfg);
    if unverified.is_empty() {
        srow(&ok, "Verify", &ui::green("every repo verifies packages"));
    } else {
        srow(&warn, "Verify", &ui::yellow(&format!("OFF for: {}", unverified.join(", "))));
    }

    // Transport security
    let insecure = insecure_http_repos(cfg);
    if insecure.is_empty() {
        srow(&ok, "Transport", &ui::dim("all repos use https or file"));
    } else {
        srow(
            &warn,
            "Transport",
            &ui::yellow(&format!(
                "plaintext http (MITM-able): {} — prefer https",
                insecure.join(", ")
            )),
        );
    }

    // Credentials transport: a repo carrying credentials= over plaintext http.
    // Without the `insecure` flag the login is NOT sent (fail-safe); with it,
    // the login is exposed on the wire — flagged either way.
    let is_http = |r: &&config::Repo| r.url.to_ascii_lowercase().starts_with("http://");
    let cred_blocked: Vec<String> = cfg
        .repos
        .iter()
        .filter(|r| r.credentials.is_some() && is_http(r) && !r.insecure)
        .map(|r| r.name.clone())
        .collect();
    let cred_insecure: Vec<String> = cfg
        .repos
        .iter()
        .filter(|r| r.credentials.is_some() && is_http(r) && r.insecure)
        .map(|r| r.name.clone())
        .collect();
    if !cred_blocked.is_empty() {
        srow(
            &bad,
            "Credentials",
            &ui::red(&format!(
                "credentials over plaintext http, NOT sent: {} — use https (or add `insecure` to force)",
                cred_blocked.join(", ")
            )),
        );
    } else if !cred_insecure.is_empty() {
        srow(
            &warn,
            "Credentials",
            &ui::yellow(&format!(
                "credentials sent over plaintext http (insecure): {} — exposed on the wire, at your own risk",
                cred_insecure.join(", ")
            )),
        );
    }

    // Release match: any active repo built for a different Slackware release than
    // this system is a foot-gun (release-mixing) — installing from it is refused.
    if let Some(sys) = system_release() {
        let mism: Vec<String> = cfg
            .repos
            .iter()
            .filter_map(|r| {
                repo_release_token(&r.url)
                    .filter(|rel| *rel != sys)
                    .map(|rel| format!("{} → {}", r.name, dist::show(&rel)))
            })
            .collect();
        if mism.is_empty() {
            srow(&ok, "Repo release", &ui::dim(&format!("all match this system ({})", dist::show(&sys))));
        } else {
            srow(
                &warn,
                "Repo release",
                &ui::yellow(&format!(
                    "MISMATCH (installs refused without --yes): {} — system is {}",
                    mism.join(", "),
                    dist::show(&sys)
                )),
            );
        }
    }

    // Arch match: a repo whose packages are ALL built for a foreign architecture
    // (ignoring noarch) is the wrong tree for this system — installs from it are
    // refused. Judged from the package arch field, never the URL (folder names
    // lie: alienbob uses /x86_64/, conraid slackware64-current, SBo none).
    {
        let sys_fam = arch_family(&cfg.arch);
        let foreign: Vec<String> = cfg
            .repos
            .iter()
            .filter_map(|r| {
                let archs = db.repo_archs(&r.name);
                let non_noarch: Vec<&str> =
                    archs.iter().copied().filter(|a| *a != "noarch").collect();
                if non_noarch.is_empty() {
                    return None; // only noarch, or no metadata — nothing to judge
                }
                if non_noarch.iter().any(|a| arch_family(a) == sys_fam) {
                    return None; // provides at least some packages for this arch
                }
                Some(format!("{} ({})", r.name, non_noarch.join("/")))
            })
            .collect();
        if foreign.is_empty() {
            srow(&ok, "Repo arch", &ui::dim(&format!("all match this system ({})", cfg.arch)));
        } else {
            srow(
                &warn,
                "Repo arch",
                &ui::yellow(&format!(
                    "MISMATCH (installs refused): {} — system is {}",
                    foreign.join(", "),
                    cfg.arch
                )),
            );
        }
    }

    // Frozen repos: hard (needs trust-repo) vs soft (unreachable, auto-retried)
    let hard: Vec<&config::Repo> = cfg
        .repos
        .iter()
        .filter(|r| repo::is_hard_quarantined(&cfg.state_dir, &r.name))
        .collect();
    let soft: Vec<&config::Repo> = cfg
        .repos
        .iter()
        .filter(|r| {
            repo::is_quarantined(&cfg.state_dir, &r.name)
                && !repo::is_hard_quarantined(&cfg.state_dir, &r.name)
        })
        .collect();
    if hard.is_empty() && soft.is_empty() {
        srow(&ok, "Repo trust", &ui::dim("no repos are frozen"));
    } else {
        if !hard.is_empty() {
            let names: Vec<String> = hard.iter().map(|r| r.name.clone()).collect();
            srow(
                &warn,
                "Repo trust",
                &ui::red(&format!(
                    "FROZEN (unsafe, unused): {} — trust with `trust-repo NAME`, or `del-repo NAME`",
                    names.join(", ")
                )),
            );
            for r in &hard {
                if let Some(reason) = repo::quarantine_reason(&cfg.state_dir, &r.name) {
                    println!("             {}", ui::dim(&format!("{}: {reason}", r.name)));
                }
            }
        }
        if !soft.is_empty() {
            let names: Vec<String> = soft.iter().map(|r| r.name.clone()).collect();
            srow(
                &warn,
                "Repo trust",
                &ui::yellow(&format!(
                    "unreachable (will retry on next update): {}",
                    names.join(", ")
                )),
            );
        }
    }

    // GPG keys — verify EMPIRICALLY. A repo is "covered" if its cached CHECKSUMS
    // signature verifies against our keyring, regardless of WHICH key signed it:
    // extras/testing/patches reuse slackware's key, so they verify without a key
    // of their own. Only repos whose policy wants gpg are relevant, and the
    // keyring is root-only, so a non-root run can't check.
    let need_gpg: Vec<&config::Repo> = repos
        .iter()
        .filter(|r| r.verify_policy(&cfg.verify).wants(config::Check::Gpg))
        .copied()
        .collect();
    if !need_gpg.is_empty() {
        let keyring = cfg.state_dir.join("gpg");
        let keyring_has_keys = std::fs::read_dir(&keyring)
            .map(|d| {
                d.flatten().any(|e| e.file_name().to_string_lossy().ends_with("-GPG-KEY"))
            })
            .unwrap_or(false);
        if current_uid() != Some(0) {
            srow(&info, "GPG keys", &ui::dim("cannot verify without root"));
        } else if !keyring_has_keys {
            srow(&warn, "GPG keys", &ui::yellow("none imported yet"));
            gpg_missing = true;
        } else {
            // Keyring populated and we're root: actually verify each repo's
            // signature. Good = covered (by any key); NoSignature = none cached
            // yet / repo ships none (md5 covers it); "no public key" = its signer
            // really isn't imported; BADSIG = tampering.
            let mut verified: Vec<&str> = Vec::new();
            let mut nosig: Vec<&str> = Vec::new();
            let mut nokey: Vec<&str> = Vec::new();
            let mut errd: Vec<&str> = Vec::new();
            for r in &need_gpg {
                // A repo with no cached metadata has nothing to verify yet —
                // don't count it as "md5-covered"; the Metadata row already
                // flags it. (e.g. a freshly added or wrong-URL repo.)
                if missing_meta.iter().any(|m| m == &r.name) {
                    continue;
                }
                match gpg::verify_checksums(r, &cfg.cache_dir, &cfg.state_dir) {
                    Ok(gpg::Verify::Good(_)) => verified.push(r.name.as_str()),
                    Ok(gpg::Verify::NoSignature) => nosig.push(r.name.as_str()),
                    Ok(gpg::Verify::Tampered(_)) => tampered.push(r.name.clone()),
                    Ok(gpg::Verify::Unverifiable(m)) if m.contains("no public key") => {
                        nokey.push(r.name.as_str())
                    }
                    Ok(gpg::Verify::Unverifiable(_)) => errd.push(r.name.as_str()),
                    Err(_) => errd.push(r.name.as_str()),
                }
            }
            if !tampered.is_empty() {
                srow(&bad, "GPG keys", &ui::red(&format!("BAD signature: {}", tampered.join(", "))));
            } else if !nokey.is_empty() {
                srow(&warn, "GPG keys", &ui::yellow(&format!("no imported key verifies: {}", nokey.join(", "))));
                gpg_missing = true;
            } else if !verified.is_empty() {
                let extra = if nosig.is_empty() {
                    String::new()
                } else {
                    format!(" (+{} via md5)", nosig.len())
                };
                srow(&ok, "GPG keys", &ui::green(&format!("{} repo(s) verify{extra}", verified.len())));
            } else if !nosig.is_empty() {
                srow(&info, "GPG keys", &ui::dim("no signatures fetched yet (run `slacker update`)"));
            } else {
                srow(&info, "GPG keys", &ui::dim(&format!("could not verify: {}", errd.join(", "))));
            }
        }
    }

    // Metadata freshness — has `update` been run, and how long ago?
    let mut cached = 0usize;
    let mut oldest: Option<std::time::SystemTime> = None;
    for r in &repos {
        let p = repo::meta_path(r, &cfg.cache_dir, repo::PACKAGES_TXT);
        if let Ok(meta) = std::fs::metadata(&p) {
            cached += 1;
            if let Ok(mtime) = meta.modified() {
                oldest = Some(match oldest {
                    Some(o) if o < mtime => o,
                    _ => mtime,
                });
            }
        }
    }
    if cached == 0 {
        srow(&bad, "Metadata", &ui::red("not downloaded yet"));
        metadata_incomplete = true;
    } else if cached < repos.len() {
        srow(&warn, "Metadata", &ui::yellow(&format!("cached for {}/{} repos", cached, repos.len())));
        metadata_incomplete = true;
    } else {
        let age = oldest.map(ago).unwrap_or_else(|| "?".into());
        srow(&ok, "Metadata", &ui::dim(&format!("cached for all repos (oldest {age} old)")));
    }

    // Blacklist — show valid rules, and surface any the parser had to skip so a
    // typo'd rule is visible instead of silently vanishing. The comment/blank
    // handling mirrors config's own parse_lines, so the count matches the loader.
    let n_bl = cfg.blacklist.len();
    let bl_invalid = std::fs::read_to_string(cfg.config_dir.join("blacklist"))
        .map(|t| {
            t.lines()
                .map(|l| match l.find('#') {
                    Some(i) => l[..i].trim(),
                    None => l.trim(),
                })
                .filter(|l| !l.is_empty())
                .filter(|l| config::parse_blacklist_rule(l).is_err())
                .count()
        })
        .unwrap_or(0);
    if bl_invalid == 0 {
        srow(if n_bl == 0 { &info } else { &ok }, "Blacklist", &ui::dim(&format!("{n_bl} rule(s)")));
    } else {
        srow(
            &warn,
            "Blacklist",
            &ui::yellow(&format!("{n_bl} valid, {bl_invalid} invalid — check syntax in `blacklist`")),
        );
    }

    // Pins (`@repo 100% pkg`) — flag any whose repo is not active, since such a
    // pin has no effect until that repo exists.
    let pins = cfg.pins();
    if !pins.is_empty() {
        let active: HashSet<&str> = cfg.repos.iter().map(|r| r.name.as_str()).collect();
        let dangling: Vec<String> = pins
            .iter()
            .filter(|(_, repo)| !active.contains(repo))
            .map(|(pkg, repo)| format!("{pkg} -> {repo}"))
            .collect();
        if dangling.is_empty() {
            srow(&ok, "Pins", &ui::dim(&format!("{} pin(s)", pins.len())));
        } else {
            srow(
                &warn,
                "Pins",
                &ui::yellow(&format!(
                    "{} pin(s), {} to an inactive repo (no effect): {}",
                    pins.len(),
                    dangling.len(),
                    dangling.join(", ")
                )),
            );
            println!(
                "             {}",
                ui::dim("activate the repo, re-pin elsewhere, or `slacker unpin <pkg>`")
            );
        }
    }

    // Build-tag priority coverage. An installed package whose tag has neither a
    // priority line in `repos` nor any configured repo shipping that tag cannot
    // be ranked, so the priority rule protects it (installed_priority is maximal)
    // and nothing will ever replace it — a freeze the user never asked for and
    // cannot see. Surface it here, grouped by tag, since the fix is one line per
    // tag rather than one action per package.
    //
    // A package the user has explicitly frozen is skipped: keeping it is then a
    // stated intention, not an unnoticed side effect of a missing priority, and
    // reporting it would be nagging about something already decided.
    let mut unranked_tags: Vec<String> = Vec::new();
    let mut unranked_pkgs = 0usize;
    {
        let mut by_tag: HashMap<&str, Vec<&str>> = HashMap::new();
        for i in &installed {
            let tag = i.build_tag();
            if tag.is_empty() || bl_installed(cfg, Some(&db), i) {
                continue;
            }
            if db.installed_priority(i, &cfg.tag_priorities) == i32::MAX {
                by_tag.entry(tag).or_default().push(i.name.as_str());
            }
        }
        if by_tag.is_empty() {
            srow(&ok, "Tag prio", &ui::dim("every installed package's source can be ranked"));
        } else {
            let mut tags: Vec<(&&str, &Vec<&str>)> = by_tag.iter().collect();
            tags.sort_by_key(|(t, _)| **t);
            unranked_pkgs = by_tag.values().map(Vec::len).sum();
            unranked_tags = tags.iter().map(|(t, _)| (**t).to_string()).collect();
            srow(
                &warn,
                "Tag prio",
                &ui::yellow(&format!(
                    "{unranked_pkgs} installed package(s) from {} unranked tag(s) — kept for safety, never upgraded",
                    tags.len()
                )),
            );
            for (tag, names) in tags {
                // Long lists are noise; a handful names the culprit well enough.
                let shown = if names.len() > 6 {
                    format!("{}, +{} more", names[..6].join(", "), names.len() - 6)
                } else {
                    names.join(", ")
                };
                println!(
                    "             {}",
                    ui::dim(&format!(
                        "{tag}: {shown} — add `100  {}  {tag}` to `repos`, or removepkg and install from a repo",
                        tag.trim_start_matches('_')
                    ))
                );
            }
        }
    }

    // Leftover `*.new` config files from past upgrades, waiting to be reconciled.
    let pending_new = newconfig::find_new_configs(&newconfig::default_roots()).len();
    if pending_new > 0 {
        srow(
            &warn,
            "Configs",
            &ui::yellow(&format!("{pending_new} pending .new file(s) — run `slacker new-config`")),
        );
    }

    // Integrity of slacker's OWN files: ownership, write-exposure, stray symlinks.
    // Scoped strictly to config + cache + state (NOT the system package DB, which
    // the pkgtools own). A compromise of the state dir defeats GPG pinning and the
    // trust markers, so it is reported prominently with the exact fix.
    let mut audit_roots: Vec<&Path> = vec![cfg.config_dir.as_path(), cfg.cache_dir.as_path()];
    if cfg.state_dir != cfg.cache_dir {
        audit_roots.push(cfg.state_dir.as_path());
    }
    let audit = audit_owned_paths(&audit_roots);
    if audit.severe() {
        security_problem = true;
    }
    let fix_own = format!("chown -R root:root {} {}", cfg.config_dir.display(), cfg.cache_dir.display());
    let fix_w = format!("chmod -R go-w {} {}", cfg.config_dir.display(), cfg.cache_dir.display());
    if audit.clean() {
        let tail = if audit.unreadable > 0 {
            format!(
                " ({} checked, {} dir(s) need root to read — re-run as root for a full audit)",
                audit.checked, audit.unreadable
            )
        } else {
            format!(" ({} checked)", audit.checked)
        };
        srow(
            if audit.unreadable > 0 { &info } else { &ok },
            "Integrity",
            &ui::dim(&format!("config + cache + state root-owned, no world-writable paths, no stray symlinks{tail}")),
        );
    } else {
        if audit.symlinks > 0 {
            srow(
                &bad,
                "Symlinks",
                &ui::red(&format!(
                    "{} unexpected symlink(s) — slacker plants none here; investigate before trusting the cache: {}",
                    audit.symlinks,
                    fmt_symlink_sample(&audit.sample_symlinks, audit.symlinks)
                )),
            );
        }
        if audit.world_writable > 0 {
            srow(
                &bad,
                "Writable",
                &ui::red(&format!(
                    "{} world-writable path(s) — anyone can tamper (e.g. {}); fix: {fix_w}",
                    audit.world_writable,
                    fmt_path_sample(&audit.sample_world_writable, audit.world_writable)
                )),
            );
        }
        if audit.group_writable > 0 {
            srow(
                &warn,
                "Writable",
                &ui::yellow(&format!(
                    "{} group-writable path(s) (e.g. {}); fix: {fix_w}",
                    audit.group_writable,
                    fmt_path_sample(&audit.sample_group_writable, audit.group_writable)
                )),
            );
        }
        if audit.not_root > 0 {
            srow(
                &warn,
                "Ownership",
                &ui::yellow(&format!(
                    "{} path(s) not owned by root (e.g. {}); fix: {fix_own}",
                    audit.not_root,
                    fmt_path_sample(&audit.sample_not_root, audit.not_root)
                )),
            );
        }
        if audit.unreadable > 0 {
            srow(
                &info,
                "Integrity",
                &ui::dim(&format!(
                    "{} dir(s) need root to read — re-run as root for a full audit",
                    audit.unreadable
                )),
            );
        }
    }

    // ---------- Installed ----------
    println!("\n{}", ui::blue("Installed"));
    if !cfg.pkg_db_dir.exists() {
        srow(
            &bad,
            "Admin dir",
            &ui::red(&format!(
                "{} does not exist — slacker sees no installed packages",
                cfg.pkg_db_dir.display()
            )),
        );
    }
    let (per_repo, per_rule, per_other) = installed_attribution(cfg, Some(&db), &installed);
    srow(&ok, "Packages", &ui::white(&installed.len().to_string()));
    let mut parts: Vec<String> = repos
        .iter()
        .map(|r| {
            // Isolate a repo with no metadata as `?` instead of a misleading 0;
            // the rest report their real counts.
            if missing_meta.iter().any(|m| m == &r.name) {
                format!("{} ?", r.name)
            } else {
                format!("{} {}", r.name, per_repo.get(&r.name).copied().unwrap_or(0))
            }
        })
        .collect();
    // Declared tag-priority rules (SBo, local, ...) are named sources too.
    let mut seen_rule = std::collections::HashSet::new();
    for tp in &cfg.tag_priorities {
        if seen_rule.insert(tp.name.as_str()) {
            parts.push(format!("{} {}", tp.name, per_rule.get(&tp.name).copied().unwrap_or(0)));
        }
    }
    // Any remaining build tags are legitimate sources, shown by their tag.
    let mut others: Vec<(&String, &usize)> = per_other.iter().collect();
    others.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (t, c) in others {
        parts.push(format!("{t} {c}"));
    }
    srow(&info, "By source", &ui::dim(&parts.join(", ")));
    if !missing_meta.is_empty() {
        srow(
            &info,
            "",
            &ui::yellow(&format!(
                "? = no metadata yet ({}) — run `slacker update`, or check the repo URL",
                missing_meta.join(", ")
            )),
        );
    }

    // ---------- Online ----------
    println!("\n{}", ui::blue("Online"));
    let probe_repo = cfg.repos.iter().find(|r| r.official).or_else(|| repos.first().copied());
    match probe_repo {
        None => srow(&info, "Connection", &ui::dim("no repo to probe")),
        Some(r) => {
            // The same small file check-updates compares: ChangeLog for the
            // official repo, otherwise CHECKSUMS.md5.
            let file = if r.official { repo::CHANGELOG } else { repo::CHECKSUMS };
            let url = r.join_url(file);
            match download::get_bytes(&url) {
                Ok(remote) => {
                    srow(
                        &ok,
                        "Reachable",
                        &ui::dim(&format!("{} ({} KB)", r.name, remote.len() / 1024)),
                    );
                    let cached_file = std::fs::read(repo::meta_path(r, &cfg.cache_dir, file)).ok();
                    match cached_file {
                        Some(local) if local == remote => {
                            srow(&ok, "Updates", &ui::green(&format!("{} is up to date", r.name)))
                        }
                        Some(_) => {
                            srow(&warn, "Updates", &ui::yellow(&format!("{} has pending changes", r.name)));
                            pending = true;
                        }
                        None => srow(&warn, "Updates", &ui::yellow("not synced yet")),
                    }
                    srow(&info, "All repos", &ui::dim("run `slacker check-updates` for every repo"));
                }
                Err(e) => {
                    srow(&bad, "Reachable", &ui::red(&format!("{} unreachable: {e}", r.name)));
                    unreachable = true;
                }
            }
        }
    }

    // ---------- Stock-db (resolve-stock only) ----------
    if cfg.resolve_stock {
        let path = cfg.stock_db_path();
        if !path.exists() {
            srow(
                &bad,
                "Stock-db",
                &ui::red("no database installed \u{2014} run `slacker update`, then `slacker status`"),
            );
        } else if !stockdb::is_valid_db(&path) {
            srow(
                &bad,
                "Stock-db",
                &ui::red("present but invalid (not a database) \u{2014} run `slacker update`"),
            );
        } else {
            let base = cfg.stock_db_url.trim_end_matches('/');
            let stamp = if base.is_empty() {
                None
            } else {
                download::get_bytes(&format!("{base}/README.md"))
                    .ok()
                    .and_then(|b| stockdb::readme_stamp(&String::from_utf8_lossy(&b)))
            };
            let head = changelog::changelog_repo(&cfg.repos)
                .and_then(|r| changelog::cached_changelog(r, &cfg.cache_dir))
                .and_then(|c| stockdb::changelog_head(&c));
            match (stamp, head) {
                (Some(s), Some(h)) if s == h => {
                    srow(&ok, "Stock-db", &ui::green("present, valid, in sync with -current"))
                }
                (Some(_), Some(_)) => srow(
                    &warn,
                    "Stock-db",
                    &ui::yellow("present but behind -current \u{2014} run `slacker update`"),
                ),
                _ => srow(&ok, "Stock-db", &ui::dim("present, valid (sync not verified)")),
            }
        }
    }

    // ---------- Verdict ----------
    // Recommended commands in the canonical post-install order:
    //   update gpg -> update -> install-new -> upgrade-all.
    let mut steps: Vec<&str> = Vec::new();
    if gpg_missing {
        steps.push("slacker update gpg");
    }
    // `update` comes first whenever the local cache is not in step with the
    // repos: either no metadata yet (fresh/unsynced) OR the online repo has
    // changes the cache has not pulled (pending). install-new/upgrade-all read
    // the cache, so it must be refreshed before them.
    if metadata_incomplete || pending {
        steps.push("slacker update");
    }
    // A fresh/unsynced system, or one with pending changes, installs the newly
    // added packages BEFORE upgrading — install-new always precedes upgrade-all,
    // because new packages may be dependencies of the upgrades.
    if metadata_incomplete || pending {
        steps.push("slacker install-new");
        steps.push("slacker upgrade-all");
    }
    // Advisories: real issues that are not a command to run in this sequence.
    let mut notes: Vec<String> = Vec::new();
    if !tampered.is_empty() {
        notes.push(format!(
            "BAD GPG signature for {} — possible tampering; do NOT install from it until resolved",
            tampered.join(", ")
        ));
    }
    if !unverified.is_empty() {
        notes.push(format!(
            "verification is OFF for {} — set VERIFY=all in slacker.conf or verify= per repo",
            unverified.join(", ")
        ));
    }
    if unreachable {
        notes.push(
            "the mirror did not respond — check your network or the active line in `mirrors`".into(),
        );
    }
    if !unranked_tags.is_empty() {
        notes.push(format!(
            "{unranked_pkgs} installed package(s) carry a build tag with no priority ({}) — they cannot be ranked, so they are kept and never upgraded; give each tag a priority in `repos`, or removepkg and install from a repo",
            unranked_tags.join(", ")
        ));
    }
    if security_problem {
        notes.push(
            "slacker's OWN files have unsafe permissions or unexpected symlinks (see above) — resolve before installing; the package and trust cache cannot be trusted until then".into(),
        );
    }

    println!();
    // A "fresh" system is one that is correctly configured and safe, just not
    // synced yet — the expected state on a first run, BEFORE any `update`. We
    // greet it as ready rather than listing problems.
    let fresh_setup = metadata_incomplete && !security_problem && tampered.is_empty();
    if steps.is_empty() && notes.is_empty() {
        println!("{}", ui::green("\u{2713} slacker is set up correctly."));
    } else if steps.is_empty() {
        println!("{}", ui::blue("slacker is set up, with notes:"));
        for n in &notes {
            println!("  {} {}", ui::yellow("!"), ui::dim(n));
        }
    } else {
        let header = if fresh_setup {
            "Setup looks correct \u{2014} you're ready. Start with your first update, in order:"
        } else {
            "slacker is configured. Recommended next steps, in order:"
        };
        println!("{}", ui::blue(header));
        for s in &steps {
            println!("  {} {}", ui::yellow("\u{2192}"), ui::white(s));
        }
        for n in &notes {
            println!("  {} {}", ui::yellow("!"), ui::dim(n));
        }
        if metadata_incomplete {
            println!(
                "  {} {}",
                ui::blue("\u{00B7}"),
                ui::dim("after that, re-run `slacker status` to verify the rest")
            );
        }
    }
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_check_updates(cfg: &Config) -> Result<Outcome, String> {
    if cfg.repos.is_empty() {
        return Err("no repos configured".into());
    }
    let width = cfg.repos.iter().map(|r| r.name.len()).max().unwrap_or(8);
    let mut any_pending = false;
    let mut any_unknown = false;
    for r in cfg.repos_by_priority() {
        let label = match changelog::check_repo_updates(r, &cfg.cache_dir) {
            changelog::UpdateStatus::UpToDate => ui::green("up-to-date"),
            changelog::UpdateStatus::Pending => {
                any_pending = true;
                ui::yellow("updates pending")
            }
            changelog::UpdateStatus::Unknown => {
                any_unknown = true;
                ui::dim("unknown (run update first)")
            }
            // Kept in the same bucket as Unknown so the exit code is unchanged;
            // only the wording differs, because "run update first" is useless
            // advice for a repo that cannot be reached at all.
            changelog::UpdateStatus::Unreachable => {
                any_unknown = true;
                ui::red("unreachable (check its URL)")
            }
        };
        println!("  {}  {label}", ui::white(&format!("{:<width$}", r.name)));
    }
    warn_unverified_repos(cfg);
    if any_pending {
        println!("\n{}", ui::blue("Run `slacker update` then `slacker upgrade-all`."));
        Ok(Outcome::Pending)
    } else if any_unknown {
        Ok(Outcome::Ok)
    } else {
        println!("\n{}", ui::green("Everything up-to-date."));
        Ok(Outcome::Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_in_dirs_detects_executables() {
        let dir = std::env::temp_dir().join("slacker_tool_in_dirs_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let exe = dir.join("faketool");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let dirs = vec![dir.clone()];
        assert!(tool_in_dirs("faketool", &dirs)); // present and executable
        assert!(!tool_in_dirs("absent", &dirs)); // not there at all

        // On unix a present-but-non-executable file must NOT count as a tool.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let plain = dir.join("plain");
            std::fs::write(&plain, b"x").unwrap();
            std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!tool_in_dirs("plain", &dirs));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pkgtools_found_in_sbin_off_path() {
        // A plain user's PATH lacks /sbin; the pkgtools must still count as
        // present when they sit in the sbin dir (a temp stand-in here, with a
        // name that is on nobody's real PATH).
        let sbin = std::env::temp_dir().join("slacker_pkgtool_sbin_test");
        let _ = std::fs::remove_dir_all(&sbin);
        std::fs::create_dir_all(&sbin).unwrap();
        let exe = sbin.join("slacker-fake-installpkg");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(pkgtool_present("slacker-fake-installpkg", &sbin));
        assert!(!pkgtool_present("slacker-fake-removepkg", &sbin));

        // A symlinked sbin (a future merged /usr: /sbin -> usr/sbin) still works.
        #[cfg(unix)]
        {
            let link = std::env::temp_dir().join("slacker_pkgtool_sbin_link");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&sbin, &link).unwrap();
            assert!(pkgtool_present("slacker-fake-installpkg", &link));
            let _ = std::fs::remove_file(&link);
        }
        let _ = std::fs::remove_dir_all(&sbin);
    }

    #[cfg(unix)]
    #[test]
    fn audit_flags_writable_and_symlinks_without_following() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join("slacker_audit_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();

        let normal = root.join("normal");
        std::fs::write(&normal, b"x").unwrap();
        std::fs::set_permissions(&normal, std::fs::Permissions::from_mode(0o644)).unwrap();

        let ww = root.join("sub").join("ww");
        std::fs::write(&ww, b"x").unwrap();
        std::fs::set_permissions(&ww, std::fs::Permissions::from_mode(0o666)).unwrap();

        // A symlink must be COUNTED but never followed (so its target is not
        // re-audited, and a link pointing outside the tree cannot lure us out).
        let link = root.join("link");
        std::os::unix::fs::symlink(&normal, &link).unwrap();

        let a = audit_owned_paths(&[root.as_path()]);
        assert!(a.world_writable >= 1, "world-writable file detected");
        assert!(a.symlinks >= 1, "stray symlink detected");
        assert!(a.severe(), "world-writable + symlink is a severe finding");
        assert!(!a.clean());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn humanize_bytes_scales() {
        assert_eq!(humanize_bytes(0), "0 B");
        assert_eq!(humanize_bytes(512), "512 B");
        assert_eq!(humanize_bytes(1024), "1 KiB");
        assert_eq!(humanize_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(humanize_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
        assert_eq!(humanize_bytes(CACHE_WARN_BYTES), "2.0 GiB");
    }

    #[test]
    fn scan_package_cache_counts_only_repo_package_files() {
        let tmp = std::env::temp_dir().join("slacker_cache_scan_test");
        let _ = std::fs::remove_dir_all(&tmp);
        let pkgs = tmp.join("packages");
        std::fs::create_dir_all(pkgs.join("slackware")).unwrap();
        std::fs::create_dir_all(pkgs.join("conraid")).unwrap();
        std::fs::write(pkgs.join("slackware").join("a-1-x86_64-1.txz"), vec![0u8; 100])
            .unwrap();
        std::fs::write(pkgs.join("conraid").join("b-2-x86_64-1cf.txz"), vec![0u8; 50])
            .unwrap();
        // A stray file directly under packages/ is not inside a repo dir -> ignored.
        std::fs::write(pkgs.join("stray"), vec![0u8; 9999]).unwrap();
        let (files, bytes, newest) = scan_package_cache(&pkgs);
        assert_eq!(files, 2);
        assert_eq!(bytes, 150);
        assert!(newest.is_some());
        // A missing cache reads as empty, never an error.
        let (f, b, n) = scan_package_cache(&tmp.join("nope"));
        assert_eq!((f, b), (0, 0));
        assert!(n.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn freshness_url_is_patches_on_stable_root_on_current() {
        // -current: the main-tree root moves, so read the release root.
        assert_eq!(
            osuosl_freshness_url("slackware64-current", true),
            "http://ftp.osuosl.org/pub/slackware/slackware64-current/PACKAGES.TXT"
        );
        // stable: the root is frozen, so read patches/ (where updates land).
        assert_eq!(
            osuosl_freshness_url("slackware64-15.0", false),
            "http://ftp.osuosl.org/pub/slackware/slackware64-15.0/patches/PACKAGES.TXT"
        );
    }
}

#[cfg(test)]
mod attribution_tests {
    use super::attribute_tags;
    use crate::config::TagPriority;
    use crate::pkg::PkgId;

    fn ids(names: &[&str]) -> Vec<PkgId> {
        names.iter().map(|n| PkgId::parse(n).unwrap()).collect()
    }
    fn rule(name: &str, tag: &str) -> TagPriority {
        TagPriority { name: name.into(), tag: tag.into(), priority: 100 }
    }

    #[test]
    fn every_installed_package_has_a_source_never_untracked() {
        let installed = ids(&[
            "aaa-1.0-x86_64-1",          // empty tag   -> official repo
            "bbb-2.0-x86_64-1",          // empty tag   -> official repo
            "vim-9.1-x86_64-1_SBo",      // declared    -> rule "SBo"
            "mc-4.8-x86_64-1_SBo",       // declared    -> rule "SBo"
            "asio-1.36.0-x86_64-1cf",    // repo-served -> "conraid"
            "slacker-0.3-x86_64-1_FRG",  // other tag   -> "_FRG"
            "myradio-1.0-x86_64-1_wsr",  // other tag   -> "_wsr"
        ]);
        let rules = vec![rule("SBo", "_SBo"), rule("local", "_YOURTAG")];
        let (per_repo, per_rule, per_other) = attribute_tags(
            Some("slackware"),
            &rules,
            |t| (t == "cf").then(|| "conraid".to_string()),
            &installed,
        );

        assert_eq!(per_repo.get("slackware"), Some(&2));
        assert_eq!(per_repo.get("conraid"), Some(&1));
        assert_eq!(per_rule.get("SBo"), Some(&2));
        // A declared rule with no matching package is simply absent (count 0),
        // not an error and not "untracked".
        assert_eq!(per_rule.get("local"), None);
        assert_eq!(per_other.get("_FRG"), Some(&1));
        assert_eq!(per_other.get("_wsr"), Some(&1));

        // Nothing is dropped: every installed package is attributed exactly once.
        let total: usize =
            per_repo.values().chain(per_rule.values()).chain(per_other.values()).sum();
        assert_eq!(total, installed.len());
        // There is no "untracked" bucket; the only "other" keys are real tags.
        assert!(!per_other.keys().any(|k| k.contains("untracked")));
    }
}

#[cfg(test)]
mod locate_db_tests {
    use super::*;

    const H: u64 = 3600;

    #[test]
    fn fresh_within_budget() {
        let now = 1_000_000 * H;
        // just built
        assert_eq!(locate_db_verdict(now, now), LocateDb::Fresh(0));
        // a normal daily run, ~5h ago
        assert_eq!(locate_db_verdict(now, now - 5 * H), LocateDb::Fresh(5));
        // exactly at the 36h boundary is still fresh (one missed slot tolerated)
        assert_eq!(
            locate_db_verdict(now, now - LOCATE_DB_MAX_AGE_HOURS * H),
            LocateDb::Fresh(LOCATE_DB_MAX_AGE_HOURS)
        );
    }

    #[test]
    fn stale_past_budget() {
        let now = 1_000_000 * H;
        // one hour past the boundary -> stale
        assert_eq!(
            locate_db_verdict(now, now - (LOCATE_DB_MAX_AGE_HOURS + 1) * H),
            LocateDb::Stale(LOCATE_DB_MAX_AGE_HOURS + 1)
        );
        // a week old -> stale
        assert_eq!(locate_db_verdict(now, now - 24 * 7 * H), LocateDb::Stale(168));
    }

    #[test]
    fn future_mtime_is_fresh_not_underflow() {
        // Clock skew: DB mtime slightly in the future must not underflow into a
        // huge "stale" age; saturating_sub pins it to age 0 (Fresh).
        let now = 1_000_000 * H;
        assert_eq!(locate_db_verdict(now, now + 5 * H), LocateDb::Fresh(0));
    }
}
