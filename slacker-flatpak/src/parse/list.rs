//! `flatpak list --app|--runtime --columns=commands::LIST_COLUMNS`.

use std::collections::BTreeMap;

use crate::commands::Inst;

/// One installed ref, as one row of `flatpak list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub version: String,
    pub branch: String,
    pub arch: String,
    /// "system", "user", or the name of another system installation.
    pub installation: String,
    pub origin: String,
    pub size: String,
    pub active: String,
    pub runtime: String,
}

impl Item {
    pub fn inst(&self) -> Option<Inst> {
        Inst::from_label(&self.installation)
    }

    /// The name if flatpak gave one, else the ID.
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

pub fn parse(out: &str) -> Vec<Item> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let c = super::cells(l, 10);
            if c[0].is_empty() {
                return None;
            }
            Some(Item {
                id: c[0].clone(),
                name: c[1].clone(),
                version: c[2].clone(),
                branch: c[3].clone(),
                arch: c[4].clone(),
                installation: c[5].clone(),
                origin: c[6].clone(),
                size: c[7].clone(),
                active: c[8].clone(),
                runtime: c[9].clone(),
            })
        })
        .collect()
}

/// The same ID, branch and arch installed in more than one installation,
/// e.g. Sober in both user and system. Each group is sorted by installation.
pub fn duplicates(items: &[Item]) -> Vec<Vec<&Item>> {
    let mut groups: BTreeMap<(&str, &str, &str), Vec<&Item>> = BTreeMap::new();
    for i in items {
        groups.entry((&i.id, &i.branch, &i.arch)).or_default().push(i);
    }
    groups
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|mut g| {
            g.sort_by(|a, b| a.installation.cmp(&b.installation));
            g
        })
        .collect()
}

/// True if the copies of a duplicate group are not the same commit.
pub fn differ(group: &[&Item]) -> bool {
    group.windows(2).any(|w| w[0].active != w[1].active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_flatpak_1_14_output() {
        let apps = parse(include_str!("../../fixtures/list-app-1.14.txt"));
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].id, "org.test.Hello");
        assert_eq!(apps[0].version, "");
        assert_eq!(apps[0].installation, "system");
        assert_eq!(apps[1].inst(), Some(Inst::User));
        // flatpak separates the number and unit with a no-break space.
        assert_eq!(apps[0].size, "1.0\u{a0}kB");
        assert_eq!(apps[0].runtime, "org.test.Platform/x86_64/1");

        // The runtime rows leave the empty last cell out.
        let rts = parse(include_str!("../../fixtures/list-runtime-1.14.txt"));
        assert_eq!(rts.len(), 2);
        assert_eq!(rts[0].name, "test platform");
        assert_eq!(rts[0].active, "83e5384409ce");
        assert_eq!(rts[0].runtime, "");
    }

    #[test]
    fn reads_real_flatpak_1_18_output_from_slackware() {
        let apps = parse(include_str!("../../fixtures/list-app-slackware.txt"));
        assert_eq!(apps.len(), 12);
        assert_eq!(apps[0].id, "com.obsproject.Studio");
        assert_eq!(apps[0].name, "OBS Studio");
        assert_eq!(apps[0].size, "593.5\u{a0}MB");
        // An app with no version leaves the cell empty, not shifted.
        assert_eq!(apps[1].id, "com.xronlinux.BreezyDesktop");
        assert_eq!(apps[1].version, "");
        assert_eq!(apps[1].branch, "master");
        assert_eq!(apps[1].origin, "breezydesktop-origin");
    }

    #[test]
    fn sober_in_both_installations_is_a_duplicate() {
        // After the update both copies are the same commit.
        let apps = parse(include_str!("../../fixtures/list-app-slackware.txt"));
        let d = duplicates(&apps);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0][0].id, "org.vinegarhq.Sober");
        assert_eq!(d[0][0].installation, "system");
        assert_eq!(d[0][1].installation, "user");
        assert!(!differ(&d[0]));
        // Different commits: the 1.14 test repo, user one updated, system not.
        let test = parse(include_str!("../../fixtures/list-app-1.14.txt"));
        assert!(differ(&duplicates(&test)[0]));
    }

    #[test]
    fn empty_output_is_empty() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n").is_empty());
    }
}
