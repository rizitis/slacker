//! Network and local downloads, plus integrity checking.
//!
//! Supports https/http (via ureq with a bundled rustls backend — TLS is
//! compiled into the binary with the Mozilla CA roots, so slacker needs no
//! system libssl and runs unchanged on OpenSSL 1.1 and 3.x systems alike) and
//! file:// URLs for a local repo clone, NFS mount, or mounted install media.
//! file:// is handled directly against the filesystem since ureq is HTTP-only.
//! http and https go through a proxy when PROXY in slacker.conf sets one, or
//! says `env` to take https_proxy / http_proxy / all_proxy / no_proxy from the
//! environment (see `proxy_var` and `proxy_for`). Unset: direct, as before.

use md5::{Digest, Md5};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The running Slackware release for the User-Agent: `current` on -current, or
/// the stable version (e.g. `15.0`) read from /etc/os-release — so the UA is
/// truthful on whatever the tool actually runs on, exactly as zypper reports
/// `openSUSE-Leap-15.2`. Read once. Falls back to `current` (slacker's primary
/// target) if the release can't be determined.
fn slackware_release() -> &'static str {
    static REL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    REL.get_or_init(|| {
        crate::dist::parse_release_from_os(
            crate::system::version_id().as_deref(),
            crate::system::version_codename().as_deref(),
        )
        .map(|r| crate::dist::release_suffix(&r))
        .unwrap_or_else(|| "current".to_string())
    })
}

/// Identifying User-Agent, in the spirit of zypper's `ZYpp x.y (curl a.b)
/// openSUSE-Leap-15.2-x86_64`: it names the tool, version, TLS backend, the
/// distribution+release and the architecture, so a mirror operator can
/// recognise slacker as a legitimate package manager (and whitelist it) rather
/// than an anonymous scraper. Honest identification, never browser spoofing.
fn user_agent() -> String {
    format!(
        "slacker/{} (rustls) Slackware-{}-{}",
        env!("CARGO_PKG_VERSION"),
        slackware_release(),
        std::env::consts::ARCH,
    )
}

/// Build a ureq Agent for `url`. TLS is rustls, linked into the binary with the
/// Mozilla CA roots bundled (ureq's `tls` feature), so there is no dependency
/// on a system OpenSSL. Goes through a proxy when PROXY in slacker.conf sets
/// one for `url` (see `proxy_var` and `proxy_for`).
fn build_agent(timeout: Duration, url: &str) -> Result<ureq::Agent, String> {
    let mut b = ureq::AgentBuilder::new().timeout(timeout).user_agent(&user_agent());
    if let Some(p) = proxy_for(url, &proxy_var)? {
        // ureq takes the credentials literally; a URL carries them %-encoded.
        // ureq sends them only to the proxy, in the CONNECT for an https URL.
        // Never add a Proxy-Authorization header to the request itself: ureq
        // keeps such a header across redirects, so a mirror (or anyone on a
        // plain-http path) could redirect to a host of its choosing and
        // receive the proxy password inside the tunnel.
        let decoded = match p.split_once("://").and_then(|(s, r)| r.rsplit_once('@').map(|(c, h)| (s, c, h))) {
            Some((s, cred, host)) => format!("{s}://{}@{host}", percent_decode(cred)),
            None => p.clone(),
        };
        b = b.proxy(ureq::Proxy::new(&decoded).map_err(|e| format!("proxy {}: {e}", hide_password(&p)))?);
    }
    Ok(b.build())
}

/// PROXY and NO_PROXY from slacker.conf (None when unset), set once at
/// startup, before any command runs (see `set_proxy`).
static CONF_PROXY: std::sync::RwLock<(Option<String>, Option<String>)> = std::sync::RwLock::new((None, None));

/// Install slacker.conf's PROXY and NO_PROXY. PROXY unset or `off`: no proxy,
/// whatever the environment says (the default, as before proxies were
/// supported: an environment variable alone never redirects root's
/// downloads). PROXY=env: the environment's https_proxy, http_proxy,
/// all_proxy and no_proxy. PROXY=URL: that proxy for every http and https
/// download, the same under sudo, cron and slacker-gui (pkexec), which clear
/// the environment. NO_PROXY, when set, replaces the environment's no_proxy.
pub fn set_proxy(proxy: Option<String>, no_proxy: Option<String>) {
    if let Ok(mut w) = CONF_PROXY.write() {
        *w = (proxy, no_proxy);
    }
}

/// One proxy variable, as `proxy_for` asks for it, following PROXY (see
/// `set_proxy`).
fn proxy_var(name: &str) -> Option<String> {
    let conf = CONF_PROXY.read().map(|g| g.clone()).unwrap_or_default();
    conf_or_env(name, &conf, &|k| std::env::var(k).ok())
}

fn conf_or_env(
    name: &str,
    (proxy, no_proxy): &(Option<String>, Option<String>),
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    // `name` comes both ways (https_proxy, then HTTPS_PROXY): match it without
    // case, but read the environment under the exact name asked for.
    let key = name.to_ascii_lowercase();
    if key == "no_proxy" && no_proxy.is_some() {
        return no_proxy.clone();
    }
    match proxy.as_deref() {
        None => None,
        Some(p) if p.eq_ignore_ascii_case("off") => None,
        Some(p) if p.eq_ignore_ascii_case("env") => env(name),
        Some(p) => match key.as_str() {
            "all_proxy" => Some(p.to_string()),
            "no_proxy" => env(name),
            _ => None,
        },
    }
}

/// What `slacker status` shows: the proxy downloads use and where it comes
/// from, `Err` when it is one slacker cannot use, None when there is none.
pub fn describe_proxy() -> Option<Result<String, String>> {
    describe_proxy_with(&proxy_var, CONF_PROXY.read().map(|g| g.clone()).unwrap_or_default())
}

fn describe_proxy_with(
    var: &dyn Fn(&str) -> Option<String>,
    conf: (Option<String>, Option<String>),
) -> Option<Result<String, String>> {
    let from_env = match conf.0.as_deref() {
        None => return None,
        Some(p) if p.eq_ignore_ascii_case("off") => return None,
        Some(p) => p.eq_ignore_ascii_case("env"),
    };
    // A host no no_proxy entry can name except `*`.
    let probe = |scheme: &str| proxy_for(&format!("{scheme}://slacker-proxy-probe.invalid/"), var);
    let (https, http) = match (probe("https"), probe("http")) {
        (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
        (Ok(a), Ok(b)) => (a, b),
    };
    let shown = |p: &Option<String>| p.as_deref().map_or("direct".to_string(), hide_password);
    let mut text = match (&https, &http) {
        (None, None) if from_env => return Some(Ok("PROXY=env, but no proxy variable is set: direct".to_string())),
        (None, None) => return None,
        (a, b) if a == b => shown(a),
        (a, b) => format!("https {}, http {}", shown(a), shown(b)),
    };
    text.push_str(if from_env { " (environment, PROXY=env)" } else { " (slacker.conf)" });
    let no_proxy = var("no_proxy").or_else(|| var("NO_PROXY")).filter(|v| !v.trim().is_empty());
    if let Some(n) = no_proxy {
        text.push_str(&format!(", not for: {}", n.trim()));
    }
    Some(Ok(text))
}

/// The proxy for `url`, the way wget and curl read it (so slacker works where
/// slackpkg does): `https_proxy` for https, `http_proxy` for http, then
/// `all_proxy`; lowercase first, then uppercase; none for a host in
/// `no_proxy`. `env` gives the values (`proxy_var`, from PROXY and NO_PROXY in
/// slacker.conf). Only http:// proxies: a plain proxy that forwards http and
/// tunnels https with CONNECT, so TLS still runs end to end with the mirror.
/// ureq's own `try_proxy_from_env` is not used: it takes one variable for every
/// scheme and ignores `no_proxy`, which would send a LAN mirror to the proxy.
fn proxy_for(url: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Option<String>, String> {
    let Some((scheme, rest)) = url.split_once("://") else { return Ok(None) };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Ok(None);
    }
    let var = |name: &str| {
        env(name)
            .or_else(|| env(&name.to_ascii_uppercase()))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let Some(proxy) = var(&format!("{scheme}_proxy")).or_else(|| var("all_proxy")) else {
        return Ok(None);
    };
    let (host, port) = url_host_port(rest, if scheme == "https" { 443 } else { 80 });
    if let Some(list) = var("no_proxy") {
        if no_proxy_matches(&list, &host, port) {
            return Ok(None);
        }
    }
    let proxy = match proxy.split_once("://") {
        None => format!("http://{proxy}"),
        Some((s, _)) if s.eq_ignore_ascii_case("http") => proxy,
        Some((s, _)) => {
            return Err(format!(
                "proxy {}: only http:// proxies are supported, not {s}://",
                hide_password(&proxy)
            ))
        }
    };
    Ok(Some(proxy))
}

/// Host (lowercase, IPv6 without brackets) and port of the part of a URL after
/// `scheme://`.
fn url_host_port(rest: &str, default_port: u16) -> (String, u16) {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (h, after) = v6.split_once(']').unwrap_or((v6, ""));
        (h, after.strip_prefix(':'))
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    let port = port.and_then(|p| p.parse().ok()).unwrap_or(default_port);
    (host.trim_end_matches('.').to_ascii_lowercase(), port)
}

/// `no_proxy` as curl and wget read it: comma- or space-separated; `*` is every
/// host; a name matches itself and its subdomains (a leading `.` or `*.` is the
/// same); an IP address or CIDR range (`10.0.0.0/8`, `fd00::/8`) matches the
/// addresses in it, for a URL that names an address; `:port` limits an entry to
/// that port.
fn no_proxy_matches(list: &str, host: &str, port: u16) -> bool {
    use std::net::IpAddr;
    let host_ip: Option<IpAddr> = host.parse().ok();
    list.split([',', ' ', '\t']).map(str::trim).filter(|e| !e.is_empty()).any(|entry| {
        if entry == "*" {
            return true;
        }
        let entry = entry.to_ascii_lowercase();
        // An optional :port, but not the colons of a bare IPv6 address.
        let (pat, want_port) = match entry.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') || h.ends_with(']') => match p.parse::<u16>() {
                Ok(p) => (h.to_string(), Some(p)),
                Err(_) => (entry.clone(), None),
            },
            _ => (entry.clone(), None),
        };
        if want_port.is_some_and(|p| p != port) {
            return false;
        }
        let pat = pat.trim_start_matches('[').trim_end_matches(']');
        if let Some(ip) = host_ip {
            return match pat.split_once('/') {
                Some((net, bits)) => match (net.parse::<IpAddr>(), bits.parse::<u32>()) {
                    (Ok(net), Ok(bits)) => in_cidr(ip, net, bits),
                    _ => false,
                },
                None => pat.parse::<IpAddr>().is_ok_and(|p| p == ip),
            };
        }
        let name = pat.trim_start_matches('*').trim_start_matches('.').trim_end_matches('.');
        !name.is_empty() && (host == name || host.ends_with(&format!(".{name}")))
    })
}

fn in_cidr(ip: std::net::IpAddr, net: std::net::IpAddr, bits: u32) -> bool {
    use std::net::IpAddr::{V4, V6};
    match (ip, net) {
        (V4(a), V4(n)) if bits <= 32 => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (V6(a), V6(n)) if bits <= 128 => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// A proxy URL for messages, with any password replaced by `***`.
fn hide_password(proxy: &str) -> String {
    let (scheme, rest) = proxy.split_once("://").map_or(("", proxy), |(s, r)| (s, r));
    let sep = if scheme.is_empty() { "" } else { "://" };
    match rest.rsplit_once('@') {
        Some((cred, host)) => {
            let user = cred.split_once(':').map_or(cred, |(u, _)| u);
            format!("{scheme}{sep}{user}:***@{host}")
        }
        None => proxy.to_string(),
    }
}

/// (url-prefix, `Authorization` header) rules, set once at startup from the
/// credentials config. A request to a URL that begins with a rule's prefix
/// carries that rule's header (the LONGEST matching prefix wins). Empty/unset
/// means every request is anonymous — the default for public mirrors.
static AUTH_REGISTRY: std::sync::RwLock<Vec<(String, String)>> =
    std::sync::RwLock::new(Vec::new());

/// Install or replace the credential registry (see `credentials::build_registry`).
/// Called after the config loads, and again whenever a command rewrites the
/// repos file (e.g. add-repo) so that vetting the just-added repo uses its
/// credentials rather than a stale, pre-write registry.
pub fn set_auth(registry: Vec<(String, String)>) {
    if let Ok(mut w) = AUTH_REGISTRY.write() {
        *w = registry;
    }
}

/// The `Authorization` header for a URL, if a credential rule matches. Longest
/// prefix wins, so a specific repo rule beats a broader catalog prefix.
fn auth_for(url: &str) -> Option<String> {
    let reg = AUTH_REGISTRY.read().ok()?;
    reg.iter()
        .filter(|(prefix, _)| url.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, header)| header.clone())
}

/// Attach the matching `Authorization` header to a request, if any. Which URLs
/// carry credentials is decided when the registry is built (`build_registry`):
/// https repos always, http repos only with the `insecure` flag. A plain-https
/// registry entry cannot match an http URL, so an https→http redirect drops the
/// header. The secret only ever travels in this header — never in the URL, a log
/// line, or output.
fn with_auth(req: ureq::Request, url: &str) -> ureq::Request {
    match auth_for(url) {
        Some(h) => req.set("Authorization", &h),
        None => req,
    }
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

/// Hard ceiling for in-memory metadata fetches (PACKAGES.TXT, CHECKSUMS,
/// ChangeLog, GPG-KEY). Far above any real file; stops a hostile repo from
/// streaming gigabytes into memory and OOM-killing slacker (which runs as root).
const MAX_METADATA: u64 = 512 * 1024 * 1024;
/// Hard ceiling for a streamed download to disk (a package, MANIFEST.bz2, or
/// the decompressed MANIFEST). Above any real artifact; stops an unbounded or
/// disk-filling response, including a decompression bomb.
pub const MAX_DOWNLOAD: u64 = 8 * 1024 * 1024 * 1024;

/// Copy `reader` into `writer`, aborting if more than `cap` bytes arrive.
pub fn capped_copy<R: Read, W: std::io::Write>(
    reader: &mut R,
    writer: &mut W,
    cap: u64,
) -> Result<u64, String> {
    let mut limited = reader.take(cap + 1);
    let n = std::io::copy(&mut limited, writer).map_err(|e| e.to_string())?;
    if n > cap {
        return Err(format!(
            "transfer exceeded the {cap}-byte safety limit — aborting (possible oversized \
             or malicious response)"
        ));
    }
    Ok(n)
}

/// Fetch a URL fully into memory (for small metadata files).
pub fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    get_bytes_capped(url, MAX_METADATA)
}

/// Like get_bytes, but refuses anything larger than `cap` bytes.
pub fn get_bytes_capped(url: &str, cap: u64) -> Result<Vec<u8>, String> {
    if let Some(path) = file_url_to_path(url) {
        let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if data.len() as u64 > cap {
            return Err(format!("{} is larger than the {cap}-byte safety limit", path.display()));
        }
        return Ok(data);
    }
    let agent = build_agent(Duration::from_secs(60), url)?;
    let resp = with_auth(agent.get(url).set("Accept-Encoding", "identity"), url)
        .call()
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > cap {
        return Err(format!(
            "response from {url} exceeded the {cap}-byte safety limit — aborting"
        ));
    }
    Ok(buf)
}

/// Read just the FIRST LINE of a remote or local text file, transferring as
/// few bytes as possible. Sends an HTTP `Range` request for the leading bytes;
/// if the server ignores it and returns the whole body, only a small bounded
/// prefix is read and the stream is dropped, so the cost is tiny either way.
/// `status` uses this to compare a mirror's PACKAGES.TXT timestamp against
/// upstream without pulling the multi-megabyte file.
pub fn first_line(url: &str, timeout: Duration) -> Result<String, String> {
    const PEEK: u64 = 256;
    if let Some(path) = file_url_to_path(url) {
        // Local mirror (clone / NFS / install media): read only the prefix.
        let f = std::fs::File::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let mut buf = Vec::new();
        f.take(PEEK).read_to_end(&mut buf).map_err(|e| e.to_string())?;
        return first_nonempty_line(&buf);
    }
    let agent = build_agent(timeout, url)?;
    let resp = with_auth(
        agent
            .get(url)
            .set("Range", "bytes=0-127")
            .set("Accept-Encoding", "identity"),
        url,
    )
    .call()
    .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_reader().take(PEEK).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    first_nonempty_line(&buf)
}

/// First non-blank line of a byte buffer, trimmed. Errors if there is none.
fn first_nonempty_line(buf: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(buf);
    match text.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(l) => Ok(l.to_string()),
        None => Err("response had no readable first line".into()),
    }
}

/// Bytes of one download so far, written by the downloading thread and read by
/// another one that shows progress. `total` is 0 while unknown.
#[derive(Default)]
pub struct Meter {
    pub started: std::sync::atomic::AtomicBool,
    pub done: std::sync::atomic::AtomicU64,
    pub total: std::sync::atomic::AtomicU64,
}

/// A writer that forwards to `inner` and adds what it wrote to a `Meter`.
struct MeterWriter<'a, W> {
    inner: W,
    meter: &'a Meter,
}

impl<W: std::io::Write> std::io::Write for MeterWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.meter.done.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Download a (potentially large) package to `dest`.
pub fn download_to(url: &str, dest: &Path) -> Result<(), String> {
    download_to_metered(url, dest, None)
}

/// download_to, counting the bytes into `meter` when one is given.
pub fn download_to_metered(url: &str, dest: &Path, meter: Option<&Meter>) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    if let Some(m) = meter {
        m.started.store(true, Ordering::Relaxed);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }

    // Local file:// — just copy from the filesystem.
    if let Some(src) = file_url_to_path(url) {
        let n = std::fs::copy(&src, dest)
            .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dest.display()))?;
        if let Some(m) = meter {
            m.total.store(n, Ordering::Relaxed);
            m.done.store(n, Ordering::Relaxed);
        }
        return Ok(());
    }

    // Remote — stream to a .part then rename.
    let agent = build_agent(Duration::from_secs(600), url)?;
    // Request identity transport encoding. These artifacts (.bz2/.txz) are
    // already compressed, so transport gzip buys nothing here; meanwhile, with
    // ureq's `gzip` feature a gzip-encoded response is decoded transparently
    // while Content-Length still reports the *encoded* length. That mismatch
    // makes byte accounting wrong and can leave the final read blocking until
    // the agent timeout (~600s) instead of stopping at end-of-body.
    let resp = with_auth(agent.get(url).set("Accept-Encoding", "identity"), url)
        .call()
        .map_err(|e| e.to_string())?;
    if let Some(m) = meter {
        // Same rule as download_to_progress: Content-Length is the size of
        // what we write only when the body is not transfer-encoded.
        let encoded = resp
            .header("Content-Encoding")
            .is_some_and(|e| !e.trim().is_empty() && !e.eq_ignore_ascii_case("identity"));
        if let Some(t) = resp.header("Content-Length").and_then(|s| s.parse().ok()).filter(|_| !encoded) {
            m.total.store(t, Ordering::Relaxed);
        }
    }
    let tmp = dest.with_extension("part");
    {
        let mut reader = resp.into_reader();
        let file = std::fs::File::create(&tmp)
            .map_err(|e| format!("create {}: {e}", tmp.display()))?;
        let copied = match meter {
            Some(m) => {
                let mut w = MeterWriter { inner: file, meter: m };
                capped_copy(&mut reader, &mut w, MAX_DOWNLOAD)
            }
            None => {
                let mut f = file;
                capped_copy(&mut reader, &mut f, MAX_DOWNLOAD)
            }
        };
        if let Err(e) = copied {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("write {}: {e}", tmp.display()));
        }
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

    let agent = build_agent(Duration::from_secs(600), url)?;
    // Ask for an unencoded body so Content-Length matches the bytes we actually
    // write, the stream ends cleanly at EOF (no read blocking until the ~600s
    // timeout), and the percentage below is truthful. See download_to.
    let resp = with_auth(agent.get(url).set("Accept-Encoding", "identity"), url)
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
    if let Err(e) = capped_copy(&mut reader, &mut writer, MAX_DOWNLOAD) {
        let _ = std::fs::remove_file(dest);
        return Err(format!("write {}: {e}", dest.display()));
    }
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

    fn env_of<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn proxy_follows_the_scheme_like_wget_and_curl() {
        let e = [("https_proxy", "http://p:3128"), ("http_proxy", "proxy.lan:8080")];
        let env = env_of(&e);
        let p = |u: &str| proxy_for(u, &env).unwrap();
        assert_eq!(p("https://slackware.uk/x").as_deref(), Some("http://p:3128"));
        assert_eq!(p("http://slackware.uk/x").as_deref(), Some("http://proxy.lan:8080")); // no scheme: http
        assert_eq!(p("file:///mnt/slackware"), None);
        // uppercase too, lowercase first; all_proxy as the fallback
        let e = [("HTTPS_PROXY", "http://up:1"), ("https_proxy", "http://low:1"), ("ALL_PROXY", "http://all:1")];
        let env = env_of(&e);
        assert_eq!(proxy_for("https://a/", &env).unwrap().as_deref(), Some("http://low:1"));
        assert_eq!(proxy_for("http://a/", &env).unwrap().as_deref(), Some("http://all:1"));
        // nothing set, or set empty: direct
        assert_eq!(proxy_for("https://a/", &env_of(&[])).unwrap(), None);
        assert_eq!(proxy_for("https://a/", &env_of(&[("https_proxy", " ")])).unwrap(), None);
        // a proxy slacker cannot speak is an error, not a silent direct connection
        let e = [("https_proxy", "socks5://u:secret@p:1080")];
        let err = proxy_for("https://a/", &env_of(&e)).unwrap_err();
        assert!(err.contains("only http://") && !err.contains("secret"), "{err}");
    }

    #[test]
    fn no_proxy_skips_the_proxy_for_listed_hosts() {
        let e = [
            ("https_proxy", "http://p:3128"),
            ("no_proxy", "localhost, .lan,mirror.example.org ,10.0.0.0/8,192.168.1.5,[fd00::1],backup:8443,*.internal"),
        ];
        let env = env_of(&e);
        let direct = |u: &str| proxy_for(u, &env).unwrap().is_none();
        assert!(direct("https://localhost/x"));
        assert!(direct("https://mirror.lan/slackware/"));
        assert!(direct("https://mirror.example.org/x"));
        assert!(direct("https://a.mirror.example.org/x")); // subdomain
        assert!(direct("https://10.2.3.4/x"));
        assert!(direct("https://192.168.1.5:8443/x"));
        assert!(direct("https://[fd00::1]/x"));
        assert!(direct("https://backup:8443/x"));
        assert!(direct("https://pkgs.internal/x"));
        assert!(direct("https://user:pw@MIRROR.LAN./x")); // credentials, case, trailing dot
        assert!(!direct("https://slackware.uk/x"));
        assert!(!direct("https://notmirror.example.org/x")); // not a subdomain
        assert!(!direct("https://11.0.0.1/x"));
        assert!(!direct("https://192.168.1.6/x"));
        assert!(!direct("https://backup/x")); // the entry is for port 8443 only
        assert!(direct("https://lan/x")); // ".lan" is the domain itself too, as in curl
        let all = [("https_proxy", "http://p:3128"), ("NO_PROXY", "*")];
        assert!(proxy_for("https://slackware.uk/", &env_of(&all)).unwrap().is_none());
    }

    #[test]
    fn proxy_is_off_unless_slacker_conf_turns_it_on() {
        let env = [("https_proxy", "http://envp:1"), ("no_proxy", "env.lan")];
        let with = |proxy: Option<&str>, no: Option<&str>| {
            let conf = (proxy.map(String::from), no.map(String::from));
            move |k: &str| conf_or_env(k, &conf, &env_of(&env))
        };
        let p = |v: &dyn Fn(&str) -> Option<String>, u: &str| proxy_for(u, v).unwrap();
        // unset or off: direct, the environment is not read (as before)
        assert_eq!(p(&with(None, None), "https://slackware.uk/"), None);
        assert_eq!(p(&with(Some("off"), None), "https://slackware.uk/"), None);
        assert_eq!(p(&with(None, Some("x")), "https://slackware.uk/"), None);
        // env: the environment's variables, its no_proxy unless NO_PROXY is set
        let v = with(Some("ENV"), None);
        assert_eq!(p(&v, "https://slackware.uk/").as_deref(), Some("http://envp:1"));
        assert_eq!(p(&v, "http://slackware.uk/"), None); // no http_proxy in the environment
        assert_eq!(p(&v, "https://env.lan/"), None);
        assert!(p(&with(Some("env"), Some("other.lan")), "https://env.lan/").is_some());
        // the uppercase names too, as wget and curl read them
        let upper = [("HTTPS_PROXY", "http://up:1"), ("NO_PROXY", "up.lan")];
        let conf = (Some("env".to_string()), None);
        let v = |k: &str| conf_or_env(k, &conf, &env_of(&upper));
        assert_eq!(p(&v, "https://slackware.uk/").as_deref(), Some("http://up:1"));
        assert_eq!(p(&v, "https://up.lan/"), None);
        // a URL: that proxy for both schemes; NO_PROXY, else the environment's no_proxy
        let v = with(Some("http://conf:3128"), None);
        assert_eq!(p(&v, "https://slackware.uk/").as_deref(), Some("http://conf:3128"));
        assert_eq!(p(&v, "http://slackware.uk/").as_deref(), Some("http://conf:3128"));
        assert_eq!(p(&v, "https://env.lan/"), None);
        let v = with(Some("http://conf:3128"), Some("mirror.lan"));
        assert!(p(&v, "https://env.lan/").is_some());
        assert_eq!(p(&v, "https://mirror.lan/"), None);

        // what status says
        let conf = |a: Option<&str>, n: Option<&str>| (a.map(String::from), n.map(String::from));
        let d = |a: Option<&str>, n: Option<&str>| describe_proxy_with(&with(a, n), conf(a, n));
        assert_eq!(d(None, None), None);
        assert_eq!(d(Some("off"), None), None);
        assert_eq!(
            d(Some("http://bob:pw@conf:3128"), Some("mirror.lan")),
            Some(Ok("http://bob:***@conf:3128 (slacker.conf), not for: mirror.lan".to_string()))
        );
        assert_eq!(
            d(Some("env"), None),
            Some(Ok("https http://envp:1, http direct (environment, PROXY=env), not for: env.lan".to_string()))
        );
        let empty = (Some("env".to_string()), None);
        assert_eq!(
            describe_proxy_with(&|k| conf_or_env(k, &empty, &|_| None), empty.clone()),
            Some(Ok("PROXY=env, but no proxy variable is set: direct".to_string()))
        );
        assert!(matches!(d(Some("socks5://p:1"), None), Some(Err(e)) if e.contains("only http://")));
    }

    #[test]
    fn proxy_passwords_stay_out_of_messages() {
        assert_eq!(hide_password("http://bob:s3cret@p:3128"), "http://bob:***@p:3128");
        assert_eq!(hide_password("bob:s3cret@p:3128"), "bob:***@p:3128");
        assert_eq!(hide_password("http://p:3128"), "http://p:3128");
    }

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
