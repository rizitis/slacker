//! Network and local downloads, plus integrity checking.
//!
//! Supports https/http (via ureq with a native-tls backend) and file:// URLs
//! for a local repo clone, NFS mount, or mounted install media. file:// is
//! handled directly against the filesystem since ureq is HTTP-only.

use md5::{Digest, Md5};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Build a ureq Agent backed by the system TLS (native-tls / OpenSSL).
fn build_agent(timeout: Duration) -> Result<ureq::Agent, String> {
    let connector = native_tls::TlsConnector::new()
        .map_err(|e| format!("failed to initialise TLS backend: {e}"))?;
    Ok(ureq::AgentBuilder::new()
        .timeout(timeout)
        .tls_connector(Arc::new(connector))
        .build())
}

/// Convert a `file://` URL to a filesystem path, or None for other schemes.
///
/// Accepts `file:///abs/path` and `file://localhost/abs/path`, and
/// percent-decodes the path (so `%20` becomes a space).
fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // Optional "localhost" authority.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        // e.g. file://some-host/... — a remote host we can't read locally.
        return None;
    }
    Some(PathBuf::from(percent_decode(rest)))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Fetch a URL fully into memory (for small metadata files).
pub fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = file_url_to_path(url) {
        return std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()));
    }
    let agent = build_agent(Duration::from_secs(60))?;
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Download a (potentially large) package to `dest`.
pub fn download_to(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }

    // Local file:// — just copy from the filesystem.
    if let Some(src) = file_url_to_path(url) {
        std::fs::copy(&src, dest)
            .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dest.display()))?;
        return Ok(());
    }

    // Remote — stream to a .part then rename.
    let agent = build_agent(Duration::from_secs(600))?;
    // Request identity transport encoding. These artifacts (.bz2/.txz) are
    // already compressed, so transport gzip buys nothing here; meanwhile, with
    // ureq's `gzip` feature a gzip-encoded response is decoded transparently
    // while Content-Length still reports the *encoded* length. That mismatch
    // makes byte accounting wrong and can leave the final read blocking until
    // the agent timeout (~600s) instead of stopping at end-of-body.
    let resp = agent
        .get(url)
        .set("Accept-Encoding", "identity")
        .call()
        .map_err(|e| e.to_string())?;
    let tmp = dest.with_extension("part");
    {
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| format!("create {}: {e}", tmp.display()))?;
        std::io::copy(&mut reader, &mut file)
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, dest)
        .map_err(|e| format!("rename into {}: {e}", dest.display()))?;
    Ok(())
}

/// Like download_to, but prints live progress (bytes, and percent when the
/// server sends Content-Length) on a single refreshing line. Used for large
/// downloads such as MANIFEST where the user otherwise can't tell if it stalled.
/// Uses the same std::io::copy path as download_to (counting bytes through a
/// wrapper) and writes straight to `dest`.
pub fn download_to_progress(url: &str, dest: &Path, label: &str) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    // Local file:// — just copy, no progress needed.
    if let Some(src) = file_url_to_path(url) {
        std::fs::copy(&src, dest)
            .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dest.display()))?;
        return Ok(());
    }

    let agent = build_agent(Duration::from_secs(600))?;
    // Ask for an unencoded body so Content-Length matches the bytes we actually
    // write, the stream ends cleanly at EOF (no read blocking until the ~600s
    // timeout), and the percentage below is truthful. See download_to.
    let resp = agent
        .get(url)
        .set("Accept-Encoding", "identity")
        .call()
        .map_err(|e| e.to_string())?;
    // Only trust Content-Length as the percentage denominator when the body is
    // not transfer-encoded. If a server compresses anyway, Content-Length is
    // the *encoded* size while we count decoded bytes, which would pin the bar
    // at a false 100% while data is still arriving — the "stuck at 100%" stall.
    // In that case fall back to a plain byte counter that keeps moving.
    let content_encoded = resp
        .header("Content-Encoding")
        .map(|e| !e.trim().is_empty() && !e.eq_ignore_ascii_case("identity"))
        .unwrap_or(false);
    let total: Option<u64> = if content_encoded {
        None
    } else {
        resp.header("Content-Length").and_then(|s| s.parse().ok())
    };
    let mut reader = resp.into_reader();
    let file = std::fs::File::create(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut writer = ProgressWriter {
        inner: file,
        label: label.to_string(),
        done: 0,
        total,
        last: std::time::Instant::now(),
    };
    std::io::copy(&mut reader, &mut writer).map_err(|e| format!("write {}: {e}", dest.display()))?;
    // Final redraw + newline so the line stays put.
    print_progress(label, writer.done, total);
    println!();
    Ok(())
}

/// A writer that forwards to `inner` and reports cumulative progress.
struct ProgressWriter<W> {
    inner: W,
    label: String,
    done: u64,
    total: Option<u64>,
    last: std::time::Instant,
}

impl<W: std::io::Write> std::io::Write for ProgressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.done += n as u64;
        if self.last.elapsed().as_millis() >= 200 {
            print_progress(&self.label, self.done, self.total);
            self.last = std::time::Instant::now();
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn print_progress(label: &str, done: u64, total: Option<u64>) {
    use std::io::Write as _;
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    match total {
        Some(t) if t > 0 => {
            let pct = (done as f64 / t as f64 * 100.0).min(100.0) as u32;
            print!("\r    {label}: {:.1} / {:.1} MB ({pct}%)    ", mb(done), mb(t));
        }
        _ => print!("\r    {label}: {:.1} MB    ", mb(done)),
    }
    std::io::stdout().flush().ok();
}

/// Compute the md5 of a file as a lowercase hex string.
pub fn md5_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// SHA-256 of a file, computed by shelling out to `sha256sum` (coreutils, always
/// present on Slackware). Kept out-of-process to avoid adding a hashing crate,
/// consistent with how slacker uses the system gpg and bzip2.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let out = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("run sha256sum: {e}"))?;
    if !out.status.success() {
        return Err(format!("sha256sum failed for {}", path.display()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let hash = text.split_whitespace().next().unwrap_or("");
    if hash.len() != 64 {
        return Err(format!("unexpected sha256sum output for {}", path.display()));
    }
    Ok(hash.to_string())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn md5_of_known_content() {
        let dir = std::env::temp_dir().join("slacker_test_md5");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("abc");
        let mut file = std::fs::File::create(&f).unwrap();
        file.write_all(b"abc").unwrap();
        assert_eq!(md5_file(&f).unwrap(), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn file_url_parsing() {
        assert_eq!(file_url_to_path("file:///srv/slack/PACKAGES.TXT").unwrap(),
                   PathBuf::from("/srv/slack/PACKAGES.TXT"));
        assert_eq!(file_url_to_path("file://localhost/srv/x").unwrap(),
                   PathBuf::from("/srv/x"));
        assert_eq!(file_url_to_path("file:///mnt/my%20repo/a").unwrap(),
                   PathBuf::from("/mnt/my repo/a"));
        assert!(file_url_to_path("https://example/x").is_none());
        assert!(file_url_to_path("file://remotehost/x").is_none());
    }

    #[test]
    fn file_url_fetch_roundtrip() {
        let dir = std::env::temp_dir().join("slacker_fileurl_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("data.txt");
        std::fs::write(&f, b"local repo contents").unwrap();
        let url = format!("file://{}", f.display());
        assert_eq!(get_bytes(&url).unwrap(), b"local repo contents");
    }

    /// Live TLS smoke test (ignored by default; needs network).
    #[test]
    #[ignore]
    fn tls_handshake_works() {
        let bytes = get_bytes("https://raw.githubusercontent.com/rust-lang/rust/master/README.md");
        assert!(bytes.is_ok(), "TLS fetch failed: {:?}", bytes.err());
    }
}
