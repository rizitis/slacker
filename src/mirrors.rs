//! Mirror discovery: scan the official Slackware mirror list, probe each https
//! mirror's `slackware64-current/PACKAGES.TXT` with a single timed Range read
//! (reusing [`crate::download::first_line`]) to measure REAL HTTP latency and
//! read its index timestamp at the same time, then rank the reachable, fresh
//! mirrors by latency.
//!
//! This replaces the old ICMP-ping approach, which needs root, is widely
//! blocked or returns nonsense (a remote host "0.024 ms"), and says nothing
//! about whether the mirror actually serves a fresh -current. One small request
//! per mirror proves the mirror works, yields its latency, AND yields the
//! timestamp used to drop stale mirrors — all at once.

use crate::download;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The official, canonical Slackware mirror list. Only https mirrors are taken
/// (the downloader cannot use ftp/rsync, and a plaintext-http mirror is never
/// proposed over a secure one).
const MIRRORLIST_URL: &str = "https://mirrors.slackware.com/mirrorlist/";

/// Path appended to a mirror's base URL to reach the -current package index.
const CURRENT_PACKAGES_SUBPATH: &str = "slackware64-current/PACKAGES.TXT";

/// How many mirrors to probe concurrently. Each probe is a tiny Range request,
/// so this is far higher than MAX_PARALLEL (which is tuned for big downloads).
const PROBE_POOL: usize = 24;

/// Per-mirror probe timeout. A mirror that does not return its first line within
/// this is treated as unreachable and dropped.
const PROBE_TIMEOUT_SECS: u64 = 5;

/// How many ranked mirrors to present.
pub const TOP_N: usize = 7;

/// A mirror that answered, with its measured latency and index timestamp.
pub struct MirrorResult {
    pub country: String,
    pub base_url: String,
    pub latency_ms: u128,
    pub pkg_epoch: i64,
}

/// Fetch and parse the mirror list, returning `(country, base_url)` for every
/// https mirror found.
pub fn fetch_https_mirrors() -> Result<Vec<(String, String)>, String> {
    let bytes = download::get_bytes(MIRRORLIST_URL).map_err(|e| format!("fetch mirror list: {e}"))?;
    let page = String::from_utf8_lossy(&bytes);
    Ok(parse_https_mirrors(&page))
}

/// Pull every `https://…` URL out of the mirror-list page, with the two-letter
/// country code that prefixes its line when present. Scheme-based rather than
/// section-based: https URLs only occur in the https section, so this keeps
/// working even if the page's section headers or markup change. Any stray https
/// URL that is not a real mirror simply fails to probe and is dropped.
fn parse_https_mirrors(page: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in page.lines() {
        if let Some(url) = extract_https(line) {
            let country = line
                .split_whitespace()
                .next()
                .filter(|t| t.len() <= 3 && t.chars().all(|c| c.is_ascii_alphabetic()))
                .unwrap_or("")
                .to_string();
            out.push((country, url));
        }
    }
    out
}

/// First `https://…` token on a line, stopping at whitespace or an angle
/// bracket / quote (so both `<https://x/>` and `href="https://x/"` work).
fn extract_https(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Build a mirror's -current PACKAGES.TXT URL from its base URL.
fn candidate_url(base: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), CURRENT_PACKAGES_SUBPATH)
}

/// Probe every candidate concurrently: for each, time a single Range read of its
/// PACKAGES.TXT first line and parse the timestamp. Unreachable or unparseable
/// mirrors are silently dropped — so a wrong base path simply self-eliminates.
pub fn probe_all(mirrors: &[(String, String)]) -> Vec<MirrorResult> {
    let timeout = Duration::from_secs(PROBE_TIMEOUT_SECS);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<MirrorResult>> = Mutex::new(Vec::new());
    let pool = PROBE_POOL.min(mirrors.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..pool {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= mirrors.len() {
                    break;
                }
                let (country, base) = &mirrors[i];
                let url = candidate_url(base);
                let start = Instant::now();
                if let Ok(line) = download::first_line(&url, timeout) {
                    let ms = start.elapsed().as_millis();
                    if let Some(epoch) = crate::parse_packages_date(&line) {
                        results.lock().unwrap().push(MirrorResult {
                            country: country.clone(),
                            base_url: base.clone(),
                            latency_ms: ms,
                            pkg_epoch: epoch,
                        });
                    }
                }
            });
        }
    });
    results.into_inner().unwrap()
}

/// Keep only fresh mirrors (no more than the staleness threshold behind upstream,
/// when an upstream reference is available), sort by latency, and take the
/// fastest `top_n`. With no upstream reference (osuosl unreachable) the freshness
/// filter is skipped and ranking is by latency alone.
pub fn rank(mut results: Vec<MirrorResult>, upstream_epoch: Option<i64>, top_n: usize) -> Vec<MirrorResult> {
    if let Some(up) = upstream_epoch {
        results.retain(|m| !crate::mirror_is_stale(up, m.pkg_epoch));
    }
    results.sort_by_key(|m| m.latency_ms);
    results.truncate(top_n);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
---
meta-abstract: Slackware mirrors
---
### Last Updated: Thu Jun 25 20:54:01 UTC 2026
```
Available https mirrors:

us\t\t <https://mirror.example.us/slackware/>
de\t\t <https://mirror.example.de/pub/slackware/>

Available http mirrors:

us\t\t <http://plain.example.us/slackware/>

Available ftp mirrors:

us\t\t <ftp://ftp.example.us/slackware/>

Available rsync mirrors:

us\t\t <rsync://rsync.example.us/slackware/>
```
";

    #[test]
    fn parses_only_https_with_country() {
        let got = parse_https_mirrors(SAMPLE);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("us".to_string(), "https://mirror.example.us/slackware/".to_string()));
        assert_eq!(got[1], ("de".to_string(), "https://mirror.example.de/pub/slackware/".to_string()));
        // http / ftp / rsync never leak in.
        assert!(got.iter().all(|(_, u)| u.starts_with("https://")));
    }

    #[test]
    fn extract_https_handles_wrappers() {
        assert_eq!(extract_https("xx <https://a/b/>"), Some("https://a/b/".to_string()));
        assert_eq!(extract_https("href=\"https://a/b/\""), Some("https://a/b/".to_string()));
        assert_eq!(extract_https("https://a/b/c"), Some("https://a/b/c".to_string()));
        assert_eq!(extract_https("no url here"), None);
        assert_eq!(extract_https("ftp://only/ftp/"), None);
    }

    #[test]
    fn candidate_url_appends_current_index() {
        assert_eq!(
            candidate_url("https://m/slackware/"),
            "https://m/slackware/slackware64-current/PACKAGES.TXT"
        );
        assert_eq!(
            candidate_url("https://m/slackware"),
            "https://m/slackware/slackware64-current/PACKAGES.TXT"
        );
    }

    fn mk(lat: u128, epoch: i64) -> MirrorResult {
        MirrorResult { country: "x".into(), base_url: "u".into(), latency_ms: lat, pkg_epoch: epoch }
    }

    #[test]
    fn rank_drops_stale_and_sorts_by_latency() {
        let up = 2_000_000i64;
        // lag in seconds: 0 (fresh), 100_000 (<48h, fresh), 200_000 (>48h, stale)
        let results = vec![mk(300, up), mk(50, up - 100_000), mk(10, up - 200_000)];
        let ranked = rank(results, Some(up), TOP_N);
        assert_eq!(ranked.len(), 2); // stale one dropped
        assert_eq!(ranked[0].latency_ms, 50); // fastest fresh first
        assert_eq!(ranked[1].latency_ms, 300);
    }

    #[test]
    fn rank_without_upstream_keeps_all_by_latency() {
        let up = 2_000_000i64;
        let results = vec![mk(300, up), mk(50, up - 100_000), mk(10, up - 200_000)];
        let ranked = rank(results, None, TOP_N);
        assert_eq!(ranked.len(), 3); // no freshness filter
        assert_eq!(ranked[0].latency_ms, 10);
    }

    #[test]
    fn rank_truncates_to_top_n() {
        let up = 1_000i64;
        let results = vec![mk(300, up), mk(50, up), mk(10, up)];
        let ranked = rank(results, Some(up), 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].latency_ms, 10);
        assert_eq!(ranked[1].latency_ms, 50);
    }
}
