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

/// Remove a trailing Slackware package extension, if any.
fn strip_pkg_ext(s: &str) -> &str {
    for ext in [".txz", ".tgz", ".tbz", ".tlz", ".tar.gz", ".tar.xz"] {
        if let Some(stripped) = s.strip_suffix(ext) {
            return stripped;
        }
    }
    s
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
}
