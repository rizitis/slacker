//! `slacker list-repos` (cmd_list_repos)

#[derive(Debug, Clone, PartialEq)]
pub enum RepoState {
    Normal,
    /// `[FROZEN]`: hard quarantine (bad metadata or signature).
    Frozen,
    /// `[unreachable — retrying]`: soft quarantine.
    Unreachable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    pub priority: i32,
    pub name: String,
    /// None when slacker prints `?` (no metadata for this repo yet).
    pub installed: Option<u32>,
    pub verify: String,
    pub url: String,
    /// `official`, `immutable`, `subtree`, `credentials=NAME`, `insecure`.
    pub flags: Vec<String>,
    pub state: RepoState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagRule {
    pub priority: i32,
    pub name: String,
    pub tag: String,
    pub installed: u32,
    /// `(declared, no installed package)`
    pub unused: bool,
}

/// One line of the "Repository contents" table (slacker 0.14.3 and later).
/// None where slacker prints `?`.
#[derive(Debug, Clone, PartialEq)]
pub struct Contents {
    pub name: String,
    pub offers: Option<u32>,
    /// What upgrade-all would take from this repo.
    pub upgrades: Option<u32>,
    /// As slacker prints it (`14.2G`).
    pub download: Option<String>,
    pub installed: Option<String>,
    /// `YYYY-MM-DD HH:MM`, UTC.
    pub date: Option<String>,
}

#[derive(Debug, Default)]
pub struct Repos {
    pub repos: Vec<Repo>,
    pub tags: Vec<TagRule>,
    /// Empty with an older slacker, which does not print the table.
    pub contents: Vec<Contents>,
    pub total_installed: Option<u32>,
    /// `Installed under other build tags: N [tag=n, ...]`
    pub other_tags: Option<String>,
    /// e.g. `no metadata yet for: ...`
    pub notes: Vec<String>,
    pub unrecognized: Vec<String>,
}

#[derive(PartialEq)]
enum Section {
    None,
    Repos,
    Tags,
    Contents,
}

pub fn parse(text: &str) -> Repos {
    let mut out = Repos::default();
    let mut section = Section::None;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t == "Configured repositories (highest priority first):" {
            section = Section::Repos;
            continue;
        }
        if t == "Build-tag priorities:" {
            section = Section::Tags;
            continue;
        }
        if t.starts_with("Repository contents") && t.ends_with(':') {
            section = Section::Contents;
            continue;
        }
        if let Some(n) = t.strip_prefix("Total installed packages: ") {
            section = Section::None;
            out.total_installed = n.parse().ok();
            continue;
        }
        if let Some(rest) = t.strip_prefix("Installed under other build tags: ") {
            out.other_tags = Some(rest.to_string());
            continue;
        }
        if t.starts_with("no metadata yet for: ") {
            out.notes.push(t.to_string());
            continue;
        }
        let is_header = t.starts_with("Pri |") || (section == Section::Contents && t.starts_with("Name "));
        let is_rule = t.starts_with("-----+") || (section == Section::Contents && t.starts_with('-'));
        if is_header || is_rule {
            continue;
        }
        let parsed = match section {
            Section::Repos => parse_repo(t).map(|r| out.repos.push(r)),
            Section::Tags => parse_tag(t).map(|r| out.tags.push(r)),
            Section::Contents => parse_contents(t).map(|r| out.contents.push(r)),
            Section::None => None,
        };
        if parsed.is_none() {
            out.unrecognized.push(line.to_string());
        }
    }
    out
}

fn parse_repo(t: &str) -> Option<Repo> {
    let cells: Vec<&str> = t.splitn(5, " | ").collect();
    if cells.len() != 5 {
        return None;
    }
    let priority = cells[0].trim().parse().ok()?;
    let name = cells[1].trim().to_string();
    let installed = match cells[2].trim() {
        "?" => None,
        n => Some(n.parse().ok()?),
    };
    let verify = cells[3].trim().to_string();

    let mut parts = cells[4].split("  ").map(str::trim).filter(|p| !p.is_empty());
    let url = parts.next()?.to_string();
    let mut flags = Vec::new();
    let mut state = RepoState::Normal;
    for p in parts {
        if p == "[FROZEN]" {
            state = RepoState::Frozen;
        } else if p.starts_with("[unreachable") {
            state = RepoState::Unreachable;
        } else if let Some(f) = p.strip_prefix('(').and_then(|f| f.strip_suffix(')')) {
            flags.push(f.to_string());
        } else {
            return None;
        }
    }
    Some(Repo { priority, name, installed, verify, url, flags, state })
}

fn parse_contents(t: &str) -> Option<Contents> {
    let cells: Vec<&str> = t.split(" | ").map(str::trim).collect();
    if cells.len() != 6 {
        return None;
    }
    let known = |c: &str| (c != "?").then(|| c.to_string());
    let num = |c: &str| -> Option<Option<u32>> {
        match c {
            "?" => Some(None),
            n => n.parse().ok().map(Some),
        }
    };
    Some(Contents {
        name: cells[0].to_string(),
        offers: num(cells[1])?,
        upgrades: num(cells[2])?,
        download: known(cells[3]),
        installed: known(cells[4]),
        date: known(cells[5]),
    })
}

fn parse_tag(t: &str) -> Option<TagRule> {
    let cells: Vec<&str> = t.splitn(4, " | ").collect();
    if cells.len() != 4 {
        return None;
    }
    // The count is right-aligned, so trim its padding before looking for the
    // two-space gap in front of the note.
    let last = cells[3].trim();
    let (count, unused) = match last.split_once("  ") {
        Some((n, rest)) if rest.trim() == "(declared, no installed package)" => (n, true),
        Some(_) => return None,
        None => (last, false),
    };
    Some(TagRule {
        priority: cells[0].trim().parse().ok()?,
        name: cells[1].trim().to_string(),
        tag: cells[2].trim().to_string(),
        installed: count.trim().parse().ok()?,
        unused,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::fixture;

    #[test]
    fn captured_table() {
        let r = parse(&fixture("list-repos.txt"));
        assert!(r.unrecognized.is_empty(), "{:?}", r.unrecognized);
        assert_eq!(r.repos.len(), 8);
        assert_eq!(r.tags.len(), 7);
        assert_eq!(r.total_installed, Some(2226));
        let p = &r.repos[0];
        assert_eq!((p.priority, p.name.as_str(), p.installed), (200, "patches", Some(0)));
        assert_eq!(p.flags, vec!["immutable", "subtree"]);
        assert_eq!(
            p.url,
            "file:////home/omen/DOCKER_IMAGES/docker-slackware/TREE/patches"
        );
        let s = &r.repos[1];
        assert_eq!((s.installed, s.flags.clone()), (Some(1887), vec!["official".to_string()]));
        assert_eq!(r.repos[3].name, "conraid");
        assert!(r.repos[3].flags.is_empty());
        let t = &r.tags[6];
        assert_eq!((t.name.as_str(), t.tag.as_str(), t.installed), ("x86_64_rtz", "x86_64_rtz", 2));
    }

    #[test]
    fn contents_table() {
        let r = parse(&fixture("list-repos-contents.txt"));
        assert!(r.unrecognized.is_empty(), "{:?}", r.unrecognized);
        // the two tables above it read as before
        assert_eq!(r.repos.len(), 8);
        assert_eq!(r.tags.len(), 7);
        assert_eq!(r.total_installed, Some(2226));
        assert_eq!(r.contents.len(), 8);
        let p = &r.contents[0];
        assert_eq!((p.name.as_str(), p.offers, p.upgrades), ("patches", Some(187), Some(3)));
        assert_eq!(p.download.as_deref(), Some("1.9G"));
        assert_eq!(p.date.as_deref(), Some("2026-11-24 05:12"));
        let last = r.contents.last().unwrap();
        assert_eq!((last.installed.clone(), last.date.clone()), (None, None));
    }

    #[test]
    fn older_slacker_has_no_contents() {
        let r = parse(&fixture("list-repos.txt"));
        assert!(r.contents.is_empty());
    }

    #[test]
    fn states_flags_and_notes() {
        let r = parse(&fixture("list-repos-problems.txt"));
        assert!(r.unrecognized.is_empty(), "{:?}", r.unrecognized);
        assert_eq!(r.repos[1].installed, None);
        assert_eq!(r.repos[1].verify, "gpg,md5");
        assert_eq!(r.repos[1].state, RepoState::Unreachable);
        let shady = &r.repos[2];
        assert_eq!(shady.state, RepoState::Frozen);
        assert_eq!(shady.flags, vec!["credentials=shady", "insecure"]);
        assert_eq!(shady.verify, "none");
        assert!(r.tags[1].unused && !r.tags[0].unused);
        assert_eq!(r.other_tags.as_deref(), Some("3 [_rtz=2, cf=1]"));
        assert_eq!(r.notes.len(), 1);
    }
}
