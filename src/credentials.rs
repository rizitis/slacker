//! Repository authentication credentials (HTTP Basic), modelled on zypper.
//!
//! A secret NEVER lives in the `repos` file. Instead, credentials are kept in
//! files the admin controls, exactly as zypper's `/etc/zypp/credentials.d/` and
//! apt's `/etc/apt/auth.conf.d/`:
//!
//!   * A NAMED credential set in `<config_dir>/credentials.d/<name>`, referenced
//!     from a repo line with the `credentials=<name>` flag (zypper's
//!     `?credentials=<name>`). One `username = ` / `password = ` pair per file.
//!
//!   * A global CATALOG `<config_dir>/credentials.cat` with per-URL sections,
//!     applied by URL prefix to any repo without an explicit reference:
//!
//!         [https://forge.slackware.nl/rizitis]
//!         username = rizitis
//!         password = <token>
//!
//! Lookup order per repo: the `credentials=<name>` reference wins; else the
//! longest matching catalog prefix; else the repo is fetched anonymously.
//!
//! SECURITY: a credential file must be a regular file owned by root (uid 0) with
//! no group/other permission bits (0600 or stricter). Anything looser is
//! REFUSED (with a warning) rather than used — fail-safe, like ssh with private
//! keys. Secrets are only ever sent as an `Authorization: Basic` header and are
//! never printed, logged, or placed in a URL.

use crate::config::Config;
use crate::ui;
use std::path::Path;

/// A username/password pair for HTTP Basic authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub password: String,
}

impl Credential {
    /// The value of an `Authorization: Basic` header for this credential.
    pub fn basic_header(&self) -> String {
        let raw = format!("{}:{}", self.username, self.password);
        format!("Basic {}", base64_encode(raw.as_bytes()))
    }
}

/// Parse a single credential file: `username = ` and `password = ` lines
/// (`:` also accepted as separator, comments with `#`). None if either field is
/// missing.
pub fn parse_credential_file(text: &str) -> Option<Credential> {
    let (mut user, mut pass) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once('=').or_else(|| line.split_once(':')) else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "username" | "user" => user = Some(val.trim().to_string()),
            "password" | "pass" => pass = Some(val.trim().to_string()),
            _ => {}
        }
    }
    Some(Credential {
        username: user?,
        password: pass?,
    })
}

/// Parse the catalog into (url_prefix, Credential) pairs. Sections are headed by
/// `[<url-prefix>]`; each carries its own `username`/`password`. Malformed
/// sections (missing a field) are skipped.
pub fn parse_catalog(text: &str) -> Vec<(String, Credential)> {
    let mut out = Vec::new();
    let mut cur_url: Option<String> = None;
    let mut buf = String::new();
    let flush = |url: &mut Option<String>, buf: &mut String, out: &mut Vec<(String, Credential)>| {
        if let Some(u) = url.take() {
            if let Some(c) = parse_credential_file(buf) {
                out.push((u, c));
            }
        }
        buf.clear();
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            flush(&mut cur_url, &mut buf, &mut out);
            cur_url = Some(inner.trim().to_string());
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    flush(&mut cur_url, &mut buf, &mut out);
    out
}

/// Verify a credential file is safe to read: a regular file, owned by root, with
/// no group/other permission bits. Returns Err(reason) otherwise. On non-Unix
/// (never the target here) the check is skipped.
fn secure_perms(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let md = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if !md.is_file() {
            return Err("not a regular file".to_string());
        }
        if md.uid() != 0 {
            return Err("not owned by root".to_string());
        }
        let mode = md.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!("group/world-accessible (mode {mode:o}, need 600)"));
        }
    }
    let _ = path;
    Ok(())
}

/// Load and validate a named credential set from `credentials.d/<name>`. Returns
/// Ok(None) with a warning if the file is missing or has unsafe permissions, so
/// a misconfigured secret never silently leaks and never aborts the run.
fn load_named(config_dir: &Path, name: &str) -> Option<Credential> {
    let path = config_dir.join("credentials.d").join(name);
    if !path.exists() {
        eprintln!(
            "  {} credentials '{name}' referenced but {} not found — fetching anonymously",
            ui::yellow("!"),
            path.display()
        );
        return None;
    }
    if let Err(reason) = secure_perms(&path) {
        eprintln!(
            "  {} refusing credential file {} — {reason}\n      fix: chown root:root {} && chmod 600 {}",
            ui::yellow("!"),
            path.display(),
            path.display(),
            path.display()
        );
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    match parse_credential_file(&text) {
        Some(c) => Some(c),
        None => {
            eprintln!(
                "  {} credential file {} has no username/password — ignoring",
                ui::yellow("!"),
                path.display()
            );
            None
        }
    }
}

/// Read and validate the catalog, if present. Same permission rules as a named
/// file (it also holds secrets).
fn load_catalog(config_dir: &Path) -> Vec<(String, Credential)> {
    let path = config_dir.join("credentials.cat");
    if !path.exists() {
        return Vec::new();
    }
    if let Err(reason) = secure_perms(&path) {
        eprintln!(
            "  {} refusing credential catalog {} — {reason}\n      fix: chown root:root {} && chmod 600 {}",
            ui::yellow("!"),
            path.display(),
            path.display(),
            path.display()
        );
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_catalog(&text),
        Err(_) => Vec::new(),
    }
}

/// Build the (url_prefix, Authorization-header) registry the downloader consults.
/// For every repo with a `credentials=<name>` flag, the repo URL maps to that
/// named set (these come FIRST, so an explicit reference wins ties). Then every
/// catalog section is added as a prefix rule. The downloader picks the LONGEST
/// matching prefix for each request URL.
/// True for an `https://` URL — the only transport over which slacker will send
/// credentials. Plaintext `http://` would expose them on the wire, so (like apt,
/// which matches credentials to https by default) they are refused there.
fn is_https(url: &str) -> bool {
    url.starts_with("https://")
}

pub fn build_registry(cfg: &Config) -> Vec<(String, String)> {
    let mut reg: Vec<(String, String)> = Vec::new();
    // Explicit per-repo references first.
    for r in &cfg.repos {
        if let Some(name) = &r.credentials {
            if let Some(c) = load_named(&cfg.config_dir, name) {
                if is_https(&r.url) {
                    reg.push((r.url.clone(), c.basic_header()));
                } else if r.insecure {
                    eprintln!(
                        "  {} repo '{}' sends credentials over plaintext http (insecure flag) — exposed on the network, at your own responsibility",
                        ui::yellow("!"),
                        r.name
                    );
                    reg.push((r.url.clone(), c.basic_header()));
                } else {
                    eprintln!(
                        "  {} repo '{}' has credentials but uses plaintext http — NOT sending them (they would leak on the network).\n      \
                         switch this repo to https://, or add the `insecure` flag to send them anyway at your own risk",
                        ui::yellow("!"),
                        r.name
                    );
                }
            }
        }
    }
    // Catalog prefixes next.
    for (prefix, cred) in load_catalog(&cfg.config_dir) {
        if is_https(&prefix) {
            reg.push((prefix, cred.basic_header()));
        } else {
            eprintln!(
                "  {} credentials catalog entry [{}] is plaintext http — ignored (credentials are sent over https only)",
                ui::yellow("!"),
                prefix
            );
        }
    }
    reg
}

/// Standard base64 (RFC 4648) with padding. Small and dependency-free — used
/// only to encode `user:pass` for the Basic header.
pub fn base64_encode(input: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((n >> 18) & 0x3f) as usize] as char);
        out.push(A[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // The classic RFC example.
        assert_eq!(base64_encode(b"Aladdin:open sesame"), "QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    }

    #[test]
    fn basic_header_encodes_user_pass() {
        let c = Credential {
            username: "rizitis".into(),
            password: "t0ken".into(),
        };
        assert_eq!(c.basic_header(), format!("Basic {}", base64_encode(b"rizitis:t0ken")));
    }

    #[test]
    fn parse_credential_file_reads_user_pass() {
        let c = parse_credential_file("username = rizitis\npassword = s3cret\n").unwrap();
        assert_eq!(c.username, "rizitis");
        assert_eq!(c.password, "s3cret");
        // Colon separator, comments, and `user`/`pass` aliases also work.
        let c2 =
            parse_credential_file("# forge\nuser: bob\npass: p@ss\n").unwrap();
        assert_eq!(c2.username, "bob");
        assert_eq!(c2.password, "p@ss");
        // Missing a field -> None.
        assert!(parse_credential_file("username = only\n").is_none());
    }

    #[test]
    fn parse_catalog_reads_url_sections() {
        let text = "\
            [https://forge.slackware.nl/rizitis]\n\
            username = rizitis\n\
            password = tok1\n\
            \n\
            [https://private.example.com/repo]\n\
            username = bob\n\
            password = tok2\n";
        let cat = parse_catalog(text);
        assert_eq!(cat.len(), 2);
        assert_eq!(cat[0].0, "https://forge.slackware.nl/rizitis");
        assert_eq!(cat[0].1.password, "tok1");
        assert_eq!(cat[1].0, "https://private.example.com/repo");
        assert_eq!(cat[1].1.username, "bob");
    }

    #[test]
    fn only_https_transports_credentials() {
        assert!(is_https("https://forge.slackware.nl/rizitis"));
        assert!(!is_https("http://forge.slackware.nl/rizitis"));
        assert!(!is_https("file:///srv/repo"));
    }
}
