//! `file-search`: find which package ships a given file.
//!
//! `update` already decompressed and concatenated the per-arch MANIFEST(s)
//! into a single plain-text MANIFEST per repo, so here we just read and scan
//! it. Decompression itself shells out to the system `bzip2`, avoiding a
//! compiled bzip2 dependency.

use crate::config::Repo;
use crate::repo;
use std::path::Path;

pub struct FileHit {
    pub repo: String,
    pub package: String,
    pub path: String,
}

/// Search every repo's cached (plain) MANIFEST for `needle`.
pub fn file_search(
    repos: &[Repo],
    cache_root: &Path,
    needle: &str,
) -> Result<Vec<FileHit>, String> {
    // MANIFEST paths are relative (e.g. "bin/bash"), so a user-typed absolute
    // path like "/bin/bash" is normalized by dropping the leading slash.
    let needle = needle.strip_prefix('/').unwrap_or(needle);
    let mut hits = Vec::new();
    for r in repos {
        let manifest = repo::meta_path(r, cache_root, repo::MANIFEST);
        let Ok(text) = crate::repo::read_text_lossy(&manifest) else {
            continue; // repo without a manifest; skip
        };
        hits.extend(search_manifest(&text, &r.name, needle));
    }
    Ok(hits)
}

fn search_manifest(text: &str, repo_name: &str, needle: &str) -> Vec<FileHit> {
    let mut hits = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if let Some(idx) = line.find("Package:") {
            let path = line[idx + "Package:".len()..].trim();
            current = basename(path).to_string();
            continue;
        }
        let b = line.as_bytes();
        let is_entry = matches!(b.first(), Some(b'-' | b'd' | b'l' | b'b' | b'c' | b'p' | b's'));
        if !is_entry {
            continue;
        }
        if let Some(path) = entry_path(line) {
            if path.contains(needle) {
                hits.push(FileHit {
                    repo: repo_name.to_string(),
                    package: current.clone(),
                    path: path.to_string(),
                });
            }
        }
    }
    hits
}

/// Extract the path field from a tar-style listing line, dropping any
/// "symlink -> target" suffix.
fn entry_path(line: &str) -> Option<&str> {
    let mut it = line.split_whitespace();
    for _ in 0..5 {
        it.next()?;
    }
    let rest_start = line.find(it.next()?)?;
    let path = &line[rest_start..];
    Some(path.split(" -> ").next().unwrap_or(path).trim())
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: &str = "\
++========================================
||   Package:  ./slackware64/a/bash-5.2.21-x86_64-3.txz
++========================================
drwxr-xr-x root/root         0 2024-01-01 00:00 usr/
-rwxr-xr-x root/root      1234 2024-01-01 00:00 usr/bin/bash
||   Package:  ./slackware64/n/wget-1.24-x86_64-1.txz
-rwxr-xr-x root/root      5678 2024-01-01 00:00 usr/bin/wget
lrwxrwxrwx root/root         0 2024-01-01 00:00 usr/bin/wget-link -> wget
";

    #[test]
    fn finds_file_and_package() {
        let hits = search_manifest(M, "slackware", "bin/bash");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].package, "bash-5.2.21-x86_64-3.txz");
        assert_eq!(hits[0].path, "usr/bin/bash");
    }

    #[test]
    fn symlink_target_stripped() {
        let hits = search_manifest(M, "slackware", "wget-link");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "usr/bin/wget-link");
        assert_eq!(hits[0].package, "wget-1.24-x86_64-1.txz");
    }
}
