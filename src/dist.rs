//! Direction gating for `upgrade-dist`.
//!
//! A distribution upgrade is the one operation that may legitimately ignore
//! every other rule (priority, blacklist), so the *direction* must be locked
//! down hard. This module is the gate: it decides whether a move from the
//! running Slackware release to the target release (the one the official mirror
//! points at) is permitted, and refuses everything that is not **explicitly**
//! whitelisted — a whitelist, never a blacklist, so an input nobody anticipated
//! (a hand-edited `os-release`, a mirror aimed at an older stable, a future
//! release) fails closed.
//!
//! Only pure string/decision logic lives here so the whole matrix is
//! unit-testable without a system; detecting the two endpoints (os-release for
//! the running side, the official repo URL for the target) and driving the
//! upgrade itself live in main.rs.

/// A Slackware release, only as finely as upgrade-dist needs to tell them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Release {
    /// The rolling `-current`.
    Current,
    /// A numbered stable release, e.g. `15.0`, `15.1`, `16.0`.
    Stable(String),
}

/// An allowed upgrade route. Anything not represented here is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// `15.0` → `-current`.
    StableToCurrent,
    /// `15.0` → the next stable N (carries N, e.g. `15.1`).
    StableToStable(String),
}

/// Human label for a release, for messages.
pub fn show(r: &Release) -> String {
    match r {
        Release::Current => "-current".to_string(),
        Release::Stable(v) => v.clone(),
    }
}

/// The single starting point upgrade-dist supports.
const START: &str = "15.0";

/// Decide the route, refusing everything not explicitly allowed (fail-closed).
///
/// Allowed, and only these:
///   * `15.0` → `-current`
///   * `15.0` → a stable strictly newer than `15.0`
///
/// Every other pair — running on -current (nothing newer; must never go back to
/// a stable), starting from a stable other than 15.0, a same-release no-op, or a
/// move to an older/unparseable stable — returns `Err` with a specific reason.
pub fn dist_route(running: &Release, target: &Release) -> Result<Route, String> {
    use Release::*;
    match (running, target) {
        // The only permitted starting point is stable 15.0.
        (Stable(s), _) if s != START => Err(format!(
            "upgrade-dist can only start from Slackware {START}; this system reports {s}"
        )),
        // -current has nothing newer to move to, and must never go backward to a
        // stable. Routine -current updates are a different command.
        (Current, _) => Err(
            "upgrade-dist never runs on -current: there is nothing newer to move to and it \
             must never step back to a stable release. For routine -current updates use \
             `slacker upgrade-all`."
                .into(),
        ),
        // From here on `running` is exactly stable 15.0.
        (Stable(_), Current) => Ok(Route::StableToCurrent),
        (Stable(_), Stable(n)) if version_newer(n, START) => Ok(Route::StableToStable(n.clone())),
        (Stable(_), Stable(n)) if n == START => {
            Err(format!("already on Slackware {START} — nothing to dist-upgrade"))
        }
        (Stable(_), Stable(n)) => Err(format!(
            "refusing to move from {START} to the older or unrecognised stable {n} — \
             upgrade-dist only moves forward"
        )),
    }
}

/// Parse the running release from `/etc/os-release` fields. `VERSION_CODENAME`
/// is authoritative for `-current` (where `VERSION_ID` may read like `15.0+`);
/// otherwise a clean `VERSION_ID` names the stable. Anything else → None, which
/// the caller turns into a refusal rather than a guess.
pub fn parse_release_from_os(version_id: Option<&str>, codename: Option<&str>) -> Option<Release> {
    if codename
        .map(|c| c.trim().eq_ignore_ascii_case("current"))
        .unwrap_or(false)
    {
        return Some(Release::Current);
    }
    let v = version_id?.trim().trim_matches('"').trim_end_matches('+').trim();
    is_version_like(v).then(|| Release::Stable(v.to_string()))
}

/// Parse the target release from the official repo URL by finding its
/// `slackware<arch>-<suffix>` path segment (e.g. `slackware64-current`,
/// `slackware64-15.0`, `slackware-15.0`, `slackwareaarch64-current`). The suffix
/// is `current` or a stable version. None if no such segment is present.
pub fn parse_release_from_url(url: &str) -> Option<Release> {
    for seg in url.split('/') {
        let Some(rest) = seg.strip_prefix("slackware") else {
            continue;
        };
        // rest is like "64-current", "-15.0", "aarch64-current"; the release
        // suffix is whatever follows the FIRST '-'.
        let Some(dash) = rest.find('-') else { continue };
        let suffix = &rest[dash + 1..];
        if suffix.eq_ignore_ascii_case("current") {
            return Some(Release::Current);
        }
        if is_version_like(suffix) {
            return Some(Release::Stable(suffix.to_string()));
        }
    }
    None
}

/// The release suffix used in a `slackware{64}-<suffix>` directory and as the
/// typed-confirmation token: `current` or the stable version (e.g. `15.1`).
pub fn release_suffix(r: &Release) -> String {
    match r {
        Release::Current => "current".to_string(),
        Release::Stable(v) => v.clone(),
    }
}

/// Parse the user's `upgrade-dist` TARGET argument into a [`Release`]: `current`
/// or `-current` → the rolling release; a bare dotted version like `15.1` → that
/// stable. None for anything else, so a garbage target is refused rather than
/// guessed.
pub fn parse_target(arg: &str) -> Option<Release> {
    let t = arg.trim().trim_start_matches('-');
    if t.eq_ignore_ascii_case("current") {
        return Some(Release::Current);
    }
    is_version_like(t).then(|| Release::Stable(t.to_string()))
}

/// True for a bare dotted version like `15.0` or `16` — digits and dots only,
/// at least one digit, no leading/trailing dot.
fn is_version_like(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
        && s.chars().any(|c| c.is_ascii_digit())
        && !s.starts_with('.')
        && !s.ends_with('.')
}

/// `a` newer than `b`, comparing `MAJOR.MINOR` numerically. Unparseable input
/// (anything not `X` or `X.Y`) → false, so a garbage target is never "newer".
fn version_newer(a: &str, b: &str) -> bool {
    match (parse_ver(a), parse_ver(b)) {
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
}

fn parse_ver(s: &str) -> Option<(u32, u32)> {
    let mut it = s.split('.');
    let maj = it.next()?.parse().ok()?;
    let min = match it.next() {
        Some(m) => m.parse().ok()?,
        None => 0,
    };
    if it.next().is_some() {
        return None; // only X or X.Y
    }
    Some((maj, min))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stable(v: &str) -> Release {
        Release::Stable(v.to_string())
    }

    #[test]
    fn os_release_current_wins_over_version_id() {
        // On -current, VERSION_ID often reads "15.0+"; codename decides.
        assert_eq!(
            parse_release_from_os(Some("15.0+"), Some("current")),
            Some(Release::Current)
        );
        assert_eq!(parse_release_from_os(None, Some("current")), Some(Release::Current));
        assert_eq!(parse_release_from_os(None, Some("CURRENT")), Some(Release::Current));
    }

    #[test]
    fn os_release_stable_from_version_id() {
        assert_eq!(parse_release_from_os(Some("15.0"), None), Some(stable("15.0")));
        assert_eq!(parse_release_from_os(Some("\"15.0\""), Some("")), Some(stable("15.0")));
        assert_eq!(parse_release_from_os(Some("15.1"), None), Some(stable("15.1")));
    }

    #[test]
    fn os_release_garbage_is_none() {
        assert_eq!(parse_release_from_os(Some("rolling"), None), None);
        assert_eq!(parse_release_from_os(Some(""), None), None);
        assert_eq!(parse_release_from_os(Some("."), None), None);
        assert_eq!(parse_release_from_os(None, None), None);
    }

    #[test]
    fn url_target_detection() {
        assert_eq!(
            parse_release_from_url("https://mirror/slackware/slackware64-current/"),
            Some(Release::Current)
        );
        assert_eq!(
            parse_release_from_url("https://mirror/slackware-current"),
            Some(Release::Current)
        );
        assert_eq!(
            parse_release_from_url("https://mirror/slackwarearm/slackwareaarch64-current/x"),
            Some(Release::Current)
        );
        assert_eq!(
            parse_release_from_url("https://mirror/slackware/slackware64-15.0/"),
            Some(stable("15.0"))
        );
        assert_eq!(
            parse_release_from_url("https://mirror/slackware64-15.1"),
            Some(stable("15.1"))
        );
        // No slackware*-<suffix> segment present.
        assert_eq!(parse_release_from_url("https://mirror/debian/stable"), None);
        assert_eq!(parse_release_from_url("https://mirror/slackware64/"), None);
    }

    #[test]
    fn allowed_routes() {
        assert_eq!(
            dist_route(&stable("15.0"), &Release::Current),
            Ok(Route::StableToCurrent)
        );
        assert_eq!(
            dist_route(&stable("15.0"), &stable("15.1")),
            Ok(Route::StableToStable("15.1".into()))
        );
        assert_eq!(
            dist_route(&stable("15.0"), &stable("16.0")),
            Ok(Route::StableToStable("16.0".into()))
        );
    }

    #[test]
    fn refuses_backward_and_sideways() {
        // -current never upgrade-dists anywhere.
        assert!(dist_route(&Release::Current, &stable("15.0")).is_err());
        assert!(dist_route(&Release::Current, &stable("15.1")).is_err());
        assert!(dist_route(&Release::Current, &Release::Current).is_err());
        // 15.0 -> older or same stable.
        assert!(dist_route(&stable("15.0"), &stable("15.0")).is_err());
        assert!(dist_route(&stable("15.0"), &stable("14.2")).is_err());
    }

    #[test]
    fn refuses_wrong_starting_point() {
        // Only 15.0 may start a dist-upgrade.
        assert!(dist_route(&stable("14.2"), &Release::Current).is_err());
        assert!(dist_route(&stable("14.2"), &stable("15.0")).is_err());
        // Someone already on a future stable does not get to jump again here.
        assert!(dist_route(&stable("15.1"), &stable("16.0")).is_err());
        assert!(dist_route(&stable("15.1"), &Release::Current).is_err());
    }

    #[test]
    fn refuses_unparseable_target_version() {
        // A target that parses as Stable but is not a real forward version.
        assert!(dist_route(&stable("15.0"), &stable("15.0.1")).is_err());
        assert!(dist_route(&stable("15.0"), &stable("nonsense")).is_err());
    }

    #[test]
    fn version_compare() {
        assert!(version_newer("15.1", "15.0"));
        assert!(version_newer("16.0", "15.0"));
        assert!(version_newer("15.10", "15.9")); // numeric, not lexical
        assert!(!version_newer("15.0", "15.0"));
        assert!(!version_newer("14.2", "15.0"));
        assert!(!version_newer("15.0.1", "15.0")); // not X.Y
        assert!(!version_newer("abc", "15.0"));
    }

    #[test]
    fn target_argument_parses() {
        assert_eq!(parse_target("current"), Some(Release::Current));
        assert_eq!(parse_target("-current"), Some(Release::Current));
        assert_eq!(parse_target(" CURRENT "), Some(Release::Current));
        assert_eq!(parse_target("15.1"), Some(stable("15.1")));
        assert_eq!(parse_target("16"), Some(stable("16")));
        // version-like, so parse_target accepts it; dist_route is what rejects a
        // non-forward / non-X.Y target.
        assert_eq!(parse_target("15.0.1"), Some(stable("15.0.1")));
        assert!(dist_route(&stable("15.0"), &parse_target("15.0.1").unwrap()).is_err());
        assert_eq!(parse_target("foo"), None);
        assert_eq!(parse_target(""), None);
    }
}
