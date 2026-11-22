//! `file-search`: find which package ships a given file.
//!
//! `update` keeps each repo's MANIFEST decompressed in the cache
//! (`repo::refresh_manifest`), so here we only read and scan it.

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

const HEADER: &str = "||   Package:";

/// Find `needle` in the paths of a MANIFEST. The text is searched for the
/// needle itself, and only the lines it occurs in are parsed; the package of
/// each hit is the last `Package:` header before it, found by walking the
/// headers forward with the hits, so the whole text is read once.
fn search_manifest(text: &str, repo_name: &str, needle: &str) -> Vec<FileHit> {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    let mut current = String::new();
    let mut header_scan = 0usize; // headers before this offset are taken in
    let mut next_line = 0usize; // a hit inside an already handled line is skipped
    for (at, _) in text.match_indices(needle) {
        if at < next_line {
            continue;
        }
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        next_line = end;
        // Take in the headers between the last hit and this line.
        while let Some(h) = text[header_scan..start].find(HEADER) {
            let hstart = header_scan + h;
            let hend = text[hstart..].find('\n').map_or(text.len(), |i| hstart + i);
            current = basename(text[hstart + HEADER.len()..hend].trim()).to_string();
            header_scan = hend;
        }
        // Nothing before this line is scanned for headers again.
        header_scan = header_scan.max(start);
        let line = &text[start..end];
        let is_entry = matches!(line.as_bytes().first(), Some(b'-' | b'd' | b'l' | b'b' | b'c' | b'p' | b's'));
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

/// One line of a package's listing in a MANIFEST.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `-` file, `d` directory, `l` symlink, ...
    pub kind: u8,
    pub size: u64,
    pub path: String,
}

/// The listings of the packages named in `wanted` (package file names, e.g.
/// `bash-5.3-x86_64-1.txz`), as (file name, entries). Only the `Package:`
/// headers are looked at for every other package.
pub fn entries_of(text: &str, wanted: &std::collections::HashSet<&str>) -> Vec<(String, Vec<Entry>)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(h) = text[pos..].find(HEADER) {
        let hstart = pos + h;
        let hend = text[hstart..].find('\n').map_or(text.len(), |i| hstart + i);
        pos = hend;
        let name = basename(text[hstart + HEADER.len()..hend].trim());
        if !wanted.contains(name) {
            continue;
        }
        let body_end = text[hend..].find(HEADER).map_or(text.len(), |i| hend + i);
        let mut entries = Vec::new();
        for line in text[hend..body_end].lines() {
            let Some(&kind) = line.as_bytes().first() else { continue };
            if !matches!(kind, b'-' | b'd' | b'l' | b'b' | b'c' | b'p' | b's' | b'h') {
                continue;
            }
            let size = line.split_whitespace().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            if let Some(path) = entry_path(line) {
                entries.push(Entry { kind, size, path: path.trim_start_matches("./").to_string() });
            }
        }
        out.push((name.to_string(), entries));
        pos = body_end;
    }
    out
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
    fn hits_take_the_package_above_them() {
        // "bin/" occurs in both packages and in a header line
        let hits = search_manifest(M, "slackware", "bin/");
        let got: Vec<(&str, &str)> = hits.iter().map(|h| (h.package.as_str(), h.path.as_str())).collect();
        assert_eq!(
            got,
            vec![
                ("bash-5.2.21-x86_64-3.txz", "usr/bin/bash"),
                ("wget-1.24-x86_64-1.txz", "usr/bin/wget"),
                ("wget-1.24-x86_64-1.txz", "usr/bin/wget-link"),
            ]
        );
        // a needle only in a header or in the owner field is no hit
        assert!(search_manifest(M, "slackware", "slackware64/a").is_empty());
        assert!(search_manifest(M, "slackware", "root/root").is_empty());
    }

    #[test]
    fn entries_of_the_wanted_packages_only() {
        let want: std::collections::HashSet<&str> = ["wget-1.24-x86_64-1.txz"].into_iter().collect();
        let got = entries_of(M, &want);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "wget-1.24-x86_64-1.txz");
        assert_eq!(
            got[0].1,
            vec![
                Entry { kind: b'-', size: 5678, path: "usr/bin/wget".into() },
                Entry { kind: b'l', size: 0, path: "usr/bin/wget-link".into() },
            ]
        );
    }

    #[test]
    fn symlink_target_stripped() {
        let hits = search_manifest(M, "slackware", "wget-link");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "usr/bin/wget-link");
        assert_eq!(hits[0].package, "wget-1.24-x86_64-1.txz");
    }
}

