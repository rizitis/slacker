//! `flatpak remotes --columns=commands::REMOTE_COLUMNS`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub title: String,
    pub url: String,
    pub priority: String,
    /// "system" or "user" (or another installation's name), taken from options.
    pub installation: String,
    /// Every option flatpak printed, e.g. ["user", "no-gpg-verify"].
    pub options: Vec<String>,
}

impl Remote {
    pub fn disabled(&self) -> bool {
        self.options.iter().any(|o| o == "disabled")
    }

    pub fn no_gpg(&self) -> bool {
        self.options.iter().any(|o| o == "no-gpg-verify")
    }
}

pub fn parse(out: &str) -> Vec<Remote> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let c = super::cells(l, 5);
            if c[0].is_empty() {
                return None;
            }
            let options: Vec<String> =
                c[4].split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
            let installation = options.first().cloned().unwrap_or_default();
            let title = if c[1] == "-" { String::new() } else { c[1].clone() };
            Some(Remote {
                name: c[0].clone(),
                title,
                url: c[2].clone(),
                priority: c[3].clone(),
                installation,
                options,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_flatpak_1_14_output() {
        let r = parse(include_str!("../../fixtures/remotes-1.14.txt"));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, "testrepo");
        assert_eq!(r[0].title, "");
        assert_eq!(r[0].installation, "system");
        assert_eq!(r[1].installation, "user");
        assert!(r[1].no_gpg());
        assert!(!r[1].disabled());
    }

    #[test]
    fn reads_the_remotes_seen_on_slackware() {
        let r = parse(include_str!("../../fixtures/remotes-slackware.txt"));
        assert_eq!(r.len(), 3);
        assert_eq!(r[2].name, "sober");
        assert_eq!(r[2].url, "https://sober.vinegarhq.org/repo/");
        assert_eq!(r[2].installation, "user");
        assert!(!r[2].no_gpg());
    }
}
