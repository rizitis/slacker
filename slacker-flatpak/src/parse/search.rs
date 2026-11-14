//! `flatpak search --columns=commands::SEARCH_COLUMNS TEXT`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub name: String,
    pub description: String,
    pub id: String,
    pub version: String,
    pub branch: String,
    /// Remotes that offer it; flatpak joins several with commas.
    pub remotes: Vec<String>,
}

impl Hit {
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

pub fn parse(out: &str) -> Vec<Hit> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let c = super::cells(l, 6);
            if c[2].is_empty() {
                return None;
            }
            Some(Hit {
                name: c[0].clone(),
                description: c[1].clone(),
                id: c[2].clone(),
                version: c[3].clone(),
                branch: c[4].clone(),
                remotes: c[5].split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sober_is_offered_by_two_remotes_on_two_branches() {
        let h = parse(include_str!("../../fixtures/search-sober-slackware.txt"));
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].id, "org.vinegarhq.Sober");
        assert_eq!((h[0].branch.as_str(), h[0].remotes.as_slice()), ("master", &["sober".to_string()][..]));
        assert_eq!((h[1].version.as_str(), h[1].branch.as_str()), ("1.8.0", "stable"));
        assert_eq!(h[1].remotes, vec!["flathub"]);
        assert_eq!(h[1].description, "Play, chat & explore on Roblox");
    }

    #[test]
    fn rows_without_a_version_keep_their_columns() {
        let h = parse(include_str!("../../fixtures/search-text-slackware.txt"));
        assert_eq!(h.len(), 40);
        let dev = h.iter().find(|x| x.id == "com.sublimetext.three.DevUtils").unwrap();
        assert_eq!(dev.version, "");
        assert_eq!(dev.branch, "21.08");
        assert_eq!(dev.remotes, vec!["flathub"]);
        assert!(parse("").is_empty());
    }
}
