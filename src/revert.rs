//! Rollback support for `revert-pkg`: pure helpers for reading the previous
//! versions of a package out of Slackware's removed-packages records, and (in a
//! following step) for locating those packages in the cumulative -current
//! archive.
//!
//! Only pure string logic lives here so it can be unit-tested without a
//! filesystem or network; the command that drives it (the -current guard,
//! reading the records directory, network fetch, GPG verification and
//! upgradepkg) lives in main.rs.

use crate::pkg::PkgId;
use std::collections::HashMap;

/// Strip the `-upgraded-<timestamp>` suffix that `upgradepkg` appends to a
/// removed-package record, e.g.
/// `vlc-3.0.21-x86_64-2alien-upgraded-2026-06-25,17:43:40` -> `vlc-3.0.21-x86_64-2alien`.
/// Records left by a plain `removepkg` carry no such suffix and are returned
/// unchanged. The timestamp always begins with a digit, which guards against a
/// (pathological) package name that itself contains the literal `-upgraded-`.
pub fn strip_upgraded_suffix(entry: &str) -> &str {
    match entry.rsplit_once("-upgraded-") {
        Some((prefix, suffix)) if suffix.starts_with(|c: char| c.is_ascii_digit()) => prefix,
        _ => entry,
    }
}

/// From a list of removed-package record names (ideally newest-first) and a
/// target package name, return the distinct OFFICIAL previous versions of that
/// name, preserving input order and capped at `limit`.
///
/// "Official" means the build tag carries no third-party suffix (alien, cf,
/// _SBo, _FRG, ...) — i.e. `build_tag()` is empty — because the cumulative
/// archive only holds official Slackware packages. Non-official records are
/// skipped here so the user is never offered a version that cannot be fetched.
/// De-duplication is by full id, keeping the first (newest, given the input
/// order) occurrence.
pub fn previous_official_versions(entries: &[&str], name: &str, limit: usize) -> Vec<PkgId> {
    let mut out: Vec<PkgId> = Vec::new();
    for raw in entries {
        let clean = strip_upgraded_suffix(raw);
        let id = match PkgId::parse(clean) {
            Some(id) => id,
            None => continue,
        };
        if id.name != name {
            continue;
        }
        if !id.build_tag().is_empty() {
            continue; // third-party — not present in the cumulative archive
        }
        if out.iter().any(|e| e.tag() == id.tag()) {
            continue; // dedup by full id, keeping the first (newest) seen
        }
        out.push(id);
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Parse a cumulative-archive `PACKAGES.TXT` into a map of package *base name*
/// -> its `PACKAGE LOCATION:` (e.g. `vlc` -> `./slackware64/xap`). PACKAGES.TXT
/// lists only the current entry per name, but the series location is stable
/// across versions, so the current entry's location is correct for an older
/// build of the same package too — which is exactly what we need (we want the
/// series, not the version). The first location seen for a name wins.
pub fn parse_locations(packages_txt: &str) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    let mut pending_name: Option<String> = None;
    for line in packages_txt.lines() {
        if let Some(rest) = line.strip_prefix("PACKAGE NAME:") {
            // The name field is a full filename; take its base package name.
            pending_name = PkgId::parse(rest.trim()).map(|id| id.name);
        } else if let Some(rest) = line.strip_prefix("PACKAGE LOCATION:") {
            if let Some(name) = pending_name.take() {
                map.entry(name).or_insert_with(|| rest.trim().to_string());
            }
        }
    }
    map
}

/// Build the cumulative-archive download URL (the `.txz`) for a specific full
/// package id, using the base-name -> location map and the archive base URL.
///
/// Returns None when the package's series is unknown — i.e. the name is not in
/// the archive's PACKAGES.TXT. That happens for a package that has left -current
/// entirely, or one that lives only under `extra/` or `patches/` (the archive's
/// root PACKAGES.TXT covers the main `slackware64/` tree). The caller turns that
/// into an honest "couldn't locate this package in the archive" message rather
/// than guessing a path. The `.asc` URL is this URL with `.asc` appended.
pub fn cumulative_url_for(
    base_url: &str,
    locations: &HashMap<String, String>,
    id: &PkgId,
) -> Option<String> {
    let loc = locations.get(&id.name)?;
    let rel = loc.trim_start_matches("./").trim_matches('/');
    Some(format!(
        "{}/{}/{}.txz",
        base_url.trim_end_matches('/'),
        rel,
        id.tag()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_handles_upgraded_and_plain() {
        assert_eq!(
            strip_upgraded_suffix("flatpak-1.18.0-x86_64-1alien-upgraded-2026-06-25,17:43:40"),
            "flatpak-1.18.0-x86_64-1alien"
        );
        assert_eq!(
            strip_upgraded_suffix("emacs-30.2-x86_64-4"),
            "emacs-30.2-x86_64-4"
        ); // plain removepkg record, unchanged
        assert_eq!(
            strip_upgraded_suffix("slacker-0.7.1-x86_64-1_FRG-upgraded-2026-06-25,19:38:11"),
            "slacker-0.7.1-x86_64-1_FRG"
        );
    }

    #[test]
    fn filters_official_only_and_dedups_newest_first() {
        // Mixed real-world records, newest-first. Only official 'vlc' survives.
        let entries = [
            "vlc-3.0.21-x86_64-2-upgraded-2026-06-20,10:00:00", // official, newest
            "vlc-3.0.21-x86_64-2-upgraded-2026-06-01,09:00:00", // same id again -> dedup
            "vlc-3.0.20-x86_64-1-upgraded-2026-05-10,08:00:00", // official, older
            "vlc-3.0.99-x86_64-1alien",                         // third-party -> skip
            "flatpak-1.18.0-x86_64-2cf",                        // different name -> skip
        ];
        let got = previous_official_versions(&entries, "vlc", 10);
        let tags: Vec<String> = got.iter().map(|p| p.tag()).collect();
        assert_eq!(tags, vec!["vlc-3.0.21-x86_64-2", "vlc-3.0.20-x86_64-1"]);
    }

    #[test]
    fn respects_limit_in_input_order() {
        let entries = [
            "foo-1-x86_64-1",
            "foo-2-x86_64-1",
            "foo-3-x86_64-1",
            "foo-4-x86_64-1",
        ];
        let got = previous_official_versions(&entries, "foo", 2);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].version, "1");
        assert_eq!(got[1].version, "2");
    }

    #[test]
    fn name_match_is_exact() {
        // 'flatpak' must not match 'flatpak-xdg-utils'
        let entries = ["flatpak-xdg-utils-1.0.6-x86_64-2", "flatpak-1.18.0-x86_64-2"];
        let got = previous_official_versions(&entries, "flatpak", 10);
        let tags: Vec<String> = got.iter().map(|p| p.tag()).collect();
        assert_eq!(tags, vec!["flatpak-1.18.0-x86_64-2"]);
    }

    const PACKAGES_FIXTURE: &str = "\
PACKAGE NAME:  vlc-3.0.21-x86_64-2.txz
PACKAGE LOCATION:  ./slackware64/xap
PACKAGE SIZE (compressed):  10000 K
PACKAGE DESCRIPTION:
vlc: vlc (media player)

PACKAGE NAME:  emacs-30.2-x86_64-4.txz
PACKAGE LOCATION:  ./slackware64/e
PACKAGE DESCRIPTION:
emacs: emacs (GNU Emacs)

PACKAGE NAME:  plasma-activities-6.7.1-x86_64-1.txz
PACKAGE LOCATION:  ./slackware64/kde
PACKAGE DESCRIPTION:
plasma-activities: ...
";

    #[test]
    fn parses_name_to_location() {
        let m = parse_locations(PACKAGES_FIXTURE);
        assert_eq!(m.get("vlc").map(String::as_str), Some("./slackware64/xap"));
        assert_eq!(m.get("emacs").map(String::as_str), Some("./slackware64/e"));
        assert_eq!(
            m.get("plasma-activities").map(String::as_str),
            Some("./slackware64/kde")
        );
        assert_eq!(m.get("nonexistent"), None);
    }

    #[test]
    fn builds_url_for_older_version_via_stable_location() {
        let m = parse_locations(PACKAGES_FIXTURE);
        // Want an OLDER vlc: the location comes from the (current) entry but is
        // stable, so the URL still points at the right series for the old build.
        let old = PkgId::parse("vlc-3.0.20-x86_64-1").unwrap();
        let url =
            cumulative_url_for("https://slackware.uk/cumulative/slackware64-current", &m, &old);
        assert_eq!(
            url.as_deref(),
            Some("https://slackware.uk/cumulative/slackware64-current/slackware64/xap/vlc-3.0.20-x86_64-1.txz")
        );
    }

    #[test]
    fn unknown_package_yields_no_url() {
        let m = parse_locations(PACKAGES_FIXTURE);
        let id = PkgId::parse("ghost-1.0-x86_64-1").unwrap();
        assert_eq!(cumulative_url_for("https://x/y", &m, &id), None);
    }

    #[test]
    fn trailing_slash_in_base_url_is_handled() {
        let m = parse_locations(PACKAGES_FIXTURE);
        let id = PkgId::parse("emacs-30.2-x86_64-4").unwrap();
        let url = cumulative_url_for("https://x/y/", &m, &id);
        assert_eq!(
            url.as_deref(),
            Some("https://x/y/slackware64/e/emacs-30.2-x86_64-4.txz")
        );
    }

    #[test]
    fn builds_32bit_url_from_slackware_tree() {
        // On the 32-bit archive the base is `slackware-current` and the cumulative
        // PACKAGES.TXT gives `./slackware/...` LOCATIONs; the URL must follow the
        // location verbatim, so the path auto-adapts to the 32-bit tree.
        let mut m = HashMap::new();
        m.insert("vlc".to_string(), "./slackware/xap".to_string());
        let id = PkgId::parse("vlc-3.0.20-i586-1").unwrap();
        let url = cumulative_url_for("https://slackware.uk/cumulative/slackware-current", &m, &id);
        assert_eq!(
            url.as_deref(),
            Some("https://slackware.uk/cumulative/slackware-current/slackware/xap/vlc-3.0.20-i586-1.txz")
        );
    }
}
