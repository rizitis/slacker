//! `flatpak remotes --show-disabled --columns=commands::REMOTE_COLUMNS`.

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

    /// No GPG on a remote flatpak uses. A disabled remote is not used, so it
    /// is not worth a warning (its tag still says it).
    pub fn gpg_warning(&self) -> bool {
        self.no_gpg() && !self.disabled()
    }

    /// The remote flatpak makes by itself when an application is installed
    /// from a .flatpak file (NAME-origin): no URL, so nothing can come from
    /// it, enabled or not.
    pub fn is_bundle_origin(&self) -> bool {
        self.url.is_empty()
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
    fn a_disabled_remote_keeps_its_installation() {
        let r = parse(include_str!("../../fixtures/remotes-disabled-1.14.txt"));
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].installation.as_str(), r[0].disabled()), ("system", false));
        assert_eq!((r[1].installation.as_str(), r[1].disabled()), ("user", true));
        // Both have no GPG; only the enabled one is worth a warning.
        assert!(r[0].gpg_warning() && !r[1].gpg_warning());
        assert!(!r[0].is_bundle_origin());
    }

    #[test]
    fn a_bundle_origin_remote_has_no_url() {
        // Real flatpak 1.14.6 output after `flatpak install --user hello.flatpak`
        // (r-tz's breezydesktop-origin and optimusui-origin look the same).
        let r = parse("hello-origin\thello.flatpak\t\t0\tuser,disabled,no-enumerate,no-gpg-verify\n");
        assert_eq!(r.len(), 1);
        assert!(r[0].is_bundle_origin() && r[0].disabled() && !r[0].gpg_warning());
        assert_eq!(r[0].installation, "user");
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
