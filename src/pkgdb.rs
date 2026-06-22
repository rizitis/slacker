//! The unified package database with priority resolution, plus the matching
//! logic slackpkg exposes: substring patterns, series names, install-new
//! diffing, and clean-system orphan detection.

use crate::config::Config;
use crate::pkg::PkgId;
use crate::repo::{self, AvailPkg};
use std::collections::{HashMap, HashSet};

pub struct PkgDb {
    all: Vec<AvailPkg>,
    priority: HashMap<String, i32>,
    official_priority: Option<i32>,
}

impl PkgDb {
    pub fn load(cfg: &Config) -> Result<PkgDb, String> {
        let mut all = Vec::new();
        let mut priority = HashMap::new();
        for r in &cfg.repos {
            priority.insert(r.name.clone(), r.priority);
            all.extend(repo::load_repo(r, &cfg.cache_dir, &cfg.arch)?);
        }
        let official_priority = cfg.repos.iter().find(|r| r.official).map(|r| r.priority);
        Ok(PkgDb { all, priority, official_priority })
    }

    fn repo_priority(&self, repo: &str) -> i32 {
        *self.priority.get(repo).unwrap_or(&0)
    }

    /// Resolve a single name (or `repo:name`) to the winning candidate.
    pub fn resolve(&self, query: &str) -> Option<&AvailPkg> {
        let (pinned, name) = split_pin(query);
        self.all
            .iter()
            .filter(|p| p.id.name == name)
            .filter(|p| pinned.map_or(true, |r| p.repo == r))
            .max_by(|a, b| self.repo_priority(&a.repo).cmp(&self.repo_priority(&b.repo)))
    }

    /// All candidates for a name across repos (highest priority first).
    pub fn candidates(&self, name: &str) -> Vec<&AvailPkg> {
        let mut v: Vec<&AvailPkg> = self.all.iter().filter(|p| p.id.name == name).collect();
        v.sort_by(|a, b| self.repo_priority(&b.repo).cmp(&self.repo_priority(&a.repo)));
        v
    }

    /// Expand a slackpkg-style PATTERN into winning packages.
    ///
    /// A pattern matches: an exact `repo:name` pin, an exact series name
    /// (a, ap, kde, ...), or a substring of the package name. Returns one
    /// winning candidate per distinct package name, highest priority first.
    pub fn match_pattern(&self, pattern: &str) -> Vec<&AvailPkg> {
        let (pinned, term) = split_pin(pattern);
        let is_series = self.all.iter().any(|p| p.series == term);

        let mut winners: HashMap<&str, &AvailPkg> = HashMap::new();
        for p in &self.all {
            if let Some(r) = pinned {
                if p.repo != r {
                    continue;
                }
            }
            // If the term names a Slackware series (a, ap, kde, y, ...), match
            // exactly that series — not every package whose name happens to
            // contain the letter(s). Otherwise match an exact name or substring.
            let hit = if is_series {
                p.series == term
            } else {
                p.id.name == term || p.id.name.contains(term)
            };
            if !hit {
                continue;
            }
            let better = match winners.get(p.id.name.as_str()) {
                Some(existing) => self.repo_priority(&p.repo) > self.repo_priority(&existing.repo),
                None => true,
            };
            if better {
                winners.insert(p.id.name.as_str(), p);
            }
        }
        let mut out: Vec<&AvailPkg> = winners.into_values().collect();
        out.sort_by(|a, b| {
            self.repo_priority(&b.repo)
                .cmp(&self.repo_priority(&a.repo))
                .then(a.id.name.cmp(&b.id.name))
        });
        out
    }

    /// Search names and summaries (one winner per name).
    pub fn search(&self, term: &str) -> Vec<&AvailPkg> {
        let needle = term.to_lowercase();
        let mut seen: HashMap<&str, &AvailPkg> = HashMap::new();
        for p in &self.all {
            if p.id.name.to_lowercase().contains(&needle)
                || p.summary.to_lowercase().contains(&needle)
            {
                let better = match seen.get(p.id.name.as_str()) {
                    Some(e) => self.repo_priority(&p.repo) > self.repo_priority(&e.repo),
                    None => true,
                };
                if better {
                    seen.insert(p.id.name.as_str(), p);
                }
            }
        }
        let mut out: Vec<&AvailPkg> = seen.into_values().collect();
        out.sort_by(|a, b| a.id.name.cmp(&b.id.name));
        out
    }

    /// Upgrade candidates: installed packages with a differing winning revision.
    /// The priority that an *installed* package should be treated as having,
    /// based on where it came from (its build tag):
    ///   1. a user-defined tag priority (SBo `_SBo`, local `_rtz`, ...)
    ///   2. otherwise, the highest-priority repo that ships packages with the
    ///      same build tag (so `cf` -> conraid, `alien` -> alienbob)
    ///   3. an empty tag (official-style `-1`) -> the official repo's priority
    ///   4. an unknown tag -> i32::MAX, i.e. never auto-replace it
    fn installed_priority(&self, inst: &PkgId, tag_prios: &[crate::config::TagPriority]) -> i32 {
        let tag = inst.build_tag();
        if let Some(tp) = tag_prios.iter().find(|t| t.tag == tag) {
            return tp.priority;
        }
        let from_repo = self
            .all
            .iter()
            .filter(|p| p.id.build_tag() == tag)
            .map(|p| self.repo_priority(&p.repo))
            .max();
        if let Some(p) = from_repo {
            return p;
        }
        if tag.is_empty() {
            // official-style package: treat as the official repo's priority
            return self.official_priority.unwrap_or(i32::MAX);
        }
        i32::MAX // unknown source — protect it
    }

    /// Pending upgrades, respecting source priority so SBo/local packages are
    /// never silently migrated to a lower-priority repo or downgraded.
    ///
    /// For each installed package we take the highest-priority available
    /// candidate (`resolve`) and the installed package's own source priority
    /// (`installed_priority`):
    ///   - candidate from a *higher* priority repo  -> propose (source wins)
    ///   - candidate from an *equal* priority repo   -> propose only if the
    ///     version or build actually differs (a genuine self-upgrade)
    ///   - candidate from a *lower* priority repo    -> skip (no migration down)
    pub fn upgrades_for(
        &self,
        installed: &[PkgId],
        tag_prios: &[crate::config::TagPriority],
    ) -> Vec<Upgrade<'_>> {
        let mut out = Vec::new();
        for inst in installed {
            let Some(avail) = self.resolve(&inst.name) else {
                continue;
            };
            // identical to what's installed: nothing to do
            if avail.id.version == inst.version && avail.id.build == inst.build {
                continue;
            }
            let inst_prio = self.installed_priority(inst, tag_prios);
            let cand_prio = self.repo_priority(&avail.repo);
            let propose = if cand_prio > inst_prio {
                true // higher-priority source wins (even same version)
            } else if cand_prio == inst_prio {
                avail.id.is_other_revision_of(inst) // genuine self-upgrade
            } else {
                false // lower-priority source: never migrate down
            };
            if propose {
                out.push(Upgrade { installed: inst.clone(), available: avail });
            }
        }
        out.sort_by(|a, b| a.installed.name.cmp(&b.installed.name));
        out
    }

    /// install-new: packages newly added to a repo since the last update that
    /// are not installed. `new_filenames` maps repo name -> filenames that
    /// appeared since the previous snapshot.
    pub fn newly_added<'a>(
        &'a self,
        new_by_repo: &HashMap<String, HashSet<String>>,
        installed: &[PkgId],
    ) -> Vec<&'a AvailPkg> {
        let inst_names: HashSet<&str> = installed.iter().map(|p| p.name.as_str()).collect();
        let mut out: Vec<&AvailPkg> = self
            .all
            .iter()
            .filter(|p| {
                new_by_repo
                    .get(&p.repo)
                    .map_or(false, |set| set.contains(&p.filename))
                    && !inst_names.contains(p.id.name.as_str())
            })
            .collect();
        out.sort_by(|a, b| a.id.name.cmp(&b.id.name));
        out.dedup_by(|a, b| a.id.name == b.id.name);
        out
    }

    /// clean-system: installed packages that exist in no configured repo.
    pub fn orphans<'a>(&self, installed: &'a [PkgId]) -> Vec<&'a PkgId> {
        let known: HashSet<&str> = self.all.iter().map(|p| p.id.name.as_str()).collect();
        installed.iter().filter(|p| !known.contains(p.name.as_str())).collect()
    }
}

/// A pending upgrade.
pub struct Upgrade<'a> {
    pub installed: PkgId,
    pub available: &'a AvailPkg,
}

/// Split a `repo:name` pin into (Some(repo), name) or (None, query).
fn split_pin(query: &str) -> (Option<&str>, &str) {
    match query.split_once(':') {
        Some((r, n)) => (Some(r), n),
        None => (None, query),
    }
}

#[cfg(test)]
mod upgrade_tests {
    use super::*;
    use crate::config::TagPriority;
    use crate::pkg::PkgId;
    use crate::repo::AvailPkg;

    fn avail(nv: &str, repo: &str) -> AvailPkg {
        AvailPkg {
            id: PkgId::parse(nv).unwrap(),
            filename: format!("{nv}.txz"),
            location: "./x".into(),
            series: "x".into(),
            size_k: None,
            size_uncompressed_k: None,
            summary: String::new(),
            description: String::new(),
            md5: None,
            repo: repo.into(),
        }
    }

    fn db(pkgs: Vec<AvailPkg>, prios: &[(&str, i32)], official: Option<i32>) -> PkgDb {
        let mut priority = HashMap::new();
        for (n, p) in prios {
            priority.insert(n.to_string(), *p);
        }
        PkgDb { all: pkgs, priority, official_priority: official }
    }

    fn tag(name: &str, t: &str, p: i32) -> TagPriority {
        TagPriority { name: name.into(), tag: t.into(), priority: p }
    }

    #[test]
    fn sbo_is_not_migrated_to_lower_repo() {
        // installed SBo asio; conraid (80) offers a newer build. SBo prio 100.
        let db = db(
            vec![avail("asio-1.36.0-x86_64-1cf", "conraid")],
            &[("conraid", 80)],
            Some(100),
        );
        let installed = vec![PkgId::parse("asio-1.28.2-x86_64-1_SBo").unwrap()];
        let ups = db.upgrades_for(&installed, &[tag("SBo", "_SBo", 100)]);
        assert!(ups.is_empty(), "SBo package must not migrate to conraid");
    }

    #[test]
    fn no_downgrade_from_lower_repo() {
        // installed SBo libdca 0.0.7; alienbob (60) has older 0.0.6. SBo 100.
        let db = db(
            vec![avail("libdca-0.0.6-x86_64-1alien", "alienbob")],
            &[("alienbob", 60)],
            Some(100),
        );
        let installed = vec![PkgId::parse("libdca-0.0.7-x86_64-3_SBo").unwrap()];
        let ups = db.upgrades_for(&installed, &[tag("SBo", "_SBo", 100)]);
        assert!(ups.is_empty(), "must not downgrade across repos");
    }

    #[test]
    fn genuine_self_upgrade_within_same_repo() {
        // installed conraid flatpak 1.17; conraid has 1.18 → upgrade.
        let db = db(
            vec![avail("flatpak-1.18.0-x86_64-2cf", "conraid")],
            &[("conraid", 80)],
            Some(100),
        );
        let installed = vec![PkgId::parse("flatpak-1.17.6-x86_64-1cf").unwrap()];
        let ups = db.upgrades_for(&installed, &[]);
        assert_eq!(ups.len(), 1, "conraid self-upgrade must be proposed");
    }

    #[test]
    fn official_upgrade_proposed() {
        let db = db(
            vec![avail("mkinitrd-1.4.11-x86_64-74", "slackware")],
            &[("slackware", 100)],
            Some(100),
        );
        let installed = vec![PkgId::parse("mkinitrd-1.4.11-x86_64-73").unwrap()];
        let ups = db.upgrades_for(&installed, &[]);
        assert_eq!(ups.len(), 1, "official build bump must upgrade");
    }

    #[test]
    fn series_pattern_matches_only_series() {
        // two game packages in series "y", plus unrelated packages with "y"
        let mut a = avail("nethack-5.0.0-x86_64-4", "slackware");
        a.series = "y".into();
        let mut b = avail("gnugo-3.8-x86_64-1", "slackware");
        b.series = "y".into();
        let mut c = avail("python3-3.12.0-x86_64-1", "slackware");
        c.series = "d".into();
        let mut d = avail("wayland-1.25.0-x86_64-1", "slackware");
        d.series = "l".into();
        let db = db(vec![a, b, c, d], &[("slackware", 100)], Some(100));
        let m = db.match_pattern("y");
        let mut names: Vec<&str> = m.iter().map(|p| p.id.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["gnugo", "nethack"], "series y must not pull *y* names");
    }

    #[test]
    fn substring_pattern_still_works() {
        let mut c = avail("python3-3.12.0-x86_64-1", "slackware");
        c.series = "d".into();
        let db = db(vec![c], &[("slackware", 100)], Some(100));
        let m = db.match_pattern("python");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id.name, "python3");
    }

    #[test]
    fn higher_priority_same_version_is_proposed() {
        // installed alienbob foo 1.0-1alien; conraid (higher, 80) has 1.0-1cf.
        // Both repos carry foo (realistic), so the installed tag `alien`
        // resolves to alienbob's priority (60). user said: same version from a
        // higher repo -> yes, change.
        let db = db(
            vec![
                avail("foo-1.0-x86_64-1cf", "conraid"),
                avail("foo-1.0-x86_64-1alien", "alienbob"),
            ],
            &[("conraid", 80), ("alienbob", 60)],
            Some(100),
        );
        let installed = vec![PkgId::parse("foo-1.0-x86_64-1alien").unwrap()];
        let ups = db.upgrades_for(&installed, &[]);
        assert_eq!(ups.len(), 1, "higher-priority same-version should be proposed");
        assert_eq!(ups[0].available.repo, "conraid");
    }
}
