//! `history`: a chronological log of package changes — what was installed,
//! upgraded and removed, and when — derived entirely from the Slackware
//! pkgtools admin directories under ADM_DIR. Because the source of truth is the
//! pkgtools database itself (not a log slacker keeps), this also captures
//! changes made by installpkg/upgradepkg/removepkg, sbopkg, slackpkg, etc.
//!
//! Sources (under ADM_DIR):
//!   packages/<id>                     currently installed; mtime = install time
//!   removed_packages/<id>             removed by removepkg;  ctime = removal time
//!   removed_packages/<id>-upgraded-T  the OLD version replaced by upgradepkg
//!                                     at local time T (ctime = that instant)
//!
//! A plain `mv` preserves mtime, so a removed record's mtime is still its
//! *install* time; the removal/upgrade instant is the record's ctime. The
//! `-upgraded-T` suffix additionally carries the upgrade time as a local
//! wall-clock string, which we use to calibrate the local UTC offset for
//! display (no timezone crate, DST-correct via nearest sample).

use crate::pkg::PkgId;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// One physical record on disk: a package installed at `installed_at`, possibly
/// later departed (removed or upgraded away).
struct RawRecord {
    pkg: PkgId,
    installed_at: i64,
    departure: Option<Departure>,
}

#[derive(Clone, Copy)]
enum Departure {
    Removed(i64),  // ctime = removal time
    Upgraded(i64), // ctime = upgrade time (== the T in the filename)
}

/// A rendered timeline event.
pub struct Event {
    pub when: i64,
    pub pkg: PkgId,
    pub kind: EventKind,
}

pub enum EventKind {
    /// Installed at `when`. `reinstall` = the same id was present and removed
    /// before this install.
    Installed { reinstall: bool },
    Removed,
    /// This (old) `pkg` was upgraded away. `to` is the replacement version: the
    /// in-place successor, or — when that record was lost to a removed_packages
    /// name collision — the next known version of the same package; None if
    /// nothing later is known.
    Upgraded { to: Option<PkgId> },
}

/// Converts a UTC epoch to a local wall-clock string, self-calibrating the
/// offset from `-upgraded-T` records (each gives a local time and its ctime).
pub struct LocalClock {
    /// (ctime_utc, offset_seconds), sorted by ctime.
    samples: Vec<(i64, i64)>,
}

impl LocalClock {
    fn offset_for(&self, epoch: i64) -> i64 {
        self.samples
            .iter()
            .min_by_key(|(c, _)| (c - epoch).abs())
            .map(|(_, o)| *o)
            .unwrap_or(0)
    }

    /// "YYYY-MM-DD HH:MM" in local time.
    pub fn format(&self, epoch: i64) -> String {
        let local = epoch + self.offset_for(epoch);
        let days = local.div_euclid(86400);
        let secs = local.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        let (h, mi) = (secs / 3600, (secs % 3600) / 60);
        format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
    }

    /// "YYYY-MM-DD" in local time (for date comparisons).
    pub fn local_date(&self, epoch: i64) -> String {
        self.format(epoch)[..10].to_string()
    }
}

/// Result of scanning the admin directories.
pub struct Timeline {
    pub events: Vec<Event>,
    pub clock: LocalClock,
    /// Currently-installed packages with their install time, straight from
    /// `packages/`. Independent of the change-log so `--installed` is always
    /// complete, even for packages whose last action was an upgrade.
    pub current: Vec<(PkgId, i64)>,
}

/// Scan the admin directories and build the timeline, clock and current set.
pub fn collect(adm_dir: &Path) -> Timeline {
    let mut records: Vec<RawRecord> = Vec::new();
    let mut samples: Vec<(i64, i64)> = Vec::new();
    let mut current: Vec<(PkgId, i64)> = Vec::new();

    // Currently installed.
    for_each_entry(&adm_dir.join("packages"), |fname, mtime, _ctime| {
        if let Some(pkg) = PkgId::parse(fname) {
            current.push((pkg.clone(), mtime));
            records.push(RawRecord { pkg, installed_at: mtime, departure: None });
        }
    });

    // Departed (removed or upgraded away).
    for_each_entry(&adm_dir.join("removed_packages"), |fname, mtime, ctime| {
        if let Some((base, when)) = parse_upgraded_suffix(fname) {
            if let Some(pkg) = PkgId::parse(base) {
                records.push(RawRecord {
                    pkg,
                    installed_at: mtime,
                    departure: Some(Departure::Upgraded(ctime)),
                });
                samples.push((ctime, to_naive_epoch(when) - ctime));
            }
        } else if let Some(pkg) = PkgId::parse(fname) {
            records.push(RawRecord {
                pkg,
                installed_at: mtime,
                departure: Some(Departure::Removed(ctime)),
            });
        }
    });

    samples.sort_by_key(|(c, _)| *c);
    Timeline { events: build_events(records), clock: LocalClock { samples }, current }
}

/// Turn raw records into timeline events. Pure (no I/O), unit-testable.
///
/// Each package name's records are its successive *tenures* on the system,
/// ordered by install time. An upgraded-away tenure is always replaced by the
/// very next tenure, so we pair them in that order — robust even when the same
/// version is rebuilt many times (which otherwise confuses time-nearest guesses
/// and leaves orphaned `-> ?` upgrades). The replacement's install is folded
/// into the upgrade event; a plain removal leaves the next tenure as a fresh
/// install. Reinstalls (the same id returning after a departure) are flagged.
fn build_events(records: Vec<RawRecord>) -> Vec<Event> {
    let n = records.len();
    let mut consumed = vec![false; n];

    // Departures keyed by full id, for reinstall detection.
    let departures: Vec<(String, i64)> = records
        .iter()
        .filter_map(|r| match r.departure {
            Some(Departure::Removed(t)) | Some(Departure::Upgraded(t)) => Some((r.pkg.tag(), t)),
            None => None,
        })
        .collect();

    // Record indices grouped by name, ordered by install time (tenure order).
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        by_name.entry(r.pkg.name.as_str()).or_default().push(i);
    }
    for idxs in by_name.values_mut() {
        idxs.sort_by_key(|&i| records[i].installed_at);
    }

    // Pair each upgraded-away tenure with the next tenure of the same name, when
    // it started at the upgrade instant; mark that successor consumed.
    let mut upgrades: Vec<(usize, Option<usize>, i64)> = Vec::new();
    for idxs in by_name.values() {
        for w in 0..idxs.len() {
            let i = idxs[w];
            let Some(Departure::Upgraded(t)) = records[i].departure else { continue };
            let next = idxs.get(w + 1).copied();
            let succ = next.filter(|&j| (records[j].installed_at - t).abs() <= 2 * 86400);
            if let Some(j) = succ {
                consumed[j] = true;
            }
            // Display target: the in-place successor, or — when its own record
            // was lost to a removed_packages name collision — the next known
            // tenure's version (so an upgrade reads "old -> new", not "old -> ?").
            upgrades.push((i, succ.or(next), t));
        }
    }

    let mut events = Vec::new();
    // Installs (skipping those folded into an upgrade).
    for i in 0..n {
        if consumed[i] {
            continue;
        }
        let tag = records[i].pkg.tag();
        let reinstall = departures.iter().any(|(t, time)| *t == tag && *time < records[i].installed_at);
        events.push(Event {
            when: records[i].installed_at,
            pkg: records[i].pkg.clone(),
            kind: EventKind::Installed { reinstall },
        });
    }
    // Removals.
    for r in &records {
        if let Some(Departure::Removed(t)) = r.departure {
            events.push(Event { when: t, pkg: r.pkg.clone(), kind: EventKind::Removed });
        }
    }
    // Upgrades.
    for (oldi, newi, t) in upgrades {
        let to = newi.map(|j| records[j].pkg.clone());
        events.push(Event { when: t, pkg: records[oldi].pkg.clone(), kind: EventKind::Upgraded { to } });
    }

    events.sort_by(|a, b| b.when.cmp(&a.when));
    events
}

/// Split a `name-…-upgraded-YYYY-MM-DD,HH:MM:SS` filename into its base package
/// id and the parsed timestamp. Returns None if there is no such suffix.
fn parse_upgraded_suffix(fname: &str) -> Option<(&str, (i64, i64, i64, i64, i64, i64))> {
    let idx = fname.rfind("-upgraded-")?;
    let base = &fname[..idx];
    let ts = &fname[idx + "-upgraded-".len()..];
    let (date, time) = ts.split_once(',')?;
    let mut d = date.split('-');
    let y = d.next()?.parse().ok()?;
    let mo = d.next()?.parse().ok()?;
    let da = d.next()?.parse().ok()?;
    if d.next().is_some() {
        return None;
    }
    let mut t = time.split(':');
    let h = t.next()?.parse().ok()?;
    let mi = t.next()?.parse().ok()?;
    let s = t.next().unwrap_or("0").parse().ok()?;
    Some((base, (y, mo, da, h, mi, s)))
}

pub(crate) fn to_naive_epoch((y, m, d, h, mi, s): (i64, i64, i64, i64, i64, i64)) -> i64 {
    days_from_civil(y, m, d) * 86400 + h * 3600 + mi * 60 + s
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of days_from_civil: (year, month, day).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Call `f(filename, mtime, ctime)` for each entry in `dir` (silent if absent).
fn for_each_entry(dir: &Path, mut f: impl FnMut(&str, i64, i64)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            f(name, meta.mtime(), meta.ctime());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(s: &str) -> PkgId {
        PkgId::parse(s).unwrap()
    }

    #[test]
    fn civil_roundtrip_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // round-trip a span of dates
        for z in [-1, 1, 1000, 20000, 20610, 30000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn format_is_local_via_calibration() {
        // One sample: at ctime 1000, local wall-clock reads 1000+7200 (UTC+2).
        let clock = LocalClock { samples: vec![(1000, 7200)] };
        // 1970-01-01 00:00 UTC + 2h = 02:00
        assert_eq!(clock.format(0), "1970-01-01 02:00");
    }

    #[test]
    fn parse_upgraded_suffix_works() {
        let (base, ts) = parse_upgraded_suffix("Cython-3.2.1-x86_64-1-upgraded-2026-03-07,11:52:19").unwrap();
        assert_eq!(base, "Cython-3.2.1-x86_64-1");
        assert_eq!(ts, (2026, 3, 7, 11, 52, 19));
        assert!(parse_upgraded_suffix("DownTube-15.0-x86_64-2_SBo").is_none());
    }

    #[test]
    fn downtube_full_life_is_reconstructed() {
        // The real sample: install -> remove -> reinstall -> upgrade -> remove.
        let t = |hms: &str| to_naive_epoch_str(hms);
        let records = vec![
            RawRecord { pkg: pid("DownTube-15.0-x86_64-1_SBo"), installed_at: t("2026-03-16,23:18:00"),
                departure: Some(Departure::Removed(t("2026-03-16,23:20:00"))) },
            RawRecord { pkg: pid("DownTube-15.0-x86_64-1_SBo"), installed_at: t("2026-03-16,23:23:00"),
                departure: Some(Departure::Upgraded(t("2026-06-07,23:01:29"))) },
            RawRecord { pkg: pid("DownTube-15.0-x86_64-2_SBo"), installed_at: t("2026-06-07,23:01:29"),
                departure: Some(Departure::Removed(t("2026-06-07,23:05:00"))) },
        ];
        let ev = build_events(records);
        // newest first
        let kinds: Vec<&str> = ev.iter().map(describe).collect();
        assert_eq!(
            kinds,
            vec![
                "removed DownTube-15.0-x86_64-2_SBo",
                "upgraded DownTube-15.0-x86_64-1_SBo -> DownTube-15.0-x86_64-2_SBo",
                "installed DownTube-15.0-x86_64-1_SBo reinstall",
                "removed DownTube-15.0-x86_64-1_SBo",
                "installed DownTube-15.0-x86_64-1_SBo",
            ]
        );
    }

    #[test]
    fn rebuilt_same_version_chain_pairs_correctly() {
        // emacs: install 30.2-3 -> upgrade to 30.2-4 -> rebuilt 30.2-4 twice
        // (upgradepkg over the same id) -> still installed. The 30.2-3 upgrade
        // must resolve to 30.2-4 (not "?"), and the same-version steps pair
        // 30.2-4 -> 30.2-4 (rendered as reinstalls), with no orphans.
        let records = vec![
            RawRecord { pkg: pid("emacs-30.2-x86_64-3"), installed_at: 1000,
                departure: Some(Departure::Upgraded(2000)) },
            RawRecord { pkg: pid("emacs-30.2-x86_64-4"), installed_at: 2000,
                departure: Some(Departure::Upgraded(3000)) },
            RawRecord { pkg: pid("emacs-30.2-x86_64-4"), installed_at: 3000,
                departure: Some(Departure::Upgraded(4000)) },
            RawRecord { pkg: pid("emacs-30.2-x86_64-4"), installed_at: 4000, departure: None },
        ];
        let ev = build_events(records);
        let lines: Vec<&str> = ev.iter().map(describe).collect();
        assert_eq!(
            lines,
            vec![
                "upgraded emacs-30.2-x86_64-4 -> emacs-30.2-x86_64-4", // rebuild (4000)
                "upgraded emacs-30.2-x86_64-4 -> emacs-30.2-x86_64-4", // rebuild (3000)
                "upgraded emacs-30.2-x86_64-3 -> emacs-30.2-x86_64-4", // real upgrade (2000)
                "installed emacs-30.2-x86_64-3",                        // first install (1000)
            ]
        );
        // No orphaned "-> ?" and no standalone install of a consumed successor.
        assert!(!lines.iter().any(|l| l.contains("-> ?")));
    }

    #[test]
    fn lost_successor_infers_target_version() {
        // emacs 30.2-3 upgraded; its 30.2-4 successor record was lost to a
        // removed_packages name collision (only a much-later 30.2-4 survives).
        // The upgrade must read "30.2-3 -> 30.2-4", never "-> ?".
        let records = vec![
            RawRecord { pkg: pid("emacs-30.2-x86_64-3"), installed_at: 1000,
                departure: Some(Departure::Upgraded(2000)) },
            RawRecord { pkg: pid("emacs-30.2-x86_64-4"), installed_at: 9_000_000,
                departure: Some(Departure::Removed(9_000_100)) },
        ];
        let ev = build_events(records);
        let lines: Vec<&str> = ev.iter().map(describe).collect();
        assert!(lines.iter().any(|l| *l == "upgraded emacs-30.2-x86_64-3 -> emacs-30.2-x86_64-4"));
        assert!(!lines.iter().any(|l| l.contains("-> ?")));
        // the later 30.2-4 is a real separate tenure, still shown (not consumed)
        assert!(lines.iter().any(|l| *l == "installed emacs-30.2-x86_64-4"));
    }

    // helpers for the test above
    fn to_naive_epoch_str(s: &str) -> i64 {
        let (d, t) = s.split_once(',').unwrap();
        let mut di = d.split('-');
        let mut ti = t.split(':');
        to_naive_epoch((
            di.next().unwrap().parse().unwrap(),
            di.next().unwrap().parse().unwrap(),
            di.next().unwrap().parse().unwrap(),
            ti.next().unwrap().parse().unwrap(),
            ti.next().unwrap().parse().unwrap(),
            ti.next().unwrap().parse().unwrap(),
        ))
    }
    fn describe(e: &Event) -> &'static str {
        // leak small strings to return &'static for easy comparison
        let s = match &e.kind {
            EventKind::Installed { reinstall, .. } => {
                format!("installed {}{}", e.pkg, if *reinstall { " reinstall" } else { "" })
            }
            EventKind::Removed => format!("removed {}", e.pkg),
            EventKind::Upgraded { to } => format!(
                "upgraded {} -> {}",
                e.pkg,
                to.as_ref().map(|p| p.to_string()).unwrap_or_else(|| "?".into())
            ),
        };
        Box::leak(s.into_boxed_str())
    }
}
