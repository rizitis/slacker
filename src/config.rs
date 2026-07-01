//! Configuration loaded from plain-text files under a config directory
//! (default `/etc/slacker`), in the spirit of slackpkg/slackpkg+:
//!
//!   slacker.conf   KEY=value globals (ARCH, CACHE_DIR, PKG_DB_DIR)
//!   repos          every repo, one per line: `priority  name  url  [official]`
//!   blacklist      one rule per line: `[@repo] REGEX` or `[@repo] series/`
//!
//! All repos — including the official Slackware mirror — live in the single
//! `repos` file with their priority in the same column, so the ordering is
//! visible at a glance. The official mirror is just a line tagged `official`;
//! its priority is set the same way as every other repo, which means it can
//! sit in first, second, or last place purely by its number.
//!
//! Everything a user edits stays human-readable plain text. No TOML, no
//! sourcing of shell — just simple line-based parsing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;

/// One integrity/authenticity check that can be applied to a repo's packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// GPG signature over the repo's CHECKSUMS file (authenticity).
    Gpg,
    /// Per-package md5 from CHECKSUMS.md5 (integrity).
    Md5,
    /// Per-package SHA-256 from CHECKSUMS.sha256, if the repo ships one.
    Sha,
}

impl Check {
    pub fn label(&self) -> &'static str {
        match self {
            Check::Gpg => "gpg",
            Check::Md5 => "md5",
            Check::Sha => "sha",
        }
    }
}

/// How thoroughly to verify a repo's packages.
///
/// - `All` (default): apply every check the repo actually provides, and never
///   fail merely because a method is absent ("best available", fail-closed on
///   any mismatch).
/// - `Required(list)`: apply exactly these checks. If one is requested but the
///   repo does not provide it, stop and tell the user how to relax it.
/// - `None`: no verification at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyPolicy {
    All,
    Required(Vec<Check>),
    None,
}

impl VerifyPolicy {
    /// Parse a `VERIFY=` / `verify=` value: `all`, `none`, or a comma list of
    /// `gpg,md5,sha`.
    pub fn parse(s: &str) -> Result<VerifyPolicy, String> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "" | "all" => Ok(VerifyPolicy::All),
            "none" => Ok(VerifyPolicy::None),
            _ => {
                let mut checks = Vec::new();
                for tok in s.split(',').map(|t| t.trim()).filter(|t| !t.is_empty()) {
                    let c = match tok {
                        "gpg" => Check::Gpg,
                        "md5" => Check::Md5,
                        "sha" | "sha256" => Check::Sha,
                        other => {
                            return Err(format!(
                                "unknown verify method '{other}' (use gpg, md5, sha, all, or none)"
                            ))
                        }
                    };
                    if !checks.contains(&c) {
                        checks.push(c);
                    }
                }
                if checks.is_empty() {
                    return Err("empty verify setting".into());
                }
                Ok(VerifyPolicy::Required(checks))
            }
        }
    }

    /// Should this check be attempted when the repo provides the data for it?
    pub fn wants(&self, c: Check) -> bool {
        match self {
            VerifyPolicy::All => true,
            VerifyPolicy::None => false,
            VerifyPolicy::Required(v) => v.contains(&c),
        }
    }

    /// Must this check be present (i.e. fail if the repo does not provide it)?
    pub fn requires(&self, c: Check) -> bool {
        match self {
            VerifyPolicy::All | VerifyPolicy::None => false,
            VerifyPolicy::Required(v) => v.contains(&c),
        }
    }
}

pub struct Config {
    pub arch: String,
    pub cache_dir: PathBuf,
    /// Directory holding persistent SECURITY STATE (GPG keyring + TOFU
    /// fingerprint pins, repo quarantine and trust markers). Separate from
    /// `cache_dir` on purpose: /var/cache is FHS-disposable (an admin or cron
    /// or systemd-tmpfiles sweep may wipe it), and losing the trust anchors
    /// would silently re-pin whatever key a repo serves on next contact. Trust
    /// state therefore lives under /var/lib, which FHS defines as persistent.
    pub state_dir: PathBuf,
    /// Directory holding the installed-package database.
    pub pkg_db_dir: PathBuf,
    /// Slackware pkgtools admin root (holds packages/, removed_packages/,
    /// scripts/, setup/). The installed-package DB defaults to `adm_dir/packages`;
    /// reserved so future features can read the sibling directories from here.
    #[allow(dead_code)]
    pub adm_dir: PathBuf,
    pub blacklist: Vec<BlacklistRule>,
    pub repos: Vec<Repo>,
    /// Resolve .dep files and pull in dependencies (RESOLVE_DEPS, default on).
    pub resolve_deps: bool,
    /// Build tags that clean-system treats as non-foreign (IGNORE_TAGS).
    pub ignore_tags: Vec<String>,
    /// Build-tag priorities for non-binary sources (SBo, local builds).
    pub tag_priorities: Vec<TagPriority>,
    /// The config directory itself (used to locate templates).
    pub config_dir: PathBuf,
    /// Global default verification policy (VERIFY in slacker.conf).
    pub verify: VerifyPolicy,
    /// Number of package downloads to run concurrently (MAX_PARALLEL in
    /// slacker.conf). Default 4; 1 disables parallelism. Clamped to 1..=16.
    pub max_parallel: usize,
    /// Master switch for the `revert-pkg` command (REVERT in slacker.conf,
    /// default on). When off, the command refuses outright, before touching
    /// removed-packages, the network, or anything else.
    pub revert_enabled: bool,
    /// Base URL of the cumulative archive that `revert-pkg` uses to fetch
    /// superseded -current packages (CUMULATIVE_URL in slacker.conf). This is a
    /// revert-only source: it is NEVER consulted by update/upgrade/install/
    /// check-updates, so its historical (older) packages cannot leak into the
    /// normal priority model.
    pub cumulative_url: String,
    /// Optional local mirror used as the SOURCE for `upgrade-dist`
    /// (DISTRO_UPGRADE_MIRROR in `distro-upgrade.conf`). When set, a distribution
    /// upgrade pulls the target release from here — a local http/file mirror, an
    /// NFS clone, or a mounted install ISO — instead of re-pointing the existing
    /// remote mirror. None = normal remote behaviour. Used ONLY by upgrade-dist.
    pub distro_upgrade_mirror: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Repo {
    pub name: String,
    pub url: String,
    pub priority: i32,
    pub official: bool,
    /// `immutable` flag on the repos line: clean-system never reports a package
    /// as foreign while it is attributed to this repo. For a tagged repo that
    /// means every package carrying its build tag; for a tagless repo it means
    /// every package it provides, by name (so a tagless repo can be kept whole).
    pub immutable: bool,
    /// `subtree` flag on the repos line: this repo is a Slackware distribution
    /// subtree (extra/, patches/, testing/, pasture/). Its PACKAGES.TXT and
    /// CHECKSUMS live in the subtree, but the package LOCATIONs are relative to
    /// the distribution ROOT (e.g. `./extra/foo`), and GPG-KEY lives at the
    /// root too. So packages and GPG-KEY are fetched against the parent of the
    /// repo URL, while metadata still comes from the URL itself.
    pub subtree: bool,
    /// Per-repo verification override (`verify=` on the repos line). None means
    /// "use the global VERIFY policy".
    pub verify: Option<VerifyPolicy>,
}

impl Repo {
    /// The verification policy that applies to this repo: its own override if
    /// present, otherwise the global default.
    pub fn verify_policy<'a>(&'a self, global: &'a VerifyPolicy) -> &'a VerifyPolicy {
        self.verify.as_ref().unwrap_or(global)
    }
}

/// A priority assigned to packages by their build tag, for sources that are not
/// binary repositories (SlackBuilds.org `_SBo`, your own `_rtz`, etc.). This
/// lets upgrade-all treat such packages as a prioritised "virtual source": a
/// package is only replaced by a candidate from a repo of *higher or equal*
/// priority, so SBo/local packages are never silently migrated to a lower
/// repo or downgraded.
#[derive(Debug, Clone)]
pub struct TagPriority {
    pub name: String,
    pub tag: String,
    pub priority: i32,
}

impl Config {
    /// Load configuration from a directory of plain-text files.
    pub fn load_dir(dir: &Path) -> Result<Config, String> {
        let conf = parse_keyvals(&read_optional(&dir.join("slacker.conf"))?);

        let cache_dir = conf
            .get("CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/cache/slacker"));
        // Persistent security state (GPG keyring + .fpr pins, quarantine/ and
        // trusted/ markers). Kept OUT of CACHE_DIR because /var/cache is
        // FHS-disposable; /var/lib is the persistent home for application state.
        let state_dir = conf
            .get("STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/lib/slacker"));
        // Slackware pkgtools admin root: holds packages/, removed_packages/,
        // scripts/, setup/ (some are symlinks resolving to different physical
        // locations, so /var/adm is the only dir that exposes the whole set by
        // name). The installed-package DB defaults to ADM_DIR/packages; future
        // features read the sibling directories from here. Default: /var/adm
        // (the canonical admin dir; on a stock system it is a symlink to /var/log).
        let adm_dir = conf
            .get("ADM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/adm"));
        // PKG_DB_DIR, when set, overrides the derived location (kept for
        // backward compatibility); otherwise it is ADM_DIR/packages.
        let pkg_db_dir = conf
            .get("PKG_DB_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| adm_dir.join("packages"));

        // RESOLVE_DEPS defaults on; set it to no/false/0 to disable .dep handling.
        let resolve_deps = match conf.get("RESOLVE_DEPS").map(|s| s.to_ascii_lowercase()) {
            Some(v) if v == "no" || v == "false" || v == "0" || v == "off" => false,
            _ => true,
        };

        // IGNORE_TAGS: build tags clean-system should not treat as foreign
        // (e.g. _SBo cf alien _FRG). Space-separated.
        let ignore_tags = conf
            .get("IGNORE_TAGS")
            .map(|v| v.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        // VERIFY: global default verification policy (all | none | gpg,md5,sha).
        let verify = match conf.get("VERIFY") {
            Some(v) => VerifyPolicy::parse(v)
                .map_err(|e| format!("slacker.conf: VERIFY: {e}"))?,
            None => VerifyPolicy::All,
        };

        // MAX_PARALLEL: how many package downloads run concurrently (default 4).
        // 1 disables parallelism (serial). Clamped to 1..=16 so a stray large
        // value can't hammer a mirror; a non-numeric value falls back to 4.
        let max_parallel = parse_max_parallel(conf.get("MAX_PARALLEL").map(String::as_str));

        // REVERT: master switch for the revert-pkg command (default on). Off
        // makes the command refuse outright, before touching anything.
        let revert_enabled = parse_revert_enabled(conf.get("REVERT").map(String::as_str));

        // ARCH is normally auto-detected from the installed system, the way
        // slackpkg does. It is only set in slacker.conf to force a specific
        // architecture (e.g. cross/ARM setups). Resolved here, before the
        // cumulative archive, so revert-pkg's default tree matches the arch.
        let arch = match conf.get("ARCH") {
            Some(a) if !a.is_empty() => a.clone(),
            _ => detect_arch(&pkg_db_dir),
        };

        // CUMULATIVE_URL: where revert-pkg looks for superseded -current
        // packages. Revert-only source; never used by update/upgrade/install.
        let cumulative_url =
            parse_cumulative_url(conf.get("CUMULATIVE_URL").map(String::as_str), &arch);

        // DISTRO_UPGRADE_MIRROR: optional local source for upgrade-dist, kept in
        // its own `distro-upgrade.conf` so the (rare, deliberate) local-mirror /
        // ISO dist setup is separate from everyday config. Absent file or empty
        // value => None => normal remote dist.
        //
        // Read leniently: this file matters ONLY to upgrade-dist (root), so an
        // unreadable one (e.g. mode 600 read by a non-root `status`/`search`)
        // must NOT abort — it is treated as absent. root, which is the only user
        // that can run upgrade-dist, reads it regardless of mode.
        let du_text = std::fs::read_to_string(dir.join("distro-upgrade.conf")).unwrap_or_default();
        let du_conf = parse_keyvals(&du_text);
        let distro_upgrade_mirror =
            parse_distro_upgrade_mirror(du_conf.get("DISTRO_UPGRADE_MIRROR").map(String::as_str));

        // The official mirror URL comes from the slackpkg-style `mirrors`
        // catalogue (uncomment exactly one). A repo line whose URL is the
        // keyword `mirror` is filled in from it, so priority/name/placement of
        // the official repo stay in `repos` while the URL stays in `mirrors`.
        let active_mirror = parse_mirrors(&read_optional(&dir.join("mirrors"))?)?;
        let (repos, tag_priorities) =
            parse_repos(&read_optional(&dir.join("repos"))?, active_mirror.as_deref())?;

        let blacklist = parse_blacklist(&read_optional(&dir.join("blacklist"))?);

        let cfg = Config {
            arch,
            cache_dir,
            state_dir,
            pkg_db_dir,
            adm_dir,
            blacklist,
            repos,
            resolve_deps,
            ignore_tags,
            tag_priorities,
            config_dir: dir.to_path_buf(),
            verify,
            max_parallel,
            revert_enabled,
            cumulative_url,
            distro_upgrade_mirror,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        if self.repos.is_empty() {
            return Err("no repositories configured (add lines to the 'repos' file)".into());
        }
        if self.repos.iter().filter(|r| r.official).count() > 1 {
            return Err("more than one repo tagged 'official' in the 'repos' file".into());
        }
        for (i, r) in self.repos.iter().enumerate() {
            if self.repos[..i].iter().any(|p| p.name == r.name) {
                return Err(format!("duplicate repo name: {}", r.name));
            }
        }
        Ok(())
    }

    pub fn repos_by_priority(&self) -> Vec<&Repo> {
        let mut v: Vec<&Repo> = self.repos.iter().collect();
        v.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.name.cmp(&b.name)));
        v
    }

    /// True if a package's build tag (e.g. `_SBo`, `cf`, `alien`) is in
    /// IGNORE_TAGS, so clean-system should not consider it foreign. The empty
    /// tag (official Slackware packages) never matches.
    pub fn is_ignored_tag(&self, build_tag: &str) -> bool {
        !build_tag.is_empty() && self.ignore_tags.iter().any(|t| t == build_tag)
    }

    /// Does any blacklist rule match a package with this full id
    /// (`name-version-arch-build`), series and source repo? `series`/`repo` may
    /// be None when unknown, in which case series/`@repo` rules don't match.
    pub fn blacklist_hit(&self, id: &str, series: Option<&str>, repo: Option<&str>) -> bool {
        self.blacklist.iter().any(|r| r.matches(id, series, repo))
    }

    /// The repo a package NAME is pinned to (`@repo 100% name`), if any. A pin
    /// forces that repo as the source and bypasses priority, for every command.
    /// Exact-name match — pins are written for one specific package.
    pub fn pinned_repo(&self, name: &str) -> Option<&str> {
        self.blacklist
            .iter()
            .filter(|r| r.pin)
            .find(|r| r.name.as_ref().map_or(false, |re| re.as_str() == name))
            .and_then(|r| r.repo.as_deref())
    }

    /// All active pins as (package name, repo) pairs, for display.
    pub fn pins(&self) -> Vec<(&str, &str)> {
        self.blacklist
            .iter()
            .filter(|r| r.pin)
            .filter_map(|r| Some((r.name.as_ref()?.as_str(), r.repo.as_deref()?)))
            .collect()
    }

    /// Name of the official repository, if one is configured.
    pub fn official_repo_name(&self) -> Option<&str> {
        self.repos.iter().find(|r| r.official).map(|r| r.name.as_str())
    }

    /// Priority of the official Slackware repo, if one is configured. A
    /// distribution subtree (extra/patches/testing/pasture) the user has ranked
    /// ABOVE this is part of the official baseline as far as `install-new` is
    /// concerned: ranking a subtree over the base repo is an explicit "I want
    /// its packages" (patches at 200, or a testing tree bumped over 100), so it
    /// must be scanned like the official tree. A subtree left at or below the
    /// base stays opt-in (`install-new <name>`).
    pub fn official_repo_priority(&self) -> Option<i32> {
        self.repos.iter().find(|r| r.official).map(|r| r.priority)
    }

    pub fn repo_by_name(&self, name: &str) -> Option<&Repo> {
        self.repos.iter().find(|r| r.name == name)
    }
}

impl Repo {
    pub fn cache_subdir(&self, cache_root: &Path) -> PathBuf {
        cache_root.join("repos").join(&self.name)
    }

    pub fn join_url(&self, location: &str) -> String {
        join_base(&self.url, location)
    }

    /// Base URL that package LOCATIONs (and GPG-KEY) resolve against. For a
    /// normal repo that is the repo URL. For a `subtree` repo (Slackware
    /// extra/, patches/, ...) the LOCATIONs are root-relative (`./extra/foo`)
    /// and GPG-KEY lives at the root, so downloads use the PARENT of the URL.
    pub fn download_base(&self) -> &str {
        if self.subtree {
            let trimmed = self.url.trim_end_matches('/');
            match trimmed.rsplit_once('/') {
                // Don't strip the scheme's "//": require a non-empty parent.
                Some((parent, _)) if !parent.ends_with(':') && !parent.is_empty() => parent,
                _ => trimmed,
            }
        } else {
            &self.url
        }
    }

    /// Join a package LOCATION against the download base (see `download_base`).
    /// For non-subtree repos this is identical to `join_url`.
    pub fn join_download_url(&self, location: &str) -> String {
        join_base(self.download_base(), location)
    }
}

/// Join a relative repo location onto a base URL: trim a trailing slash off the
/// base and a leading `./` (or `/`) off the location, then concatenate.
fn join_base(base: &str, location: &str) -> String {
    let base = base.trim_end_matches('/');
    let rel = location.trim_start_matches("./").trim_start_matches('/');
    format!("{base}/{rel}")
}

// ---- plain-text parsing helpers ------------------------------------------

/// Read a file, returning an empty string if it does not exist.
fn read_optional(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Architecture tokens we recognise in Slackware package names / os-release.
const KNOWN_ARCHES: &[&str] = &["x86_64", "aarch64", "i586", "i686", "arm", "noarch"];

/// Resolve the system architecture WITHOUT requiring a fully valid config.
/// `find-mirror` runs precisely when the mirror config is still broken (so the
/// full `load_dir` fails its mirror check), yet it must know the real arch to
/// suggest the correct `slackware{64}` tree. Reads a forced `ARCH` from
/// slacker.conf if present, else detects from the installed base — identical
/// resolution to `load_dir`, just without the validation that would reject it.
pub fn system_arch(dir: &Path) -> String {
    let text = read_optional(&dir.join("slacker.conf")).unwrap_or_default();
    let conf = parse_keyvals(&text);
    if let Some(a) = conf.get("ARCH") {
        if !a.is_empty() {
            return a.clone();
        }
    }
    let adm_dir = conf
        .get("ADM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/adm"));
    let pkg_db_dir = conf
        .get("PKG_DB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| adm_dir.join("packages"));
    detect_arch(&pkg_db_dir)
}

/// Detect the system architecture the way slackpkg does: from the installed
/// base packages, since that reflects the actual install rather than the
/// running kernel. Falls back to /etc/os-release, then `uname -m`.
fn detect_arch(pkg_db_dir: &Path) -> String {
    // 1) The arch of a core installed package is authoritative.
    for core in ["aaa_base", "aaa_glibc-solibs", "aaa_libraries", "glibc-solibs"] {
        if let Some(arch) = installed_pkg_arch(pkg_db_dir, core) {
            return arch;
        }
    }
    // 2) /etc/os-release PRETTY_NAME carries the arch as a word.
    if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                let v = v.trim_matches('"');
                for a in KNOWN_ARCHES {
                    if v.split_whitespace().any(|w| &w == a) {
                        return a.to_string();
                    }
                }
            }
        }
    }
    // 3) Last resort: the running kernel's machine type.
    if let Ok(out) = std::process::Command::new("uname").arg("-m").output() {
        let m = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !m.is_empty() {
            return m;
        }
    }
    // Nothing worked; x86_64 is the most common Slackware target.
    "x86_64".to_string()
}

/// Look up an installed package by name in the DB dir and return its arch.
fn installed_pkg_arch(pkg_db_dir: &Path, name: &str) -> Option<String> {
    let entries = std::fs::read_dir(pkg_db_dir).ok()?;
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_str()?;
        if let Some(id) = crate::pkg::PkgId::parse(fname) {
            if id.name == name {
                return Some(id.arch);
            }
        }
    }
    None
}

/// Strip a trailing `# comment` and surrounding whitespace from a line.
/// Strip a `#` comment and surrounding whitespace from a line, yielding the
/// canonical rule/value text. Shared so callers that need to match a config
/// line exactly (e.g. removing a blacklist rule) canonicalise it identically.
pub fn strip_comment(line: &str) -> &str {
    let line = match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    };
    line.trim()
}

/// Parse KEY=value lines into a map. Quotes around values are stripped.
fn parse_keyvals(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().to_string();
            let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

/// One blacklist entry. Syntax: `[@repo] PATTERN`, where PATTERN is a Slackware
/// series when it ends in `/` (e.g. `kde/`), otherwise an unanchored regular
/// expression matched against the full package id `name-version-arch-build`,
/// like slackpkg. The optional `@repo` prefix scopes the rule to one repo: for
/// an available package that is its candidate repo, for an installed package
/// its source (build tag).
pub struct BlacklistRule {
    repo: Option<String>,
    series: Option<String>,
    name: Option<Regex>,
    /// True for a positive PIN (`@repo 100% pkg`): "only ever take pkg from this
    /// repo, ignoring priority". A pin is the opposite of a freeze, so it must
    /// NOT match in the freeze path (`matches` early-returns false on it); it is
    /// consulted only by `pinned_repo`.
    pin: bool,
}

impl BlacklistRule {
    /// Match a package given its full id, series and source repo. `series`/`repo`
    /// may be None (then series/`@repo` rules simply do not match).
    pub fn matches(&self, id: &str, series: Option<&str>, repo: Option<&str>) -> bool {
        // A pin is a positive rule ("only from this repo"), never a freeze, so it
        // must never make a package look blacklisted. The freeze path stops here.
        if self.pin {
            return false;
        }
        if let Some(want) = &self.repo {
            // `@scope` matches either the candidate's REPO name (`@testing`,
            // `@alienbob`) or its build TAG (`@_SBo`, `@cf`), so a rule can be
            // scoped to a third-party tag as well as a repo. The tag is derived
            // from the full id; a non-parseable id simply doesn't match by tag.
            let repo_hit = repo == Some(want.as_str());
            let tag_hit = crate::pkg::PkgId::parse(id)
                .map_or(false, |p| p.build_tag() == want.as_str());
            if !repo_hit && !tag_hit {
                return false;
            }
        }
        if let Some(s) = &self.series {
            return series == Some(s.as_str());
        }
        match &self.name {
            Some(re) => re.is_match(id),
            None => false,
        }
    }

    /// The `@repo` scope of this rule, if any.
    pub fn repo(&self) -> Option<&str> {
        self.repo.as_deref()
    }

    /// The regex source for a name/regex rule (None for a series rule).
    pub fn pattern(&self) -> Option<&str> {
        self.name.as_ref().map(|re| re.as_str())
    }

    /// A short human description of what this rule freezes.
    pub fn describe(&self) -> String {
        let scope = match &self.repo {
            Some(r) => format!("in repo '{r}' only"),
            None => "in all repos".to_string(),
        };
        match (&self.series, &self.name) {
            (Some(s), _) => format!("series '{s}' {scope}"),
            (None, Some(re)) => format!("matches /{}/ {scope}", re.as_str()),
            (None, None) => format!("(empty rule) {scope}"),
        }
    }
}

/// Parse one blacklist line into a rule, returning a human-readable error on an
/// empty pattern or an invalid regex so callers (e.g. `frozen`) can reject it.
/// Characters that make a name a pattern (regex/glob) rather than an exact
/// package name. Pins are EXACT, so a pin name containing any of these is
/// rejected. `+ - _ .` are allowed because real names use them (e.g. gtk+,
/// python3.11, webkit2gtk6.0) — a bare `.` is a literal dot, never a wildcard.
pub fn name_has_pattern_chars(s: &str) -> bool {
    const PATTERN_CHARS: &str = "*?[](){}|^$\\";
    s.chars().any(|c| PATTERN_CHARS.contains(c))
}

/// Structural regex characters that never occur in a Slackware package name or
/// a glob — their presence means the user wrote a regex. Note `.` is NOT here:
/// real names carry literal dots (webkit2gtk6.0, python3.11, gtk4...), so a bare
/// dot stays literal; the regex "any characters" intent is spelled `.*` / `.+`.
const REGEX_SIGNAL_CHARS: &str = "[](){}|^$\\";

/// Whether PATTERN was written as a regex (taken verbatim) rather than a glob
/// (translated). Regex if it carries a structural regex character OR a `.*`/`.+`
/// "any characters" idiom; otherwise it is a glob, where `.` is a literal dot —
/// so `webkit2gtk6.0*` means "webkit2gtk6.0 followed by anything".
fn looks_like_regex(pat: &str) -> bool {
    pat.chars().any(|c| REGEX_SIGNAL_CHARS.contains(c)) || pat.contains(".*") || pat.contains(".+")
}

/// Compile a blacklist/frozen PATTERN, supporting BOTH styles:
///   * a GLOB (no regex syntax) — `*` matches any run of characters, `?` matches
///     one, every other character (incl. `.` and `+`) is literal, e.g.
///     `pkg_name-*-222*-`, `webkit2gtk6.0*`, `*-l10n-*`;
///   * a REGEX (uses regex syntax: a structural char or the `.*`/`.+` idiom) is
///     taken verbatim, so `xf86-.*-202.*` or `^vlc-[0-9]` keep their meaning.
/// Both compile to the same unanchored regex matched against the full
/// `name-version-arch-build` id, so the rest of the matcher is unchanged.
pub fn compile_pattern(pat: &str) -> Result<Regex, String> {
    let src = if looks_like_regex(pat) {
        pat.to_string()
    } else {
        glob_to_regex(pat)
    };
    Regex::new(&src).map_err(|e| format!("invalid pattern '{pat}': {e}"))
}

/// Translate a glob into an equivalent unanchored regex: `*` -> `.*`, `?` -> `.`,
/// and every other regex-special byte escaped to a literal (so a name char like
/// `+` in `gtk+` stays literal). Only called for patterns with no regex syntax.
fn glob_to_regex(glob: &str) -> String {
    let mut out = String::with_capacity(glob.len() + 4);
    for c in glob.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '+' | '.' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

pub fn parse_blacklist_rule(line: &str) -> Result<BlacklistRule, String> {
    let raw = line.trim().to_string();
    let mut rest = raw.as_str();
    let mut repo = None;
    if let Some(after) = rest.strip_prefix('@') {
        let mut it = after.splitn(2, char::is_whitespace);
        let r = it.next().unwrap_or("").trim();
        let pat = it.next().unwrap_or("").trim();
        if r.is_empty() {
            return Err(format!("'{raw}': missing repo name after '@'"));
        }
        if pat.is_empty() {
            return Err(format!("'{raw}': '@{r}' has no pattern after it"));
        }
        repo = Some(r.to_string());
        rest = pat;
    }
    // Positive PIN marker: `@repo 100% pkg` means "only ever take pkg from this
    // repo, ignoring priority" — the opposite of a freeze. `100%` must be a
    // standalone token (a leading `100%foo` is just an odd pattern, left alone).
    let mut pin = false;
    if let Some(after_marker) = rest.strip_prefix("100%") {
        if after_marker.is_empty() || after_marker.starts_with(char::is_whitespace) {
            if repo.is_none() {
                return Err(format!(
                    "'{raw}': a '100%' pin needs a repo, e.g. '@reponame 100% pkgname'"
                ));
            }
            let name = after_marker.trim();
            if name.is_empty() {
                return Err(format!("'{raw}': '100%' pin needs a package name after it"));
            }
            if name.contains(char::is_whitespace) {
                return Err(format!("'{raw}': a pin takes a single package name, not '{name}'"));
            }
            if name_has_pattern_chars(name) {
                return Err(format!(
                    "'{raw}': a pin uses an EXACT package name, not a pattern ('{name}'); \
                     to freeze a pattern use `frozen` instead"
                ));
            }
            if name.starts_with('-') || name.ends_with('-') {
                return Err(format!(
                    "'{raw}': '{name}' is not a valid package name (no leading/trailing '-')"
                ));
            }
            pin = true;
            rest = name;
        }
    }
    if let Some(series) = rest.strip_suffix('/') {
        let series = series.trim();
        if series.is_empty() {
            return Err(format!("'{raw}': empty series name before '/'"));
        }
        return Ok(BlacklistRule {
            repo,
            series: Some(series.to_string()),
            name: None,
            pin: false,
        });
    }
    let re = compile_pattern(rest).map_err(|e| format!("'{raw}': {e}"))?;
    Ok(BlacklistRule { repo, series: None, name: Some(re), pin })
}

/// Parse the whole `blacklist` file, warning about and skipping any malformed
/// line rather than aborting the load.
fn parse_blacklist(text: &str) -> Vec<BlacklistRule> {
    let mut out = Vec::new();
    for line in parse_lines(text) {
        match parse_blacklist_rule(&line) {
            Ok(rule) => out.push(rule),
            Err(e) => eprintln!("warning: ignoring blacklist entry {e}"),
        }
    }
    out
}

/// Non-empty, non-comment lines (e.g. blacklist entries).
fn parse_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(strip_comment)
        .filter(|l| !l.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Parse the `mirrors` catalogue: exactly one line may be uncommented.
/// Returns the active mirror URL, or None if none is uncommented.
fn parse_mirrors(text: &str) -> Result<Option<String>, String> {
    let active: Vec<String> = text
        .lines()
        .map(strip_comment)
        .filter(|l| !l.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .collect();
    match active.len() {
        0 => Ok(None),
        1 => Ok(Some(active.into_iter().next().unwrap())),
        n => Err(format!(
            "{n} mirrors are uncommented in 'mirrors'; exactly one must be active"
        )),
    }
}

/// Parse the `repos` file: `priority  name  url  [official]` per line.
///
/// A URL of the literal keyword `mirror` is replaced by the active mirror from
/// the `mirrors` file, so the official repo's URL lives there while its
/// priority/name/placement live here. The `mirror/<subpath>` form appends the
/// subpath to the active mirror (e.g. `mirror/extra` -> <active-mirror>/extra),
/// letting a distribution subtree follow the same mirror as the official repo.
/// Fully validate a candidate `repos` file body the way `Config::load_dir`
/// would: it parses the lines (format, priority, verify flags, distinct binary
/// priorities, unique tags, `mirror` resolution from the dir's `mirrors` file)
/// and then applies the cross-checks (at most one `official`, unique repo names,
/// at least one repo). Returns the first problem, or Ok if the file would load.
/// Used by the `add-repo`/`del-repo`/`add-tag`/`del-tag` editors to refuse any
/// change that would leave an unloadable configuration.
pub fn validate_repos_text(config_dir: &Path, repos_text: &str) -> Result<(), String> {
    let active_mirror = parse_mirrors(&read_optional(&config_dir.join("mirrors"))?)?;
    let (repos, _tags) = parse_repos(repos_text, active_mirror.as_deref())?;
    if repos.is_empty() {
        return Err("no repositories would remain (the 'repos' file needs at least one)".into());
    }
    if repos.iter().filter(|r| r.official).count() > 1 {
        return Err("more than one repo tagged 'official'".into());
    }
    for (i, r) in repos.iter().enumerate() {
        if repos[..i].iter().any(|p| p.name == r.name) {
            return Err(format!("duplicate repo name: {}", r.name));
        }
    }
    Ok(())
}

fn parse_repos(
    text: &str,
    active_mirror: Option<&str>,
) -> Result<(Vec<Repo>, Vec<TagPriority>), String> {
    let mut out = Vec::new();
    let mut tags = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(prio), Some(name), Some(third)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(format!(
                "repos:{}: expected 'priority name url [official]', got: {line}",
                lineno + 1
            ));
        };
        let priority: i32 = prio.parse().map_err(|_| {
            format!("repos:{}: priority '{prio}' is not an integer", lineno + 1)
        })?;

        // A line whose third field is a URL, the `mirror` keyword, or a
        // `mirror/<subpath>` form is a binary repo. Otherwise the third field is
        // a build tag, and the line assigns a priority to packages carrying that
        // tag (e.g. `100 SBo _SBo`).
        let is_repo = third == "mirror" || third.starts_with("mirror/") || third.contains("://");
        if !is_repo {
            if let Some(extra) = fields.next() {
                return Err(format!(
                    "repos:{}: tag-priority line takes only 'priority name tag', extra '{extra}'",
                    lineno + 1
                ));
            }
            tags.push(TagPriority {
                name: name.to_string(),
                tag: third.to_string(),
                priority,
            });
            continue;
        }

        // Resolve the `mirror` keyword from the mirrors catalogue. A bare
        // `mirror` yields the active mirror URL; `mirror/<subpath>` appends the
        // subpath to it (e.g. `mirror/extra` -> <active-mirror>/extra), so a
        // distribution subtree follows the same mirror as the official repo.
        let url = if third == "mirror" || third.starts_with("mirror/") {
            let m = active_mirror.ok_or_else(|| {
                format!(
                    "repos:{}: '{name}' uses 'mirror' but no mirror is uncommented in 'mirrors'.\n\
                     Please set up a mirror: uncomment ONE line in your 'mirrors' file \
                     (run `slacker find-mirror` to pick the fastest), then retry.",
                    lineno + 1
                )
            })?;
            match third.strip_prefix("mirror/") {
                Some(sub) => format!("{}/{}", m.trim_end_matches('/'), sub.trim_matches('/')),
                None => m.to_string(),
            }
        } else {
            third.to_string()
        };

        let mut official = false;
        let mut immutable = false;
        let mut subtree = false;
        let mut verify: Option<VerifyPolicy> = None;
        for flag in fields {
            if flag == "official" {
                official = true;
            } else if flag == "immutable" {
                immutable = true;
            } else if flag == "subtree" {
                subtree = true;
            } else if let Some(v) = flag.strip_prefix("verify=") {
                verify = Some(
                    VerifyPolicy::parse(v)
                        .map_err(|e| format!("repos:{}: verify=: {e}", lineno + 1))?,
                );
            } else {
                return Err(format!(
                    "repos:{}: unknown flag '{flag}' (allowed: official, immutable, subtree, verify=...)",
                    lineno + 1
                ));
            }
        }

        out.push(Repo {
            name: name.to_string(),
            url,
            priority,
            official,
            immutable,
            subtree,
            verify,
        });
    }

    // Binary repos must have distinct priorities, otherwise resolution between
    // two repos offering the same package would be ambiguous. (Tag priorities
    // may share a value: they apply to different build tags, not to repos
    // competing for the same package.)
    for i in 0..out.len() {
        for j in (i + 1)..out.len() {
            if out[i].priority == out[j].priority {
                return Err(format!(
                    "repos: '{}' and '{}' share priority {} — give each repo a distinct priority",
                    out[i].name, out[j].name, out[i].priority
                ));
            }
        }
    }
    // The same build tag must not be assigned two different priorities.
    for i in 0..tags.len() {
        for j in (i + 1)..tags.len() {
            if tags[i].tag == tags[j].tag {
                return Err(format!(
                    "repos: tag '{}' is assigned twice ('{}' and '{}') — list each tag once",
                    tags[i].tag, tags[i].name, tags[j].name
                ));
            }
        }
    }
    Ok((out, tags))
}

/// Parse a `MAX_PARALLEL` value into a concurrent-download count. A positive
/// integer is clamped to 1..=16 (1 = serial); anything absent or non-numeric
/// falls back to the default of 4. Kept pure so it can be unit-tested.
fn parse_max_parallel(raw: Option<&str>) -> usize {
    match raw.map(|s| s.trim().parse::<usize>()) {
        Some(Ok(n)) => n.clamp(1, 16),
        _ => 4,
    }
}

/// Parse a `REVERT` on/off value. Default is on; only an explicit
/// off/no/false/0 disables the revert-pkg command. Pure, for unit testing.
fn parse_revert_enabled(raw: Option<&str>) -> bool {
    match raw.map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) if v == "off" || v == "no" || v == "false" || v == "0" => false,
        _ => true,
    }
}

/// Parse a `CUMULATIVE_URL` value: a non-empty URL with any trailing slash
/// trimmed, falling back to the default Slackware-UK cumulative archive for
/// -current. The default's arch segment follows `arch`: `slackware64-current`
/// on x86_64, `slackware-current` on the 32-bit tree. Pure, for unit testing.
fn parse_cumulative_url(raw: Option<&str>, arch: &str) -> String {
    let default = if arch == "x86_64" {
        "https://slackware.uk/cumulative/slackware64-current"
    } else {
        "https://slackware.uk/cumulative/slackware-current"
    };
    raw.map(|s| s.trim().trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .to_string()
}

/// Parse a `DISTRO_UPGRADE_MIRROR` value: an optional local mirror base URL for
/// upgrade-dist, with any trailing slash trimmed. None when absent or empty (the
/// common case — normal remote dist). Pure, for unit testing. The URL is NOT
/// otherwise validated here; upgrade-dist checks reachability and the target
/// release before the point of no return.
fn parse_distro_upgrade_mirror(raw: Option<&str>) -> Option<String> {
    raw.map(|s| s.trim().trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_arch_reads_forced_arch_from_conf() {
        let dir = std::env::temp_dir().join(format!("slacker-arch-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("slacker.conf"), "ARCH=i586\n").unwrap();
        assert_eq!(system_arch(&dir), "i586");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn distro_upgrade_mirror_parse() {
        assert_eq!(parse_distro_upgrade_mirror(None), None);
        assert_eq!(parse_distro_upgrade_mirror(Some("")), None);
        assert_eq!(parse_distro_upgrade_mirror(Some("   ")), None);
        assert_eq!(
            parse_distro_upgrade_mirror(Some("file:///mnt/iso/")),
            Some("file:///mnt/iso".to_string())
        );
        assert_eq!(
            parse_distro_upgrade_mirror(Some("  http://nas/slackware64-current/  ")),
            Some("http://nas/slackware64-current".to_string())
        );
    }
    #[test]
    fn keyvals_and_comments() {
        let m = parse_keyvals("# c\nARCH = x86_64  # inline\nCACHE_DIR=\"/var/cache/slacker\"\n\n");
        assert_eq!(m.get("ARCH").unwrap(), "x86_64");
        assert_eq!(m.get("CACHE_DIR").unwrap(), "/var/cache/slacker");
    }

    #[test]
    fn max_parallel_parse_and_clamp() {
        assert_eq!(parse_max_parallel(None), 4); // absent -> default
        assert_eq!(parse_max_parallel(Some("8")), 8); // normal
        assert_eq!(parse_max_parallel(Some("  6 ")), 6); // trimmed
        assert_eq!(parse_max_parallel(Some("0")), 1); // floor (serial)
        assert_eq!(parse_max_parallel(Some("999")), 16); // ceiling
        assert_eq!(parse_max_parallel(Some("abc")), 4); // garbage -> default
        assert_eq!(parse_max_parallel(Some("")), 4); // empty -> default
    }

    #[test]
    fn revert_toggle_defaults_on() {
        assert!(parse_revert_enabled(None)); // absent -> on
        assert!(parse_revert_enabled(Some("on")));
        assert!(parse_revert_enabled(Some("yes")));
        assert!(parse_revert_enabled(Some("anything"))); // only explicit off disables
        assert!(!parse_revert_enabled(Some("off")));
        assert!(!parse_revert_enabled(Some(" OFF "))); // trimmed + case-insensitive
        assert!(!parse_revert_enabled(Some("no")));
        assert!(!parse_revert_enabled(Some("false")));
        assert!(!parse_revert_enabled(Some("0")));
    }

    #[test]
    fn cumulative_url_default_and_override() {
        assert_eq!(
            parse_cumulative_url(None, "x86_64"),
            "https://slackware.uk/cumulative/slackware64-current"
        );
        // 32-bit default drops the "64": slackware-current.
        assert_eq!(
            parse_cumulative_url(None, "i586"),
            "https://slackware.uk/cumulative/slackware-current"
        );
        assert_eq!(
            parse_cumulative_url(Some(""), "x86_64"),
            "https://slackware.uk/cumulative/slackware64-current"
        ); // empty -> default
        assert_eq!(
            parse_cumulative_url(Some("https://my.mirror/cumulative/slackware64-current/"), "x86_64"),
            "https://my.mirror/cumulative/slackware64-current"
        ); // trailing slash trimmed; explicit value wins regardless of arch
        assert_eq!(
            parse_cumulative_url(Some("  https://x/y  "), "i586"),
            "https://x/y"
        ); // trimmed
    }

    #[test]
    fn repos_columns_and_official() {
        let text = "# priority name url [official]\n\
                    100 slackware https://example/slack  official\n\
                    60 alienbob https://example/ab  # nice\n\
                    50 restricted https://example/r\n";
        let (r, _tags) = parse_repos(text, None).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].name, "slackware");
        assert!(r[0].official);
        assert_eq!(r[1].priority, 60);
        assert_eq!(r[1].name, "alienbob");
        assert!(!r[1].official);
    }

    #[test]
    fn repos_bad_input() {
        assert!(parse_repos("xx name url", None).is_err()); // priority not int
        assert!(parse_repos("60 nameonly", None).is_err()); // missing url
        assert!(parse_repos("60 n url bogus", None).is_err()); // unknown flag
        assert!(parse_repos("60 n url official extra", None).is_err()); // trailing junk
    }

    #[test]
    fn mirrors_one_active() {
        let text = "# catalogue\n#https://a/\nhttps://b/slackware64-current/\n#https://c/\n";
        assert_eq!(parse_mirrors(text).unwrap(), Some("https://b/slackware64-current".to_string()));
    }

    #[test]
    fn mirrors_none_active() {
        assert_eq!(parse_mirrors("#https://a/\n# x\n").unwrap(), None);
    }

    #[test]
    fn mirrors_two_active_is_error() {
        assert!(parse_mirrors("https://a/\nhttps://b/\n").is_err());
    }

    #[test]
    fn repos_mirror_keyword_resolves() {
        let (r, _tags) = parse_repos("100 slackware mirror official\n60 ab https://ab/\n",
                            Some("https://chosen/slackware64-current")).unwrap();
        assert_eq!(r[0].url, "https://chosen/slackware64-current");
        assert!(r[0].official);
        assert_eq!(r[1].url, "https://ab/");
    }

    #[test]
    fn repos_mirror_subpath_resolves_and_keeps_flags() {
        // `mirror/<subpath>` is a repo line (not a tag line) and appends the
        // subpath to the active mirror; trailing flags such as `subtree` parse.
        let (r, _tags) = parse_repos(
            "100 slackware mirror official\n\
             90 extras mirror/extra subtree immutable\n",
            Some("https://m/slackware64-current"),
        )
        .unwrap();
        assert_eq!(r[0].url, "https://m/slackware64-current");
        assert_eq!(r[1].url, "https://m/slackware64-current/extra");
        assert!(r[1].subtree);
        assert!(r[1].immutable);
    }

    #[test]
    fn repos_mirror_subpath_without_active_mirror_errors() {
        assert!(parse_repos("90 extras mirror/extra subtree\n", None).is_err());
    }

    #[test]
    fn verify_policy_parsing() {
        use super::{VerifyPolicy, Check};
        assert_eq!(VerifyPolicy::parse("").unwrap(), VerifyPolicy::All);
        assert_eq!(VerifyPolicy::parse("all").unwrap(), VerifyPolicy::All);
        assert_eq!(VerifyPolicy::parse("none").unwrap(), VerifyPolicy::None);
        assert_eq!(
            VerifyPolicy::parse("gpg,md5,sha").unwrap(),
            VerifyPolicy::Required(vec![Check::Gpg, Check::Md5, Check::Sha])
        );
        // dedup + whitespace + sha256 alias
        assert_eq!(
            VerifyPolicy::parse("gpg, gpg, sha256").unwrap(),
            VerifyPolicy::Required(vec![Check::Gpg, Check::Sha])
        );
        assert!(VerifyPolicy::parse("bogus").is_err());
    }

    #[test]
    fn verify_policy_wants_requires() {
        use super::{VerifyPolicy, Check};
        let all = VerifyPolicy::All;
        assert!(all.wants(Check::Gpg) && all.wants(Check::Sha));
        assert!(!all.requires(Check::Sha)); // best-available: never required
        let req = VerifyPolicy::parse("gpg,md5").unwrap();
        assert!(req.wants(Check::Md5) && req.requires(Check::Md5));
        assert!(!req.wants(Check::Sha) && !req.requires(Check::Sha));
        let none = VerifyPolicy::None;
        assert!(!none.wants(Check::Gpg) && !none.requires(Check::Gpg));
    }

    #[test]
    fn repos_duplicate_priority_is_error() {
        // two binary repos with the same priority must be rejected
        assert!(parse_repos("100 a https://a/\n100 b https://b/\n", None).is_err());
        // distinct priorities are fine
        assert!(parse_repos("100 a https://a/\n90 b https://b/\n", None).is_ok());
        // tag priorities MAY share a value (different tags, not competing repos)
        let (_r, tags) = parse_repos(
            "100 slackware mirror official\n100 SBo _SBo\n100 local _rtz\n",
            Some("https://m/"),
        ).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].tag, "_SBo");
        assert_eq!(tags[1].priority, 100);
    }

    #[test]
    fn repos_immutable_flag_parses() {
        let (repos, _t) = parse_repos(
            "100 slackware mirror official\n80 patches https://p/ immutable\n",
            Some("https://off/"),
        )
        .unwrap();
        let p = repos.iter().find(|r| r.name == "patches").unwrap();
        assert!(p.immutable, "immutable flag must be parsed");
        assert!(!p.official);
        // combines with verify=; an unknown flag is still rejected.
        assert!(parse_repos("80 x https://x/ immutable verify=md5\n", None).is_ok());
        assert!(parse_repos("80 x https://x/ bogus\n", None).is_err());
    }

    #[test]
    fn repos_subtree_flag_parses_and_resolves_parent_base() {
        let (repos, _t) = parse_repos(
            "100 slackware mirror official\n\
             70 extras https://m/slackware64-current/extra subtree immutable\n",
            Some("https://m/slackware64-current/"),
        )
        .unwrap();
        let e = repos.iter().find(|r| r.name == "extras").unwrap();
        assert!(e.subtree, "subtree flag must be parsed");
        assert!(e.immutable, "subtree combines with immutable");

        // Metadata still resolves against the repo URL (the subtree dir)...
        assert_eq!(
            e.join_url("PACKAGES.TXT"),
            "https://m/slackware64-current/extra/PACKAGES.TXT"
        );
        // ...but the download base is the PARENT (distribution root)...
        assert_eq!(e.download_base(), "https://m/slackware64-current");
        // ...so a root-relative LOCATION does NOT double the shared segment.
        assert_eq!(
            e.join_download_url("./extra/sendmail/sendmail-8.18.2-x86_64-1.txz"),
            "https://m/slackware64-current/extra/sendmail/sendmail-8.18.2-x86_64-1.txz"
        );

        // A normal (non-subtree) repo: download base == URL, join unchanged.
        let off = repos.iter().find(|r| r.name == "slackware").unwrap();
        assert!(!off.subtree);
        assert_eq!(off.download_base(), off.url);
        assert_eq!(
            off.join_download_url("./slackware64/l/glibc-2.39-x86_64-1.txz"),
            off.join_url("./slackware64/l/glibc-2.39-x86_64-1.txz")
        );
    }

    #[test]
    fn repos_mirror_keyword_without_active_errors() {
        assert!(parse_repos("100 slackware mirror official\n", None).is_err());
    }

    #[test]
    fn arch_detected_from_aaa_base() {
        let dir = std::env::temp_dir().join("slacker_arch_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // simulate an installed 32-bit -current system
        std::fs::File::create(dir.join("aaa_base-15.1-i686-1")).unwrap();
        std::fs::File::create(dir.join("bash-5.2.32-i686-1")).unwrap();
        assert_eq!(installed_pkg_arch(&dir, "aaa_base"), Some("i686".to_string()));
        assert_eq!(detect_arch(&dir), "i686");
    }

    #[test]
    fn blacklist_lines() {
        let b = parse_lines("# skip these\nkernel-generic\n\nkernel-huge # comment\n");
        assert_eq!(b, vec!["kernel-generic", "kernel-huge"]);
    }

    #[test]
    fn blacklist_rules() {
        let re = parse_blacklist_rule("xlibre.*").unwrap();
        assert!(re.matches("xlibre-server-25.2.0-x86_64-1", Some("x"), Some("slackware")));
        assert!(!re.matches("mesa-25-x86_64-1", Some("x"), Some("slackware")));

        let ver = parse_blacklist_rule("xf86-.*-202.*").unwrap();
        assert!(ver.matches("xf86-video-intel-20260518_931b1d93-x86_64-1", Some("x"), Some("slackware")));

        let series = parse_blacklist_rule("kde/").unwrap();
        assert!(series.matches("plasma-6-x86_64-1", Some("kde"), Some("slackware")));
        assert!(!series.matches("plasma-6-x86_64-1", Some("ap"), Some("slackware")));

        let scoped = parse_blacklist_rule("@alienbob vlc").unwrap();
        assert!(scoped.matches("vlc-3-x86_64-1", None, Some("alienbob")));
        assert!(!scoped.matches("vlc-3-x86_64-1", None, Some("slackware")));

        // `@scope` matches either the candidate's repo OR its build tag, so a
        // rule can be scoped to a third-party tag (`@_SBo`, `@cf`) as well as a
        // repo name. Build tag wins regardless of which repo serves it; an
        // official (tagless) build of the same name is left alone.
        let by_repo = parse_blacklist_rule("@testing xf86-.*").unwrap();
        assert!(by_repo.matches("xf86-input-evdev-20260421-x86_64-1", Some("x"), Some("testing")));
        assert!(!by_repo.matches("xf86-input-evdev-2.11.0-x86_64-1", Some("x"), Some("slackware")));
        let by_tag = parse_blacklist_rule("@_SBo foo").unwrap();
        assert!(by_tag.matches("foo-1.0-x86_64-1_SBo", None, Some("ponce")));
        assert!(!by_tag.matches("foo-1.0-x86_64-1", None, Some("slackware")));

        assert!(parse_blacklist_rule("@alienbob").is_err());
        assert!(parse_blacklist_rule("a[b").is_err());
    }

    #[test]
    fn pin_rule_parses_and_never_freezes() {
        // `@repo 100% pkg` is a positive PIN, not a freeze: matches() (the freeze
        // path) must always be false for it, so it can never look blacklisted.
        let pin = parse_blacklist_rule("@alienbob 100% vlc").unwrap();
        assert!(!pin.matches("vlc-3-x86_64-1", None, Some("alienbob")));
        assert!(!pin.matches("vlc-3-x86_64-1", None, Some("conraid")));

        // A pin needs a repo and a package name.
        assert!(parse_blacklist_rule("100% vlc").is_err()); // no @repo
        assert!(parse_blacklist_rule("@alienbob 100%").is_err()); // no package
        assert!(parse_blacklist_rule("@alienbob 100% vlc mpv").is_err()); // one name only

        // `100%foo` (no space) is NOT the marker — it stays an ordinary (no-op)
        // freeze pattern, so it still parses as a freeze, not a pin.
        let not_a_pin = parse_blacklist_rule("@alienbob 100%foo").unwrap();
        assert!(!not_a_pin.matches("100%foo-1-x86_64-1", None, Some("conraid"))); // repo mismatch

        // A pin name must be EXACT — regex/glob metachars are rejected.
        assert!(parse_blacklist_rule("@alienbob 100% vlc*").is_err());
        assert!(parse_blacklist_rule("@alienbob 100% vlc.*").is_err());
        assert!(name_has_pattern_chars("vlc*"));
        assert!(name_has_pattern_chars("xf86-.*"));
        assert!(!name_has_pattern_chars("gtk+")); // + and - are real name chars
        assert!(!name_has_pattern_chars("vlc-plugin"));
        // A literal dot is a real name character, NOT a pattern: dotted names
        // (python3.11, webkit2gtk6.0, foo.6) are valid EXACT pin names.
        assert!(!name_has_pattern_chars("python3.11"));
        assert!(!name_has_pattern_chars("webkit2gtk6.0"));
        assert!(parse_blacklist_rule("@ponce 100% python3.11").is_ok());
        assert!(parse_blacklist_rule("@ponce 100% foo.6").is_ok());

        // A pin name must be a valid package name: no leading/trailing '-'
        // (a real name never does), but internal dashes are fine.
        assert!(parse_blacklist_rule("@alienbob 100% vlc-").is_err());
        assert!(parse_blacklist_rule("@alienbob 100% -vlc").is_err());
        assert!(parse_blacklist_rule("@alienbob 100% vlc-plugin").is_ok());
    }

    #[test]
    fn patterns_support_both_glob_and_regex() {
        let id = "pkg_name-1.2-x86_64-222build-";
        // GLOB: no regex metacharacters -> `*` is "any run", `?` is "one char".
        let g = parse_blacklist_rule("pkg_name-*-222*-").unwrap();
        assert!(g.matches(id, None, None));
        // a glob with a leading `*` (which is an INVALID bare regex) now works
        let lead = parse_blacklist_rule("*-x86_64-*").unwrap();
        assert!(lead.matches(id, None, None));
        // `gtk+` keeps its literal '+', matching the real package name
        let plus = parse_blacklist_rule("gtk+").unwrap();
        assert!(plus.matches("gtk+-3.24-x86_64-3", None, None));
        assert!(!plus.matches("gtkmm-1-x86_64-1", None, None));

        // Real package names carry LITERAL dots; a glob keeps them literal.
        let wk = parse_blacklist_rule("webkit2gtk6.0*").unwrap();
        assert!(wk.matches("webkit2gtk6.0-2.52.4-x86_64-1_gfs", None, None));
        assert!(!wk.matches("webkit2gtk4.0-1-x86_64-1", None, None)); // 6.0 literal, not "any"
        let py = parse_blacklist_rule("python3.11*").unwrap();
        assert!(py.matches("python3.11-3.11.9-x86_64-1", None, None));

        // REGEX: a pattern that uses regex syntax is taken verbatim, unchanged.
        let r = parse_blacklist_rule("xf86-.*-202.*").unwrap();
        assert!(r.matches("xf86-video-intel-20260518-x86_64-1", None, None));
        let anchored = parse_blacklist_rule("^vlc-[0-9]").unwrap();
        assert!(anchored.matches("vlc-3-x86_64-1", None, None));
        assert!(!anchored.matches("vlc-plugin-3-x86_64-1", None, None)); // anchored: no leading match
    }

    #[test]
    fn pinned_repo_and_pins_read_back() {
        let bl = parse_blacklist("@alienbob 100% vlc\n@conraid 100% mpv\nfirefox\n@alienbob kde/\n");
        let cfg = Config {
            arch: "x86_64".into(),
            cache_dir: std::path::PathBuf::new(),
            state_dir: std::path::PathBuf::new(),
            pkg_db_dir: std::path::PathBuf::new(),
            adm_dir: std::path::PathBuf::new(),
            blacklist: bl,
            repos: vec![],
            resolve_deps: true,
            ignore_tags: vec![],
            tag_priorities: vec![],
            config_dir: std::path::PathBuf::new(),
            verify: VerifyPolicy::All,
            max_parallel: 1,
            revert_enabled: true,
            cumulative_url: String::new(),
            distro_upgrade_mirror: None,
        };
        assert_eq!(cfg.pinned_repo("vlc"), Some("alienbob"));
        assert_eq!(cfg.pinned_repo("mpv"), Some("conraid"));
        assert_eq!(cfg.pinned_repo("firefox"), None); // a freeze, not a pin
        // A freeze rule must NOT leak into pins(); only the two pins do.
        let mut pins = cfg.pins();
        pins.sort();
        assert_eq!(pins, vec![("mpv", "conraid"), ("vlc", "alienbob")]);
        // The freeze still freezes (pins didn't disturb it).
        assert!(cfg.blacklist_hit("firefox-1-x86_64-1", Some("xap"), Some("slackware")));
        // ...and the pinned packages are NOT frozen.
        assert!(!cfg.blacklist_hit("vlc-3-x86_64-1", Some("xap"), Some("alienbob")));
    }

    #[test]
    fn install_new_scans_subtrees_ranked_above_official() {
        // install-new (no arg) scans the official repo PLUS any distribution
        // subtree ranked strictly above it; subtrees at or below the base stay
        // opt-in. This mirrors the two live setups: r-tz (patches high, extras/
        // testing low) and conraid (testing bumped over the base).
        let repos = "\
            100 slackware https://m/slackware64-current/ official\n\
            200 patches https://m/slackware64-current/patches/ immutable subtree\n\
            90 extras https://m/slackware64-current/extra/ immutable subtree\n\
            85 testing https://m/slackware64-current/testing/ immutable subtree\n";
        let (repos, _tags) = parse_repos(repos, None).unwrap();
        let cfg = Config {
            arch: "x86_64".into(),
            cache_dir: std::path::PathBuf::new(),
            state_dir: std::path::PathBuf::new(),
            pkg_db_dir: std::path::PathBuf::new(),
            adm_dir: std::path::PathBuf::new(),
            blacklist: vec![],
            repos,
            resolve_deps: true,
            ignore_tags: vec![],
            tag_priorities: vec![],
            config_dir: std::path::PathBuf::new(),
            verify: VerifyPolicy::All,
            max_parallel: 1,
            revert_enabled: true,
            cumulative_url: String::new(),
            distro_upgrade_mirror: None,
        };
        assert_eq!(cfg.official_repo_priority(), Some(100));
        // The exact predicate install-new applies for the no-arg scan.
        let off = cfg.official_repo_priority();
        let scanned: Vec<&str> = cfg
            .repos
            .iter()
            .filter(|r| r.official || (r.subtree && off.is_some_and(|op| r.priority > op)))
            .map(|r| r.name.as_str())
            .collect();
        // slackware (official) + patches (subtree @200 > 100); extras @90 and
        // testing @85 stay opt-in.
        assert_eq!(scanned, vec!["slackware", "patches"]);

        // conraid's case: testing bumped over the base is scanned.
        let repos2 = "\
            100 slackware https://m/slackware64-current/ official\n\
            105 testing https://m/slackware64-current/testing/ immutable subtree\n";
        let (repos2, _t2) = parse_repos(repos2, None).unwrap();
        let cfg2 = Config { repos: repos2, ..cfg };
        let off2 = cfg2.official_repo_priority();
        let scanned2: Vec<&str> = cfg2
            .repos
            .iter()
            .filter(|r| r.official || (r.subtree && off2.is_some_and(|op| r.priority > op)))
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(scanned2, vec!["slackware", "testing"]);
    }
}
