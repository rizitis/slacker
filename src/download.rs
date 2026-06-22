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
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
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
