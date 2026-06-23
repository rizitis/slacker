//! `check-updates` and `show-changelog`, driven by ChangeLog.txt.

use crate::config::Repo;
use crate::download;
use crate::repo;
use std::path::Path;

/// Pick the repo whose ChangeLog we track: the official one, else highest
/// priority.
pub fn changelog_repo(repos: &[Repo]) -> Option<&Repo> {
    repos
        .iter()
        .find(|r| r.official)
        .or_else(|| repos.iter().max_by_key(|r| r.priority))
}

/// Return the cached ChangeLog.txt contents, if present.
pub fn cached_changelog(repo_: &Repo, cache_root: &Path) -> Option<String> {
    repo::read_text_lossy(&repo::meta_path(repo_, cache_root, repo::CHANGELOG)).ok()
}

pub enum UpdateStatus {
    UpToDate,
    Pending,
    Unknown,
}

/// Check a single repo for pending updates, working for official and external
/// repos alike.
///
/// The official (tracked) repo is the only one whose ChangeLog `update`
/// maintains, so only it uses the cheap ChangeLog comparison. Every other repo
/// is checked against its CHECKSUMS.md5 — which `update` does refresh — even if
/// a ChangeLog happens to be cached for it (e.g. from `show-changelog repo`);
/// otherwise that stale, never-refreshed ChangeLog would report "pending"
/// forever. The CHECKSUMS comparison looks only at the per-package md5 entries,
/// so a regenerated header or transport noise never causes a false "pending".
/// Returns Unknown if the repo has never been updated (nothing cached).
pub fn check_repo_updates(repo_: &Repo, cache_root: &Path) -> UpdateStatus {
    // Cheap path: compare ChangeLog, but only for the tracked (official) repo
    // whose ChangeLog `update` actually keeps current.
    if repo_.official {
        if let Some(local_cl) = cached_changelog(repo_, cache_root) {
            return match download::get_bytes(&repo_.join_url(repo::CHANGELOG)) {
                Ok(b) => {
                    if String::from_utf8_lossy(&b) == local_cl {
                        UpdateStatus::UpToDate
                    } else {
                        UpdateStatus::Pending
                    }
                }
                Err(_) => UpdateStatus::Unknown,
            };
        }
    }
    // Uniform path: compare the (smaller) CHECKSUMS.md5 against the cached copy,
    // by its package md5 entries so headers/timestamps/ordering don't matter.
    // Every successfully-updated repo caches CHECKSUMS, and `update` refreshes
    // it, so after an update this reliably returns UpToDate.
    let cached = repo::meta_path(repo_, cache_root, repo::CHECKSUMS);
    let Ok(local) = std::fs::read(&cached) else {
        return UpdateStatus::Unknown; // never updated — nothing to compare
    };
    match download::get_bytes(&repo_.join_url(repo::CHECKSUMS)) {
        Ok(remote) => {
            if repo::checksums_equal(
                &String::from_utf8_lossy(&local),
                &String::from_utf8_lossy(&remote),
            ) {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::Pending
            }
        }
        Err(_) => UpdateStatus::Unknown,
    }
}
