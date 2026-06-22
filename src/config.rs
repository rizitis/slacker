//! Configuration loaded from plain-text files under a config directory
//! (default `/etc/slacker`), in the spirit of slackpkg/slackpkg+:
//!
//!   slacker.conf   KEY=value globals (ARCH, CACHE_DIR, PKG_DB_DIR)
//!   repos          every repo, one per line: `priority  name  url  [official]`
//!   blacklist      one package name per line
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
    /// Directory holding the installed-package database.
    pub pkg_db_dir: PathBuf,
    pub blacklist: Vec<String>,
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
}

#[derive(Debug, Clone)]
pub struct Repo {
    pub name: String,
    pub url: String,
    pub priority: i32,
    pub official: bool,
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
        let pkg_db_dir = conf
            .get("PKG_DB_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/lib/pkgtools/packages"));

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

        // ARCH is normally auto-detected from the installed system, the way
        // slackpkg does. It is only set in slacker.conf to force a specific
        // architecture (e.g. cross/ARM setups).
        let arch = match conf.get("ARCH") {
            Some(a) if !a.is_empty() => a.clone(),
            _ => detect_arch(&pkg_db_dir),
        };
        // The official mirror URL comes from the slackpkg-style `mirrors`
        // catalogue (uncomment exactly one). A repo line whose URL is the
        // keyword `mirror` is filled in from it, so priority/name/placement of
        // the official repo stay in `repos` while the URL stays in `mirrors`.
        let active_mirror = parse_mirrors(&read_optional(&dir.join("mirrors"))?)?;
        let (repos, tag_priorities) =
            parse_repos(&read_optional(&dir.join("repos"))?, active_mirror.as_deref())?;

        let blacklist = parse_lines(&read_optional(&dir.join("blacklist"))?);

        let cfg = Config {
            arch,
            cache_dir,
            pkg_db_dir,
            blacklist,
            repos,
            resolve_deps,
            ignore_tags,
            tag_priorities,
            config_dir: dir.to_path_buf(),
            verify,
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

    pub fn is_blacklisted(&self, name: &str) -> bool {
        self.blacklist.iter().any(|b| b == name)
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
        let base = self.url.trim_end_matches('/');
        let rel = location.trim_start_matches("./").trim_start_matches('/');
        format!("{base}/{rel}")
    }
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
fn strip_comment(line: &str) -> &str {
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
/// priority/name/placement live here.
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

        // A line whose third field is a URL (or the 'mirror' keyword) is a
        // binary repo. Otherwise the third field is a build tag, and the line
        // assigns a priority to packages carrying that tag (e.g. `100 SBo _SBo`).
        let is_repo = third == "mirror" || third.contains("://");
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

        // Resolve the `mirror` keyword from the mirrors catalogue.
        let url = if third == "mirror" {
            match active_mirror {
                Some(m) => m.to_string(),
                None => {
                    return Err(format!(
                        "repos:{}: '{name}' uses 'mirror' but no mirror is uncommented in 'mirrors'",
                        lineno + 1
                    ))
                }
            }
        } else {
            third.to_string()
        };

        let mut official = false;
        let mut verify: Option<VerifyPolicy> = None;
        for flag in fields {
            if flag == "official" {
                official = true;
            } else if let Some(v) = flag.strip_prefix("verify=") {
                verify = Some(
                    VerifyPolicy::parse(v)
                        .map_err(|e| format!("repos:{}: verify=: {e}", lineno + 1))?,
                );
            } else {
                return Err(format!(
                    "repos:{}: unknown flag '{flag}' (allowed: official, verify=...)",
                    lineno + 1
                ));
            }
        }

        out.push(Repo {
            name: name.to_string(),
            url,
            priority,
            official,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyvals_and_comments() {
        let m = parse_keyvals("# c\nARCH = x86_64  # inline\nCACHE_DIR=\"/var/cache/slacker\"\n\n");
        assert_eq!(m.get("ARCH").unwrap(), "x86_64");
        assert_eq!(m.get("CACHE_DIR").unwrap(), "/var/cache/slacker");
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
}
