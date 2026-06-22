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
/// repos alike. If a ChangeLog is cached for this repo (the official one), it
/// is compared — cheap. Otherwise the remote PACKAGES.TXT is compared against
/// the cached copy, which works uniformly for any repo. Returns Unknown if the
/// repo has never been updated (nothing cached to compare against).
pub fn check_repo_updates(repo_: &Repo, cache_root: &Path) -> UpdateStatus {
    // Cheap path: compare ChangeLog when we have one cached (official repo).
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
    // Uniform path: compare PACKAGES.TXT against the cached copy.
    let cached = repo::meta_path(repo_, cache_root, repo::PACKAGES_TXT);
    let Ok(local) = std::fs::read(&cached) else {
        return UpdateStatus::Unknown; // never updated — nothing to compare
    };
    match download::get_bytes(&repo_.join_url(repo::PACKAGES_TXT)) {
        Ok(remote) => {
            if remote == local {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::Pending
            }
        }
        Err(_) => UpdateStatus::Unknown,
    }
}
