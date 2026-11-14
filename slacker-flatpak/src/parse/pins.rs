//! `flatpak pin --user|--system`: the runtimes flatpak keeps on purpose.
//! A runtime installed by name is pinned by flatpak itself, and
//! `flatpak uninstall --unused` never removes a pinned one.
//!
//! In plain output each pattern is on its own line, indented; on a terminal a
//! "Pinned patterns:" title comes first. A pattern is a whole or partial ref:
//! `runtime/ID/ARCH/BRANCH`, or shorter such as `ID//BRANCH`. An empty or
//! missing part matches anything, and `*` matches any run of characters.

pub fn parse(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.ends_with(':') && !l.contains(' '))
        .map(String::from)
        .collect()
}

/// Does `pattern` cover the runtime ID/ARCH/BRANCH?
pub fn covers(pattern: &str, id: &str, arch: &str, branch: &str) -> bool {
    if pattern.starts_with("app/") {
        return false;
    }
    let p = pattern.strip_prefix("runtime/").unwrap_or(pattern);
    let mut parts = p.split('/');
    let want = [id, arch, branch];
    for w in want {
        match parts.next() {
            None => return true,
            Some("") => continue,
            Some(part) => {
                if !glob(part, w) {
                    return false;
                }
            }
        }
    }
    parts.next().is_none()
}

/// `*` matches any run of characters; everything else must be equal.
fn glob(pattern: &str, text: &str) -> bool {
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else { return false };
    let pieces: Vec<&str> = pieces.collect();
    let Some((last, middle)) = pieces.split_last() else { return rest.is_empty() };
    for m in middle {
        match rest.find(m) {
            Some(i) => rest = &rest[i + m.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_flatpak_1_14_output_and_the_terminal_title() {
        let p = parse(include_str!("../../fixtures/pin-user-1.14.txt"));
        assert_eq!(p, vec!["org.gnome.Platform//47", "runtime/org.test.Platform/x86_64/1"]);
        // As r-tz's terminal printed it.
        let t = parse("Pinned patterns:\n  runtime/org.freedesktop.Platform/x86_64/24.08\n");
        assert_eq!(t, vec!["runtime/org.freedesktop.Platform/x86_64/24.08"]);
        assert!(parse("").is_empty());
    }

    #[test]
    fn whole_partial_and_glob_patterns() {
        let full = "runtime/org.freedesktop.Platform/x86_64/24.08";
        assert!(covers(full, "org.freedesktop.Platform", "x86_64", "24.08"));
        assert!(!covers(full, "org.freedesktop.Platform", "x86_64", "26.08"));
        // Not the GL extension, whose ID only starts the same.
        assert!(!covers(full, "org.freedesktop.Platform.GL.default", "x86_64", "24.08"));
        assert!(covers("org.gnome.Platform//47", "org.gnome.Platform", "x86_64", "47"));
        assert!(!covers("org.gnome.Platform//47", "org.gnome.Platform", "x86_64", "49"));
        assert!(covers("org.gnome.Platform", "org.gnome.Platform", "aarch64", "50"));
        assert!(covers("runtime/org.freedesktop.Platform.GL.*/x86_64/24.08", "org.freedesktop.Platform.GL.default", "x86_64", "24.08"));
        assert!(!covers("app/org.gnome.baobab", "org.gnome.baobab", "x86_64", "stable"));
        assert!(!covers("runtime/a.b.c/x86_64/1/extra", "a.b.c", "x86_64", "1"));
    }
}
