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
    /// Persistent source pins: package name -> repo it is pinned to
    /// (`@repo 100% name` in the blacklist). A pinned name resolves only from
    /// that repo, ignoring priority. Empty for test-built DBs.
    pins: HashMap<String, String>,
}

/// Mark every candidate a blacklist rule matches (tested against its full id,
/// series and candidate repo), once, when the db is built. The whole resolver
/// can then treat a frozen candidate as simply absent — winner-selection skips
/// it and resolution falls through to the next non-frozen candidate — without
/// re-running the blacklist on every lookup.
///
/// Pins are never frozen here: `blacklist_hit` returns false for a pin rule (a
/// pin is the positive "only from this repo" rule, the opposite of a freeze), so
/// a pinned candidate stays available. A `@repo`-scoped rule freezes only that
/// repo's candidate, so resolution falls through to other repos by priority;
/// an unscoped rule matches every candidate, so the package is held everywhere.
fn mark_frozen(all: &mut [AvailPkg], cfg: &Config) {
    for p in all.iter_mut() {
        p.frozen = cfg.blacklist_hit(&p.id.tag(), Some(&p.series), Some(&p.repo));
    }
}

impl PkgDb {
    pub fn load(cfg: &Config) -> Result<PkgDb, String> {
        let mut all = Vec::new();
        let mut priority = HashMap::new();
        for r in &cfg.repos {
            // A quarantined repo (failed safety vetting) is an inert source.
            if repo::is_quarantined(&cfg.state_dir, &r.name) {
                continue;
            }
            priority.insert(r.name.clone(), r.priority);
            all.extend(repo::load_repo(r, &cfg.cache_dir, &cfg.arch)?);
        }
        let official_priority = cfg.repos.iter().find(|r| r.official).map(|r| r.priority);
        let pins = cfg
            .pins()
            .into_iter()
            .map(|(n, r)| (n.to_string(), r.to_string()))
            .collect();
        mark_frozen(&mut all, cfg);
        Ok(PkgDb { all, priority, official_priority, pins })
    }

    /// Like `load`, but tolerant: a repo whose metadata is missing (never
    /// updated, or a wrong/unreachable URL) is skipped and its name returned,
    /// instead of failing the whole load. The read-only reporting commands
    /// (`list-repos`, `status`) use this so one un-updated repo doesn't blank out
    /// attribution for every other repo — the problem repo is isolated and the
    /// rest are reported correctly. Mutating commands keep using strict `load`.
    pub fn load_available(cfg: &Config) -> (PkgDb, Vec<String>) {
        let mut all = Vec::new();
        let mut priority = HashMap::new();
        let mut missing = Vec::new();
        for r in &cfg.repos {
            priority.insert(r.name.clone(), r.priority);
            if repo::is_quarantined(&cfg.state_dir, &r.name) {
                // Inert source: contributes nothing. Reporting commands surface
                // the quarantine separately.
                continue;
            }
            match repo::load_repo(r, &cfg.cache_dir, &cfg.arch) {
                Ok(pkgs) => all.extend(pkgs),
                Err(_) => missing.push(r.name.clone()),
            }
        }
        let official_priority = cfg.repos.iter().find(|r| r.official).map(|r| r.priority);
        let pins = cfg
            .pins()
            .into_iter()
            .map(|(n, r)| (n.to_string(), r.to_string()))
            .collect();
        mark_frozen(&mut all, cfg);
        (PkgDb { all, priority, official_priority, pins }, missing)
    }

    pub fn repo_priority(&self, repo: &str) -> i32 {
        *self.priority.get(repo).unwrap_or(&0)
    }

    /// True if `name` is a configured repo.
    pub fn is_repo(&self, name: &str) -> bool {
        self.priority.contains_key(name)
    }

    /// True if any available package carries this build tag.
    pub fn tag_in_use(&self, tag: &str) -> bool {
        self.all.iter().any(|p| p.id.build_tag() == tag)
    }

    /// The distinct package arch tokens a repo provides (e.g. {"x86_64"},
    /// {"i586"}, {"noarch"}). Empty when the repo provides nothing (no/again
    /// missing metadata). `status` uses it to flag a repo whose packages are all
    /// built for a foreign architecture.
    pub fn repo_archs(&self, repo: &str) -> HashSet<&str> {
        self.all
            .iter()
            .filter(|p| p.repo == repo)
            .map(|p| p.id.arch.as_str())
            .collect()
    }

    /// Sorted list of configured repo names (for diagnostics).
    pub fn all_repos(&self) -> Vec<String> {
        let mut v: Vec<String> = self.priority.keys().cloned().collect();
        v.sort();
        v
    }

    /// Sorted list of non-empty build tags actually in use (for diagnostics).
    pub fn all_build_tags(&self) -> Vec<String> {
        let set: HashSet<String> = self
            .all
            .iter()
            .map(|p| p.id.build_tag().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        v
    }

    /// The set of build tags used by packages in a given repo (e.g. conraid
    /// uses `cf`). Used to map an installed package back to its source repo.
    pub fn repo_build_tags(&self, repo: &str) -> HashSet<String> {
        self.all
            .iter()
            .filter(|p| p.repo == repo)
            .map(|p| p.id.build_tag().to_string())
            .collect()
    }

    /// The series of a package by name (first repo that lists it), if known.
    /// Installed packages carry no series, so series blacklist rules look it up
    /// here.
    pub fn series_of(&self, name: &str) -> Option<&str> {
        self.all.iter().find(|p| p.id.name == name).map(|p| p.series.as_str())
    }

    /// Which repo a (non-empty) build tag belongs to — the first repo shipping a
    /// package with that exact tag. Used to evaluate `@repo` blacklist rules
    /// against an installed package, whose source is recorded only as a tag.
    pub fn repo_for_tag(&self, build_tag: &str) -> Option<&str> {
        self.all.iter().find(|p| p.id.build_tag() == build_tag).map(|p| p.repo.as_str())
    }

    /// Resolve a single name (or `repo:name`) to the winning candidate.
    pub fn resolve(&self, query: &str) -> Option<&AvailPkg> {
        let (explicit, name) = split_pin(query);
        // An explicit `repo:name` (transient) wins; otherwise a persistent pin
        // (`@repo 100% name`) forces the source. Either restricts candidates to
        // that one repo, ignoring priority.
        let forced = explicit.or_else(|| self.pins.get(name).map(String::as_str));
        self.all
            .iter()
            .filter(|p| p.id.name == name)
            .filter(|p| forced.map_or(true, |r| p.repo == r))
            .max_by(|a, b| self.repo_priority(&a.repo).cmp(&self.repo_priority(&b.repo)))
    }

    /// Resolve to the highest-priority candidate that is NOT frozen — the
    /// effective winner once frozen candidates are treated as absent. A pin (or
    /// explicit `repo:name`) restricts the candidate set FIRST, then frozen
    /// candidates are excluded within it; so if the only candidate a pin allows
    /// is frozen, this returns None (the freeze wins on that collision and the
    /// package is held). Returns None when every candidate (in the allowed set)
    /// is frozen. The caller still applies the priority-floor / direction rule,
    /// so this never causes a silent downgrade.
    pub fn resolve_unfrozen(&self, query: &str) -> Option<&AvailPkg> {
        let (explicit, name) = split_pin(query);
        let forced = explicit.or_else(|| self.pins.get(name).map(String::as_str));
        self.all
            .iter()
            .filter(|p| p.id.name == name)
            .filter(|p| forced.map_or(true, |r| p.repo == r))
            .filter(|p| !p.frozen)
            .max_by(|a, b| self.repo_priority(&a.repo).cmp(&self.repo_priority(&b.repo)))
    }

    /// All candidates for a name across repos (highest priority first).
    pub fn candidates(&self, name: &str) -> Vec<&AvailPkg> {
        let mut v: Vec<&AvailPkg> = self.all.iter().filter(|p| p.id.name == name).collect();
        v.sort_by(|a, b| self.repo_priority(&b.repo).cmp(&self.repo_priority(&a.repo)));
        v
    }

    /// Every available package NAME across all (non-quarantined) repos, for
    /// "did you mean" typo suggestions. May repeat a name shipped by several
    /// repos — harmless for the closest-match search.
    pub fn available_names(&self) -> impl Iterator<Item = &str> {
        self.all.iter().map(|p| p.id.name.as_str())
    }

    /// True when no package metadata is loaded at all — i.e. `slacker update` was
    /// never run (or every repo is quarantined). Lets callers say "run update"
    /// instead of the misleading "no match" when the real problem is no data.
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Does `term` name a real Slackware *series* (a, ap, l, kde, ... or an
    /// SBo-style category) rather than just a package whose name equals some
    /// repo's per-package directory? A genuine series groups *several* distinct
    /// package names; a per-package directory (e.g. a repo shipping `ffmpeg`
    /// under `ffmpeg/`, giving it `series == "ffmpeg"`) groups exactly one name
    /// and must NOT shadow a name or `repo:name` query for that package. So a
    /// term counts as a series only if at least two *distinct* package names
    /// share it. Stops at the second distinct name.
    fn is_real_series(&self, term: &str) -> bool {
        let mut first: Option<&str> = None;
        for p in &self.all {
            if p.series != term {
                continue;
            }
            match first {
                None => first = Some(p.id.name.as_str()),
                Some(n) if n != p.id.name => return true,
                _ => {}
            }
        }
        false
    }

    /// Expand a slackpkg-style PATTERN into winning packages.
    ///
    /// A pattern matches: an exact `repo:name` pin, an exact series name
    /// (a, ap, kde, ...), or a substring of the package name. Returns one
    /// winning candidate per distinct package name, highest priority first.
    pub fn match_pattern(&self, pattern: &str) -> Vec<&AvailPkg> {
        // Explicit set selectors:
        //   @repo  -> every package in that repo   (e.g. @gnome)
        //   @_tag  -> every package with that build tag (e.g. @_SBo, @cf)
        // The '@' is required; a bare word is always a package name/substring or
        // a series, never a repo, so there is no ambiguity.
        if let Some(rest) = pattern.strip_prefix('@') {
            let is_repo = self.priority.contains_key(rest);
            let mut out: Vec<&AvailPkg> = self
                .all
                .iter()
                .filter(|p| {
                    if is_repo {
                        p.repo == rest
                    } else {
                        p.id.build_tag() == rest
                    }
                })
                .collect();
            // De-duplicate by name, keeping the highest-priority candidate, so
            // @repo/@tag don't list the same package name twice across repos.
            out.sort_by(|a, b| {
                a.id.name
                    .cmp(&b.id.name)
                    .then(self.repo_priority(&b.repo).cmp(&self.repo_priority(&a.repo)))
            });
            out.dedup_by(|a, b| a.id.name == b.id.name);
            return out;
        }

        let (pinned, term) = split_pin(pattern);
        let is_series = self.is_real_series(term);

        let mut winners: HashMap<&str, &AvailPkg> = HashMap::new();
        for p in &self.all {
            // Explicit `repo:name` (transient) restricts to that repo; otherwise
            // a persistent pin (`@repo 100% name`) makes a pinned name visible
            // only from its repo, so it can never resolve from anywhere else.
            if let Some(r) = pinned {
                if p.repo != r {
                    continue;
                }
            } else if let Some(pin) = self.pins.get(p.id.name.as_str()) {
                if &p.repo != pin {
                    continue;
                }
            }
            // If the term names a real Slackware series (a, ap, kde, y, ...) —
            // a directory that groups several package names — match that series
            // exactly, not every package whose name happens to contain the
            // letter(s). A per-package directory (one name == the dir, e.g. a
            // repo shipping `ffmpeg` under `ffmpeg/`) is NOT a series, so the
            // term falls through to an exact-name/substring match. See
            // `is_real_series`.
            let hit = if is_series {
                p.series == term
            } else {
                p.id.name == term || p.id.name.contains(term)
            };
            if !hit {
                continue;
            }
            let better = match winners.get(p.id.name.as_str()) {
                // Prefer a non-frozen candidate; among the same frozen-status,
                // higher priority wins. So a frozen top-priority candidate is
                // displaced by a lower-priority non-frozen one (the fallback),
                // and only when EVERY candidate is frozen does a frozen one win
                // (then surfaced/skipped as blacklisted by the caller).
                Some(existing) => {
                    (existing.frozen && !p.frozen)
                        || (existing.frozen == p.frozen
                            && self.repo_priority(&p.repo) > self.repo_priority(&existing.repo))
                }
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

    /// Installed packages whose name matches `pattern` but that resolve to
    /// nothing because a persistent pin points them at a repo which does not
    /// (yet) provide the package. Such a package is silently absent from
    /// `match_pattern` (the pin filters it out), so `upgrade`/`reinstall`
    /// surface it here instead of letting it vanish without a word. Returns
    /// (name, pinned_repo) pairs.
    ///
    /// Only plain selectors (name / substring / series) are considered; an
    /// `@set` selector and an explicit `repo:name` selector are not affected
    /// this way.
    pub fn pin_excluded(&self, pattern: &str, installed: &[PkgId]) -> Vec<(String, String)> {
        if pattern.starts_with('@') || pattern.contains(':') {
            return Vec::new();
        }
        let is_series = self.is_real_series(pattern);
        let mut out = Vec::new();
        for inst in installed {
            let Some(pin) = self.pins.get(&inst.name) else {
                continue;
            };
            let name_hit = if is_series {
                self.series_of(&inst.name) == Some(pattern)
            } else {
                inst.name == pattern || inst.name.contains(pattern)
            };
            if !name_hit {
                continue;
            }
            // The pinned repo offers no available package of this name, so the
            // pin cannot be satisfied and the package fell out of the match.
            let satisfiable =
                self.all.iter().any(|p| &p.repo == pin && p.id.name == inst.name);
            if !satisfiable {
                out.push((inst.name.clone(), pin.clone()));
            }
        }
        out
    }

    /// Search names and summaries (one winner per name).
    /// Find packages by exact name, case-insensitively (one entry per name, the
    /// highest-priority repo winning). `info` is the place for substrings.
    pub fn search(&self, term: &str) -> Vec<&AvailPkg> {
        let needle = term.to_lowercase();
        let mut seen: HashMap<&str, &AvailPkg> = HashMap::new();
        for p in &self.all {
            if p.id.name.to_lowercase() == needle {
                let better = match seen.get(p.id.name.as_str()) {
                    // Same rule as match_pattern: non-frozen first, then priority,
                    // so search shows the effective (fallback) winner and only
                    // marks [blacklisted] when every candidate is frozen.
                    Some(e) => {
                        (e.frozen && !p.frozen)
                            || (e.frozen == p.frozen
                                && self.repo_priority(&p.repo) > self.repo_priority(&e.repo))
                    }
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
    pub fn installed_priority(&self, inst: &PkgId, tag_prios: &[crate::config::TagPriority]) -> i32 {
        let tag = inst.build_tag();
        if let Some(tp) = tag_prios.iter().find(|t| t.tag == tag) {
            return tp.priority;
        }
        // Prefer the repo that ships THIS package (same name *and* build tag):
        // that is where the installed copy actually came from. A build tag is a
        // vendor marker, but a single package can leak into another repo — e.g.
        // Slackware's official extra/ ships alienbob's `slackpkg+` (`...alien`),
        // so the bare tag `alien` is shared by extras (90) and alienbob (10). A
        // tag-only `.max()` would then treat *every* alien package as 90; keying
        // on name+tag pins each one to its real source (flatpak -> alienbob 10).
        //
        // FROZEN candidates are excluded from this floor: a frozen candidate is
        // treated as if its repo did not offer the package at all, so it must not
        // raise the installed package's source priority either. This is what lets
        // a `@testing`-scoped freeze fall through to the official repo: a tagless
        // package installed from slackware (100) is ALSO served tagless by a
        // higher-priority testing subtree (105); without this exclusion the floor
        // would be 105 and the slackware fallback (100) would look like a
        // downgrade and be refused. With the frozen testing candidate excluded,
        // the floor is correctly 100 and the official update flows.
        let from_pkg = self
            .all
            .iter()
            .filter(|p| !p.frozen && p.id.build_tag() == tag && p.id.name == inst.name)
            .map(|p| self.repo_priority(&p.repo))
            .max();
        if let Some(p) = from_pkg {
            return p;
        }
        // The exact package is no longer offered anywhere; fall back to the
        // highest-priority repo that still ships *something* with this tag.
        let from_tag = self
            .all
            .iter()
            .filter(|p| !p.frozen && p.id.build_tag() == tag)
            .map(|p| self.repo_priority(&p.repo))
            .max();
        if let Some(p) = from_tag {
            return p;
        }
        if tag.is_empty() {
            // official-style package: treat as the official repo's priority
            return self.official_priority.unwrap_or(i32::MAX);
        }
        i32::MAX // unknown source — protect it
    }

    /// True if upgrading `inst` to `candidate` respects the source-priority
    /// rule: the candidate's repo is of *equal or higher* priority than the
    /// installed package's source, so it is not a migration to a lower repo.
    pub fn upgrade_respects_priority(
        &self,
        inst: &PkgId,
        candidate: &AvailPkg,
        tag_prios: &[crate::config::TagPriority],
    ) -> bool {
        self.repo_priority(&candidate.repo) >= self.installed_priority(inst, tag_prios)
    }

    /// True if the installed package `inst` comes from a source of *higher or
    /// equal* priority than `candidate`'s repo — i.e. the priority rule keeps
    /// the installed one rather than replacing it with the candidate.
    pub fn installed_outranks(
        &self,
        inst: &PkgId,
        candidate: &AvailPkg,
        tag_prios: &[crate::config::TagPriority],
    ) -> bool {
        self.installed_priority(inst, tag_prios) >= self.repo_priority(&candidate.repo)
    }

    /// Pending upgrades, respecting source priority so SBo/local packages are
    /// never silently migrated to a lower-priority repo or downgraded, and
    /// respecting the blacklist so a frozen candidate is treated as absent.
    ///
    /// For each installed package we take the highest-priority NON-frozen
    /// candidate (`resolve_unfrozen`) and the installed package's own source
    /// priority (`installed_priority`):
    ///   - candidate from a *higher* priority repo  -> propose (source wins)
    ///   - candidate from an *equal* priority repo   -> propose only if the
    ///     version or build actually differs (a genuine self-upgrade)
    ///   - candidate from a *lower* priority repo    -> skip (no migration down)
    ///
    /// Because a frozen candidate is skipped, a `@repo`-scoped freeze on the
    /// top-priority repo falls through to the next repo by priority (so official
    /// updates still flow), while an unscoped freeze matches every candidate and
    /// leaves the package unchanged. Returns the upgrades plus the names that are
    /// "held": packages that DO have a (newer) candidate but only frozen ones, so
    /// the caller can report them as frozen/skipped rather than silently
    /// dropping them.
    pub fn upgrades_for(
        &self,
        installed: &[PkgId],
        tag_prios: &[crate::config::TagPriority],
    ) -> (Vec<Upgrade<'_>>, Vec<String>) {
        let mut out = Vec::new();
        let mut held = Vec::new();
        for inst in installed {
            let avail = match self.resolve_unfrozen(&inst.name) {
                Some(a) => a,
                None => {
                    // No non-frozen candidate. If a (different) candidate exists
                    // but is frozen, the package is being held by the blacklist —
                    // report it so the caller can say "frozen (skipped)". If the
                    // frozen candidate is identical to what's installed, there is
                    // nothing to hold back, so stay quiet.
                    if let Some(raw) = self.resolve(&inst.name) {
                        if raw.frozen
                            && !(raw.id.version == inst.version && raw.id.build == inst.build)
                        {
                            held.push(inst.name.clone());
                        }
                    }
                    continue;
                }
            };
            // identical to what's installed: nothing to do
            if avail.id.version == inst.version && avail.id.build == inst.build {
                continue;
            }
            let propose = if self.pins.contains_key(&inst.name) {
                // A pin forces its repo as the source regardless of priority or
                // migration direction. resolve_unfrozen() already returned the
                // pinned repo's (non-frozen) candidate, and it differs from
                // what's installed (exact matches were skipped above), so the
                // package now tracks the pinned repo.
                avail.id.is_other_revision_of(inst)
            } else {
                let inst_prio = self.installed_priority(inst, tag_prios);
                let cand_prio = self.repo_priority(&avail.repo);
                if cand_prio > inst_prio {
                    true // higher-priority source wins (even same version)
                } else if cand_prio == inst_prio {
                    avail.id.is_other_revision_of(inst) // genuine self-upgrade
                } else {
                    false // lower-priority source: never migrate down
                }
            };
            if propose {
                out.push(Upgrade { installed: inst.clone(), available: avail });
            }
        }
        out.sort_by(|a, b| a.installed.name.cmp(&b.installed.name));
        held.sort();
        held.dedup();
        (out, held)
    }

    /// install-new: packages newly added to a repo since the last update that
    /// are not installed. `new_by_repo` maps repo name -> package *names* that
    /// appeared since the previous snapshot (a new build/version of an existing
    /// name is an upgrade, not a new package, so we key on name, not filename).
    ///
    /// NOTE: `install-new` no longer uses this — it now offers every official
    /// package that is not installed (so it also catches removed ones), instead
    /// of only names added since the last update. Kept on purpose (still covered
    /// by its unit test) in case the "added since last update" notion is wanted
    /// again later or elsewhere.
    #[allow(dead_code)]
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
                    .map_or(false, |set| set.contains(&p.id.name))
                    && !inst_names.contains(p.id.name.as_str())
            })
            .collect();
        out.sort_by(|a, b| a.id.name.cmp(&b.id.name));
        out.dedup_by(|a, b| a.id.name == b.id.name);
        out
    }

    /// generate-template: installed packages whose name exists in no configured
    /// repo (so they couldn't be reinstalled). clean-system does NOT use this; it
    /// has its own tag-aware rule built on `names_provided_by`.
    pub fn orphans<'a>(&self, installed: &'a [PkgId]) -> Vec<&'a PkgId> {
        let known: HashSet<&str> = self.all.iter().map(|p| p.id.name.as_str()).collect();
        installed.iter().filter(|p| !known.contains(p.name.as_str())).collect()
    }

    /// The set of package *names* provided by the given repos (or by every repo
    /// when `scope_repos` is `None`). clean-system uses this as the baseline of
    /// names that keep a tagless (empty-tag) installed package.
    pub fn names_provided_by<'a>(
        &'a self,
        scope_repos: Option<&HashSet<&str>>,
    ) -> HashSet<&'a str> {
        self.all
            .iter()
            .filter(|p| scope_repos.map_or(true, |set| set.contains(p.repo.as_str())))
            .map(|p| p.id.name.as_str())
            .collect()
    }

    /// Whether any loaded package comes from the named repo. clean-system uses
    /// this to refuse running when a baseline repo's metadata is missing.
    pub fn has_repo_packages(&self, repo: &str) -> bool {
        self.all.iter().any(|p| p.repo == repo)
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
impl PkgDb {
    /// Build a PkgDb from explicit parts, for cross-module tests (e.g. `collect`
    /// in main, which lives outside this module and so cannot use the struct
    /// fields directly). Test-only — compiled out of the release binary.
    pub(crate) fn for_test(
        all: Vec<AvailPkg>,
        prios: &[(&str, i32)],
        official: Option<i32>,
    ) -> PkgDb {
        let mut priority = HashMap::new();
        for (n, p) in prios {
            priority.insert(n.to_string(), *p);
        }
        PkgDb { all, priority, official_priority: official, pins: HashMap::new() }
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
            sha: None,
            required: Vec::new(),
            conflicts: Vec::new(),
            suggests: String::new(),
            repo: repo.into(),
            frozen: false,
        }
    }

    fn db(pkgs: Vec<AvailPkg>, prios: &[(&str, i32)], official: Option<i32>) -> PkgDb {
        let mut priority = HashMap::new();
        for (n, p) in prios {
            priority.insert(n.to_string(), *p);
        }
        PkgDb { all: pkgs, priority, official_priority: official, pins: HashMap::new() }
    }

    fn tag(name: &str, t: &str, p: i32) -> TagPriority {
        TagPriority { name: name.into(), tag: t.into(), priority: p }
    }

    /// An available candidate already marked frozen, as `mark_frozen` would on
    /// load. Lets the resolution tests exercise the freeze-fallback directly.
    fn frozen_avail(nv: &str, repo: &str) -> AvailPkg {
        let mut p = avail(nv, repo);
        p.frozen = true;
        p
    }

    #[test]
    fn scoped_freeze_falls_through_to_next_repo() {
        // conraid's case: testing (105) outranks slackware (100). The incoming
        // testing build is frozen (e.g. `@testing xf86-.*` or `xf86-.*-202.*`);
        // resolution must fall through to slackware so official updates flow.
        let pkgs = vec![
            frozen_avail("xf86-input-evdev-20260421-x86_64-1", "testing"),
            avail("xf86-input-evdev-2.12.0-x86_64-1", "slackware"),
        ];
        let d = db(pkgs, &[("testing", 105), ("slackware", 100)], Some(100));
        // Raw winner is the frozen testing build; the effective winner is slackware.
        assert_eq!(d.resolve("xf86-input-evdev").unwrap().repo, "testing");
        assert_eq!(d.resolve_unfrozen("xf86-input-evdev").unwrap().repo, "slackware");

        // Installed is the older official 2.11.0 (slackware = floor 100).
        let installed = vec![PkgId::parse("xf86-input-evdev-2.11.0-x86_64-1").unwrap()];
        let (ups, held) = d.upgrades_for(&installed, &[]);
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].available.repo, "slackware");
        assert_eq!(ups[0].available.id.version, "2.12.0");
        assert!(held.is_empty(), "official update flows; nothing held");
    }

    #[test]
    fn unscoped_freeze_holds_everywhere() {
        // An unscoped rule matches the candidate in EVERY repo -> all frozen ->
        // the package is held, and reported (a newer candidate does exist).
        let pkgs = vec![
            frozen_avail("xf86-input-evdev-20260421-x86_64-1", "testing"),
            frozen_avail("xf86-input-evdev-2.12.0-x86_64-1", "slackware"),
        ];
        let d = db(pkgs, &[("testing", 105), ("slackware", 100)], Some(100));
        assert!(d.resolve_unfrozen("xf86-input-evdev").is_none());

        let installed = vec![PkgId::parse("xf86-input-evdev-2.11.0-x86_64-1").unwrap()];
        let (ups, held) = d.upgrades_for(&installed, &[]);
        assert!(ups.is_empty());
        assert_eq!(held, vec!["xf86-input-evdev".to_string()]);
    }

    #[test]
    fn freeze_never_falls_below_floor() {
        // Installed from conraid (110). conraid's candidate is frozen; slackware
        // (100) offers a newer build but 100 < 110 = floor, so falling through to
        // it would be a priority DOWNGRADE — never proposed (the invariant holds).
        let pkgs = vec![
            frozen_avail("bar-2.0-x86_64-1cf", "conraid"),
            avail("bar-1.5-x86_64-1", "slackware"),
        ];
        let d = db(pkgs, &[("conraid", 110), ("slackware", 100)], Some(100));
        // Only non-frozen candidate is slackware...
        assert_eq!(d.resolve_unfrozen("bar").unwrap().repo, "slackware");
        // ...but it is below the installed source's floor, so no upgrade is made.
        let installed = vec![PkgId::parse("bar-1.0-x86_64-1cf").unwrap()];
        let (ups, _held) = d.upgrades_for(&installed, &[]);
        assert!(ups.is_empty(), "must never migrate down to a lower-priority repo");
    }

    #[test]
    fn pin_restricts_first_then_freeze_excludes() {
        // A pin restricts the candidate set FIRST; the freeze then excludes within
        // it. Pinned to the frozen repo -> held (freeze wins on the collision).
        // Pinned to a non-frozen repo -> resolves there, ignoring priority.
        let pkgs = vec![
            frozen_avail("vlc-4.0-x86_64-1", "testing"),
            avail("vlc-3.0-x86_64-1", "alienbob"),
            avail("vlc-3.5-x86_64-1", "slackware"),
        ];
        let mut d = db(
            pkgs,
            &[("testing", 105), ("slackware", 100), ("alienbob", 60)],
            Some(100),
        );
        d.pins.insert("vlc".into(), "testing".into());
        assert!(d.resolve_unfrozen("vlc").is_none(), "pinned candidate frozen -> held");
        d.pins.insert("vlc".into(), "alienbob".into());
        assert_eq!(d.resolve_unfrozen("vlc").unwrap().repo, "alienbob");
    }

    #[test]
    fn match_pattern_prefers_non_frozen_winner() {
        let pkgs = vec![
            frozen_avail("xf86-input-evdev-20260421-x86_64-1", "testing"),
            avail("xf86-input-evdev-2.12.0-x86_64-1", "slackware"),
        ];
        let d = db(pkgs, &[("testing", 105), ("slackware", 100)], Some(100));
        let m = d.match_pattern("xf86-input-evdev");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].repo, "slackware", "fallback (non-frozen) wins the pattern");
        assert!(!m[0].frozen);

        // When EVERY candidate is frozen, the winner is a frozen one (so the
        // install path can surface/skip it as blacklisted rather than vanish).
        let pkgs2 = vec![
            frozen_avail("xf86-input-evdev-20260421-x86_64-1", "testing"),
            frozen_avail("xf86-input-evdev-2.12.0-x86_64-1", "slackware"),
        ];
        let d2 = db(pkgs2, &[("testing", 105), ("slackware", 100)], Some(100));
        let m2 = d2.match_pattern("xf86-input-evdev");
        assert_eq!(m2.len(), 1);
        assert!(m2[0].frozen);
    }

    #[test]
    fn pin_forces_repo_and_bypasses_priority() {
        let pkgs = vec![
            avail("vlc-3.0.21-x86_64-1", "conraid"),
            avail("vlc-3.0.20-x86_64-1", "alienbob"),
        ];
        let mut d = db(pkgs, &[("conraid", 80), ("alienbob", 60)], None);

        // No pin: the priority winner (conraid) resolves.
        assert_eq!(d.resolve("vlc").unwrap().repo, "conraid");

        // Pin vlc -> alienbob: resolve now forces the LOWER-priority repo.
        d.pins.insert("vlc".into(), "alienbob".into());
        assert_eq!(d.resolve("vlc").unwrap().repo, "alienbob");
        // match_pattern (install / upgrade <name>) honors the pin too.
        let m = d.match_pattern("vlc");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].repo, "alienbob");
        // An explicit transient `repo:name` still overrides the persistent pin.
        assert_eq!(d.resolve("conraid:vlc").unwrap().repo, "conraid");

        // upgrades_for: installed vlc is conraid's build (higher priority). Without
        // the pin, alienbob's lower-priority build would never be proposed ("never
        // migrate down"); the pin bypasses that guard and proposes alienbob.
        let installed = vec![PkgId::parse("vlc-3.0.21-x86_64-1").unwrap()];
        let (ups, _held) = d.upgrades_for(&installed, &[]);
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].available.repo, "alienbob");
        assert_eq!(ups[0].available.id.version, "3.0.20");

        // Pin to a repo that does not offer the package -> no candidate (stays put).
        d.pins.insert("vlc".into(), "nosuch".into());
        assert!(d.resolve("vlc").is_none());
        assert!(d.upgrades_for(&installed, &[]).0.is_empty());
    }

    #[test]
    fn pin_excluded_flags_unsatisfiable_pins() {
        let pkgs = vec![
            avail("kernel-generic-6.12.94-i686-1", "slackware"),
            avail("kernel-headers-6.12.94-x86-1", "slackware"),
            avail("mac-4.73-i586-1", "alienbob"),
        ];
        let mut d = db(pkgs, &[("slackware", 100), ("alienbob", 60)], Some(100));
        let installed = vec![
            PkgId::parse("kernel-generic-6.12.94-i686-1").unwrap(),
            PkgId::parse("kernel-headers-6.12.94-x86-1").unwrap(),
        ];

        // No pins -> nothing excluded.
        assert!(d.pin_excluded("kernel", &installed).is_empty());

        // Pin kernel-generic -> alienbob, which has no kernel-generic: it falls
        // out of the match and is reported (this was the silent-vanish bug).
        d.pins.insert("kernel-generic".into(), "alienbob".into());
        assert_eq!(
            d.pin_excluded("kernel", &installed),
            vec![("kernel-generic".to_string(), "alienbob".to_string())]
        );
        // The unpinned kernel-headers is never reported.
        assert!(!d
            .pin_excluded("kernel", &installed)
            .iter()
            .any(|(n, _)| n == "kernel-headers"));

        // A pin to a repo that DOES provide it is satisfiable -> not excluded.
        d.pins.insert("kernel-generic".into(), "slackware".into());
        assert!(d.pin_excluded("kernel", &installed).is_empty());

        // @set and explicit repo:name selectors are not affected this way.
        d.pins.insert("kernel-generic".into(), "alienbob".into());
        assert!(d.pin_excluded("@slackware", &installed).is_empty());
        assert!(d.pin_excluded("alienbob:kernel-generic", &installed).is_empty());
    }

    #[test]
    fn available_names_and_is_empty() {
        // Empty database => is_empty true, no names (the "run update first" case).
        let empty = db(vec![], &[], None);
        assert!(empty.is_empty());
        assert_eq!(empty.available_names().count(), 0);

        // Populated => names exposed for "did you mean" suggestions.
        let d = db(
            vec![
                avail("emacs-30.1-x86_64-1", "slackware"),
                avail("vim-9.1-x86_64-1", "slackware"),
            ],
            &[("slackware", 100)],
            Some(100),
        );
        assert!(!d.is_empty());
        let mut names: Vec<&str> = d.available_names().collect();
        names.sort();
        assert_eq!(names, vec!["emacs", "vim"]);
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
        let (ups, _held) = db.upgrades_for(&installed, &[tag("SBo", "_SBo", 100)]);
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
        let (ups, _held) = db.upgrades_for(&installed, &[tag("SBo", "_SBo", 100)]);
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
        let (ups, _held) = db.upgrades_for(&installed, &[]);
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
        let (ups, _held) = db.upgrades_for(&installed, &[]);
        assert_eq!(ups.len(), 1, "official build bump must upgrade");
    }

    #[test]
    fn at_repo_selects_whole_repo() {
        let mut a = avail("gnome-shell-46.0-x86_64-1_gnome", "gnome");
        a.series = "x".into();
        let mut b = avail("mutter-46.0-x86_64-1_gnome", "gnome");
        b.series = "x".into();
        let mut c = avail("bash-5.3-x86_64-1", "slackware");
        c.series = "a".into();
        let db = db(vec![a, b, c], &[("gnome", 101), ("slackware", 100)], Some(100));
        let mut names: Vec<&str> = db.match_pattern("@gnome").iter().map(|p| p.id.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["gnome-shell", "mutter"]);
    }

    #[test]
    fn at_tag_selects_by_build_tag() {
        let mut a = avail("foo-1.0-x86_64-1_SBo", "slackware");
        a.series = "x".into();
        let mut b = avail("bar-1.0-x86_64-1cf", "conraid");
        b.series = "x".into();
        let db = db(vec![a, b], &[("slackware", 100), ("conraid", 80)], Some(100));
        let m = db.match_pattern("@_SBo");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id.name, "foo");
    }

    #[test]
    fn bare_word_is_not_a_repo() {
        // a bare repo-like word matches package names/substrings, not the repo
        let mut a = avail("gnome-shell-46.0-x86_64-1_gnome", "gnome");
        a.series = "x".into();
        let db = db(vec![a], &[("gnome", 101)], Some(100));
        // "gnome" as substring matches gnome-shell by name; repo set would too,
        // but the point is @ is required for repo *set* semantics. Here bare
        // "gnome" hits gnome-shell via substring — and NOT via repo expansion.
        let m = db.match_pattern("gnome");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id.name, "gnome-shell");
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
        let (ups, _held) = db.upgrades_for(&installed, &[]);
        assert_eq!(ups.len(), 1, "higher-priority same-version should be proposed");
        assert_eq!(ups[0].available.repo, "conraid");
    }

    #[test]
    fn newly_added_matches_by_name_not_filename() {
        // Regression guard: cmd_install_new builds new_by_repo keyed by package
        // NAME (e.g. "plasmanano"), so newly_added must look candidates up by
        // p.id.name, not by p.filename ("plasmanano-6.7.1-x86_64-1.txz"). An
        // earlier rename left this comparing filenames, which silently made
        // install-new match nothing.
        let db = db(
            vec![
                avail("hello-1.0-x86_64-1", "r"),
                avail("plasmanano-6.7.1-x86_64-1", "r"),
            ],
            &[("r", 90)],
            None,
        );
        let mut new_by_repo: HashMap<String, HashSet<String>> = HashMap::new();
        new_by_repo.insert("r".into(), HashSet::from(["plasmanano".to_string()]));

        // nothing installed -> the newly-added name is offered
        let news = db.newly_added(&new_by_repo, &[]);
        assert_eq!(news.len(), 1, "the new package must be detected by name");
        assert_eq!(news[0].id.name, "plasmanano");

        // an existing name not in the new set is never offered
        assert!(
            !news.iter().any(|p| p.id.name == "hello"),
            "an unchanged package must not be reported as new"
        );

        // already installed -> excluded
        let installed = vec![PkgId::parse("plasmanano-6.7.1-x86_64-1").unwrap()];
        assert!(
            db.newly_added(&new_by_repo, &installed).is_empty(),
            "an already-installed package must not be reported as new"
        );
    }

    #[test]
    fn names_provided_by_scopes_to_baseline() {
        // keepme is in slackware (official); foo is only in alienbob.
        let db = db(
            vec![
                avail("keepme-1.0-x86_64-1", "slackware"),
                avail("foo-1.0-x86_64-1alien", "alienbob"),
            ],
            &[("alienbob", 10)],
            Some(100),
        );
        // official-only baseline provides keepme, not foo.
        let off: HashSet<&str> = HashSet::from(["slackware"]);
        let names = db.names_provided_by(Some(&off));
        assert!(names.contains("keepme") && !names.contains("foo"));
        // baseline extended with an immutable repo also provides foo's name.
        let both: HashSet<&str> = HashSet::from(["slackware", "alienbob"]);
        assert!(db.names_provided_by(Some(&both)).contains("foo"));
        // None scope = every repo (used by generate-template).
        assert!(db.names_provided_by(None).contains("foo"));
        // repo_for_tag maps the alien tag to alienbob (drives the tagged branch).
        assert_eq!(db.repo_for_tag("alien"), Some("alienbob"));
    }

    // Regression: a vendor build tag shared by two repos. Slackware's official
    // extra/ ships alienbob's `slackpkg+` (`2alien`) at extras' priority (90),
    // while alienbob (10) ships its own alien-tagged packages. installed_priority
    // must pin each installed package to the repo that ships *it*, not take the
    // max repo priority over the whole `alien` tag.
    fn shared_tag_db() -> PkgDb {
        db(
            vec![
                avail("flatpak-1.18.0-x86_64-1alien", "alienbob"), // alien @10
                avail("flatpak-1.18.0-x86_64-2cf", "conraid"),     // cf    @80
                avail("slackpkg+-1.8.2-noarch-2alien", "extras"),  // alien @90 (official extra/)
                avail("bash-5.2.37-x86_64-1", "slackware"),        // empty tag, official @100
            ],
            &[("slackware", 100), ("extras", 90), ("conraid", 80), ("alienbob", 10)],
            Some(100),
        )
    }

    #[test]
    fn installed_priority_pins_alien_pkg_to_its_real_repo() {
        let db = shared_tag_db();
        // flatpak installed from alienbob -> 10, NOT extras' 90.
        let flatpak = PkgId::parse("flatpak-1.18.0-x86_64-1alien").unwrap();
        assert_eq!(db.installed_priority(&flatpak, &[]), 10);
        // slackpkg+ installed from extras -> 90 (it really lives there).
        let spp = PkgId::parse("slackpkg+-1.8.2-noarch-2alien").unwrap();
        assert_eq!(db.installed_priority(&spp, &[]), 90);
    }

    #[test]
    fn conraid_80_now_outranks_installed_alienbob_10() {
        // The point of the fix: alienbob (10) must lose to conraid (80).
        let db = shared_tag_db();
        let flatpak = PkgId::parse("flatpak-1.18.0-x86_64-1alien").unwrap();
        let conraid_cand = avail("flatpak-1.18.0-x86_64-2cf", "conraid");
        assert!(db.upgrade_respects_priority(&flatpak, &conraid_cand, &[]));
        assert!(!db.installed_outranks(&flatpak, &conraid_cand, &[]));
    }

    #[test]
    fn installed_priority_official_empty_tag_keeps_official() {
        let db = shared_tag_db();
        let bash = PkgId::parse("bash-5.2.37-x86_64-1").unwrap();
        assert_eq!(db.installed_priority(&bash, &[]), 100);
    }

    #[test]
    fn installed_priority_falls_back_to_tag_when_pkg_gone() {
        // No repo ships "gone"; fall back to the highest-priority repo that still
        // ships *something* with the tag -> alien exists in extras(90)+alienbob(10).
        let db = shared_tag_db();
        let removed = PkgId::parse("gone-1.0-x86_64-9alien").unwrap();
        assert_eq!(db.installed_priority(&removed, &[]), 90);
    }

    #[test]
    fn installed_priority_user_tag_wins() {
        let db = shared_tag_db();
        let sbo = [tag("SBo", "_SBo", 101)];
        let p = PkgId::parse("anything-1.0-x86_64-1_SBo").unwrap();
        assert_eq!(db.installed_priority(&p, &sbo), 101);
    }
}

#[cfg(test)]
mod series_match_tests {
    use super::*;
    use crate::pkg::PkgId;
    use crate::repo::AvailPkg;

    fn av(nv: &str, repo: &str, series: &str) -> AvailPkg {
        AvailPkg {
            id: PkgId::parse(nv).unwrap(),
            filename: format!("{nv}.txz"),
            location: format!("./{series}"),
            series: series.into(),
            size_k: None,
            size_uncompressed_k: None,
            summary: String::new(),
            description: String::new(),
            md5: None,
            sha: None,
            required: Vec::new(),
            conflicts: Vec::new(),
            suggests: String::new(),
            repo: repo.into(),
            frozen: false,
        }
    }

    fn db(pkgs: Vec<AvailPkg>, prios: &[(&str, i32)]) -> PkgDb {
        let mut priority = HashMap::new();
        for (n, p) in prios {
            priority.insert(n.to_string(), *p);
        }
        PkgDb { all: pkgs, priority, official_priority: None, pins: HashMap::new() }
    }

    // Official slackware ships ffmpeg in the *real* series `l` (many distinct
    // names); alienbob ships ffmpeg alone under `ffmpeg/`, so its series is the
    // package name itself. That per-package directory must NOT turn the term
    // "ffmpeg" into a series query and shadow name/`repo:name` matching.
    fn fx() -> PkgDb {
        db(
            vec![
                av("ffmpeg-8.1.2-x86_64-1", "slackware", "l"),
                av("zlib-1.3-x86_64-1", "slackware", "l"),
                av("glibc-2.39-x86_64-1", "slackware", "l"),
                av("ffmpeg-7.1.3-x86_64-2alien", "alienbob", "ffmpeg"),
            ],
            &[("slackware", 100), ("alienbob", 10)],
        )
    }

    #[test]
    fn is_real_series_needs_two_distinct_names() {
        let db = fx();
        assert!(db.is_real_series("l")); // 3 distinct names share it
        assert!(!db.is_real_series("ffmpeg")); // one name == per-package dir
        assert!(!db.is_real_series("nope")); // unknown series
    }

    #[test]
    fn per_name_dir_does_not_shadow_pin() {
        // Was the bug: `slackware:ffmpeg` returned nothing because "ffmpeg" was
        // mis-read as a series (alienbob's per-name dir) and slackware's ffmpeg
        // lives in series `l`, not `ffmpeg`.
        let db = fx();
        let out = db.match_pattern("slackware:ffmpeg");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.name, "ffmpeg");
        assert_eq!(out[0].repo, "slackware");
    }

    #[test]
    fn per_name_dir_does_not_shadow_bare_name() {
        let db = fx();
        let out = db.match_pattern("ffmpeg");
        assert_eq!(out.len(), 1); // one winner per name
        assert_eq!(out[0].id.name, "ffmpeg");
        assert_eq!(out[0].repo, "slackware"); // 100 beats alienbob 10
    }

    #[test]
    fn real_series_still_matches_by_series() {
        let db = fx();
        let mut names: Vec<&str> =
            db.match_pattern("l").iter().map(|p| p.id.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["ffmpeg", "glibc", "zlib"]);
        // series `l` is slackware's; alienbob's ffmpeg (series "ffmpeg") excluded
        assert!(db.match_pattern("l").iter().all(|p| p.repo == "slackware"));
    }
}
