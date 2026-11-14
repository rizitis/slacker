//! The keyfile `flatpak remote-info --show-metadata` and
//! `flatpak info --show-permissions` print.

/// The sections that say what an application may do, as (section, lines).
/// [Application], [Extension ...] and [Build] are left out: they describe
/// the build, not access.
pub fn access(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut keep = false;
    for l in text.lines().map(str::trim) {
        if let Some(name) = l.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
            keep = matches!(name, "Context" | "Session Bus Policy" | "System Bus Policy");
            if keep {
                out.push((name.to_string(), Vec::new()));
            }
            continue;
        }
        if keep && !l.is_empty() && !l.starts_with('#') {
            if let Some(last) = out.last_mut() {
                last.1.push(l.to_string());
            }
        }
    }
    out.retain(|(_, lines)| !lines.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baobab_asks_for_these() {
        let a = access(include_str!("../../fixtures/remote-info-baobab-slackware.txt"));
        let names: Vec<&str> = a.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Context", "Session Bus Policy", "System Bus Policy"]);
        assert!(a[0].1.contains(&"filesystems=~/.var/app;xdg-run/gvfsd;host;xdg-run/gvfs;".to_string()));
        assert_eq!(a[2].1, vec!["org.freedesktop.hostname1=talk"]);
    }
}
