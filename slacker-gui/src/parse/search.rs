//! `slacker search NAME`
//!
//! One line per hit (cmd_search):
//! `[label] {installed|uninstalled:<11} name-version  summary[ [blacklisted]][  (also: repo name-ver, ...)]`
//! Anything else is slacker explaining why there was no hit.

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Repo name, build tag, or `official`.
    pub source: String,
    pub installed: bool,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub blacklisted: bool,
    /// Other repos shipping the name, as `repo name-version`.
    pub also: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Search {
    pub hits: Vec<Hit>,
    /// Lines that are not hits: slacker's explanation when nothing matched.
    pub messages: Vec<String>,
}

pub fn parse(text: &str) -> Search {
    let mut out = Search::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_hit(line) {
            Some(h) => out.hits.push(h),
            None => out.messages.push(line.trim().to_string()),
        }
    }
    out
}

fn parse_hit(line: &str) -> Option<Hit> {
    let rest = line.strip_prefix('[')?;
    let (source, rest) = rest.split_once("] ")?;
    // The state word is padded to 11 characters, then one space.
    if rest.len() < 12 || !rest.is_char_boundary(11) {
        return None;
    }
    let installed = match rest[..11].trim_end() {
        "installed" => true,
        "uninstalled" => false,
        _ => return None,
    };
    let rest = rest[11..].strip_prefix(' ')?;
    let (namever, tail) = match rest.split_once("  ") {
        Some((a, b)) => (a, b),
        None => (rest.trim_end(), ""),
    };
    let (name, version) = namever.rsplit_once('-')?;
    if name.is_empty() || version.is_empty() || namever.contains(' ') {
        return None;
    }

    let mut tail = tail.trim_end();
    let mut also = Vec::new();
    if tail.ends_with(')') {
        if let Some(i) = tail.rfind("  (also: ") {
            let list = &tail[i + "  (also: ".len()..tail.len() - 1];
            also = list.split(", ").map(str::to_string).collect();
            tail = &tail[..i];
        } else if let Some(list) = tail.strip_prefix("(also: ") {
            // Empty summary: the "(also: ...)" part follows directly.
            also = list[..list.len() - 1].split(", ").map(str::to_string).collect();
            tail = "";
        }
    }
    let blacklisted = match tail.strip_suffix("[blacklisted]") {
        Some(t) => {
            tail = t.trim_end();
            true
        }
        None => false,
    };

    Some(Hit {
        source: source.to_string(),
        installed,
        name: name.to_string(),
        version: version.to_string(),
        summary: tail.trim().to_string(),
        blacklisted,
        also,
    })
}

/// PACKAGES.TXT summaries repeat the name: `emacs (GNU Emacs)`. Returns the
/// part in parentheses when that is the shape, otherwise the summary itself.
pub fn short_summary<'a>(name: &str, summary: &'a str) -> &'a str {
    summary
        .strip_prefix(name)
        .and_then(|s| s.strip_prefix(" ("))
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::fixture;

    #[test]
    fn captured_installed_hit() {
        let s = parse(&fixture("search-installed.txt"));
        assert!(s.messages.is_empty());
        assert_eq!(
            s.hits,
            vec![Hit {
                source: "slackware".into(),
                installed: true,
                name: "emacs".into(),
                version: "31.1".into(),
                summary: "emacs (GNU Emacs)".into(),
                blacklisted: false,
                also: vec!["extras emacs-31.1".into()],
            }]
        );
        assert_eq!(short_summary("emacs", &s.hits[0].summary), "GNU Emacs");
    }

    #[test]
    fn blacklisted_tagged_and_dashed_names() {
        let s = parse(&fixture("search-mixed.txt"));
        assert!(s.messages.is_empty(), "{:?}", s.messages);
        assert_eq!(s.hits.len(), 3);
        let vlc = &s.hits[0];
        assert!(vlc.blacklisted && !vlc.installed);
        assert_eq!(vlc.summary, "vlc (multimedia player)");
        assert_eq!(vlc.also, vec!["restricted vlc-3.0.21", "conraid vlc-3.0.20"]);
        let sbo = &s.hits[1];
        assert_eq!((sbo.source.as_str(), sbo.installed), ("_SBo", true));
        assert_eq!((sbo.name.as_str(), sbo.version.as_str()), ("sbopkg", "0.38.2"));
        assert_eq!(sbo.summary, "");
        let k = &s.hits[2];
        assert_eq!((k.name.as_str(), k.version.as_str()), ("kernel-generic", "7.1.7"));
    }

    #[test]
    fn no_hit_is_a_message() {
        let s = parse(&fixture("search-none.txt"));
        assert!(s.hits.is_empty());
        assert_eq!(s.messages.len(), 2);
        assert!(s.messages[0].starts_with("No package named 'emacz'"));
    }
}
