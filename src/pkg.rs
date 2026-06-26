//! Package identity and Slackware package-name parsing.
//!
//! Slackware package filenames follow `name-version-arch-build.ext`.
//! The tricky part is that `name` itself may contain dashes, so we split
//! from the right: the last three dash-separated fields are always
//! build, arch and version, and everything before them is the name.
//! This mirrors pkgtools' `split_package_name`.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PkgId {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub build: String,
}

impl PkgId {
    /// Parse a package identifier from either a bare tag
    /// (`xfce4-panel-4.18.6-x86_64-1`) or a full filename
    /// (`xfce4-panel-4.18.6-x86_64-1.txz`).
    pub fn parse(raw: &str) -> Option<PkgId> {
        // Strip a known package extension if present.
        let stem = strip_pkg_ext(raw);

        // Need at least name + version + arch + build => 3 dashes.
        let mut parts: Vec<&str> = stem.rsplitn(4, '-').collect();
        if parts.len() != 4 {
            return None;
        }
        // rsplitn yields right-to-left: [build, arch, version, name]
        let build = parts.remove(0).to_string();
        let arch = parts.remove(0).to_string();
        let version = parts.remove(0).to_string();
        let name = parts.remove(0).to_string();

        if name.is_empty() || version.is_empty() || arch.is_empty() || build.is_empty() {
            return None;
        }
        Some(PkgId { name, version, arch, build })
    }

    /// The canonical tag without extension: `name-version-arch-build`.
    pub fn tag(&self) -> String {
        format!("{}-{}-{}-{}", self.name, self.version, self.arch, self.build)
    }

    /// The repository "build tag": the build field with its leading build
    /// number stripped. Works for both styles, e.g. `1_SBo` -> `_SBo`,
    /// `7cf` -> `cf`, `1alien` -> `alien`. Official packages (`1`) give "".
    pub fn build_tag(&self) -> &str {
        self.build.trim_start_matches(|c: char| c.is_ascii_digit())
    }

    /// True if this package counts as an OFFICIAL Slackware package for the
    /// purpose of source attribution and clean-system: either a tagless build
    /// (the -current convention) OR a stable patched build carrying a
    /// `_slack<version>` tag (the official stable convention — e.g.
    /// `glibc-2.33-x86_64-9_slack15.0`). Stable patched packages are the MOST
    /// official packages on a stable system, so they must never be treated as
    /// third-party / foreign just because they carry a tag.
    pub fn is_official_build(&self) -> bool {
        is_official_tag(self.build_tag())
    }

    /// True when `other` is a different version/build of the *same* name.
    pub fn is_other_revision_of(&self, other: &PkgId) -> bool {
        self.name == other.name
            && (self.version != other.version || self.build != other.build)
    }
}

impl fmt::Display for PkgId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.tag())
    }
}

/// Remove a trailing Slackware package extension, if any. Public so a caller can
/// turn a package filename (`foo-1.0-x86_64-1.txz`) into the name of its installed
/// package-database record (`foo-1.0-x86_64-1`).
pub fn strip_pkg_ext(s: &str) -> &str {
    for ext in [".txz", ".tgz", ".tbz", ".tlz", ".tar.gz", ".tar.xz"] {
        if let Some(stripped) = s.strip_suffix(ext) {
            return stripped;
        }
    }
    s
}

/// True if `name` is a safe, self-contained package filename: a single path
/// component with no directory separators, no `.`/`..`, no leading `-`, and no
/// control characters. Repo-supplied filenames are used to build cache paths
/// and download URLs, so anything path-like (`../../etc/x`, `/etc/x`, `a/b`)
/// MUST be rejected — otherwise a malicious or MITM'd repo could make slacker,
/// running as root, write attacker bytes outside the cache (e.g. into
/// /etc/cron.d), which is arbitrary code execution. This is the choke point
/// for that class of attack and is enforced both where repo metadata is parsed
/// and again where the on-disk path is built (defence in depth).
pub fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if name.starts_with('-') {
        return false; // could be mistaken for a flag by a downstream tool
    }
    if name.chars().any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control()) {
        return false;
    }
    // Must be exactly its own basename — no embedded separators of any kind.
    std::path::Path::new(name).file_name().map(|f| f == name).unwrap_or(false)
}

/// True if a PACKAGE LOCATION (the in-repo directory, used only to build the
/// download URL) is free of `..` traversal segments. Absolute or `..`-bearing
/// locations are rejected so the fetch URL can't be steered off the repo.
pub fn is_safe_location(loc: &str) -> bool {
    !loc.split(['/', '\\']).any(|seg| seg == "..")
}

/// True if `tag` (a stripped build tag from [`PkgId::build_tag`]) denotes an
/// OFFICIAL Slackware source: the empty tag (tagless -current packages) or a
/// stable patch tag `_slack<version>` (e.g. `_slack15.0`, `_slack15.1`).
pub fn is_official_tag(tag: &str) -> bool {
    tag.is_empty() || is_slack_stable_tag(tag)
}

/// True if `tag` is exactly `_slack<version>` where `<version>` is a dotted
/// number like `15.0` or `15` — the tag the official Slackware *stable* tree
/// stamps onto its patched packages. Anything else (`_SBo`, `alien`, `cf`, a
/// bare `_slack`, `_slack_x`) is not a stable patch tag.
pub fn is_slack_stable_tag(tag: &str) -> bool {
    match tag.strip_prefix("_slack") {
        Some(v) => {
            !v.is_empty()
                && v.chars().all(|c| c.is_ascii_digit() || c == '.')
                && v.chars().any(|c| c.is_ascii_digit())
                && !v.starts_with('.')
                && !v.ends_with('.')
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tag_strips_number_both_styles() {
        let p = PkgId::parse("BeautifulSoup4-4.14.3-x86_64-1_SBo").unwrap();
        assert_eq!(p.build_tag(), "_SBo");
        let p = PkgId::parse("aircrack-ng-1.7-x86_64-7cf").unwrap();
        assert_eq!(p.build_tag(), "cf");
        let p = PkgId::parse("vlc-3.0-x86_64-1alien").unwrap();
        assert_eq!(p.build_tag(), "alien");
        let p = PkgId::parse("bash-5.3.0-x86_64-1").unwrap();
        assert_eq!(p.build_tag(), ""); // official, no tag
    }

    #[test]
    fn simple_name() {
        let p = PkgId::parse("bash-5.2.21-x86_64-3.txz").unwrap();
        assert_eq!(p.name, "bash");
        assert_eq!(p.version, "5.2.21");
        assert_eq!(p.arch, "x86_64");
        assert_eq!(p.build, "3");
    }

    #[test]
    fn dashed_name() {
        // name with internal dashes must be preserved
        let p = PkgId::parse("xfce4-panel-4.18.6-x86_64-1.txz").unwrap();
        assert_eq!(p.name, "xfce4-panel");
        assert_eq!(p.version, "4.18.6");
        assert_eq!(p.build, "1");
    }

    #[test]
    fn bare_tag_without_ext() {
        let p = PkgId::parse("aaa_base-15.1-x86_64-3").unwrap();
        assert_eq!(p.name, "aaa_base");
        assert_eq!(p.arch, "x86_64");
    }

    #[test]
    fn rejects_garbage() {
        assert!(PkgId::parse("notapackage").is_none());
        assert!(PkgId::parse("a-b").is_none());
    }

    #[test]
    fn revision_detection() {
        let a = PkgId::parse("bash-5.2.21-x86_64-3").unwrap();
        let b = PkgId::parse("bash-5.2.26-x86_64-1").unwrap();
        let c = PkgId::parse("bash-5.2.21-x86_64-3").unwrap();
        assert!(a.is_other_revision_of(&b));
        assert!(!a.is_other_revision_of(&c));
    }

    #[test]
    fn safe_filename_accepts_normal_packages() {
        assert!(is_safe_filename("bash-5.2.21-x86_64-1.txz"));
        assert!(is_safe_filename("xfce4-panel-4.18.6-x86_64-1_SBo.txz"));
    }

    #[test]
    fn safe_filename_rejects_traversal_and_tricks() {
        assert!(!is_safe_filename("../../../../etc/cron.d/x-1.0-x86_64-1.txz"));
        assert!(!is_safe_filename("/etc/cron.d/x-1.0-x86_64-1.txz"));
        assert!(!is_safe_filename("foo/bar-1.0-x86_64-1.txz"));
        assert!(!is_safe_filename("a\\b-1.0-x86_64-1.txz"));
        assert!(!is_safe_filename(".."));
        assert!(!is_safe_filename("."));
        assert!(!is_safe_filename(""));
        assert!(!is_safe_filename("-rf")); // leading dash
        assert!(!is_safe_filename("evil\u{0000}-1.0-x86_64-1.txz")); // NUL
        assert!(!is_safe_filename("evil\n-1.0-x86_64-1.txz")); // control
    }

    #[test]
    fn safe_location_rejects_dotdot() {
        assert!(is_safe_location("./slackware64/l"));
        assert!(is_safe_location("x86_64/multimedia"));
        assert!(!is_safe_location("../../../etc"));
        assert!(!is_safe_location("a/../../b"));
    }

    #[test]
    fn slack_stable_tag_is_recognised_as_official() {
        // The official stable patch tag, any version.
        assert!(is_slack_stable_tag("_slack15.0"));
        assert!(is_slack_stable_tag("_slack15.1"));
        assert!(is_slack_stable_tag("_slack16"));
        // tagless and stable-patch both count as official.
        assert!(is_official_tag(""));
        assert!(is_official_tag("_slack15.0"));
        // genuine third-party tags are NOT official.
        assert!(!is_slack_stable_tag("_SBo"));
        assert!(!is_slack_stable_tag("alien"));
        assert!(!is_slack_stable_tag("cf"));
        assert!(!is_slack_stable_tag("_slack"));     // no version
        assert!(!is_slack_stable_tag("_slack15.0a")); // trailing junk
        assert!(!is_official_tag("_SBo"));
        assert!(!is_official_tag("alien"));
        // a real package: glibc patched on 15.0 is official.
        let p = PkgId::parse("glibc-2.33-x86_64-9_slack15.0").unwrap();
        assert_eq!(p.build_tag(), "_slack15.0");
        assert!(p.is_official_build());
        // an alien package is not.
        let a = PkgId::parse("vlc-3.0.23-x86_64-1alien").unwrap();
        assert!(!a.is_official_build());
    }
}
