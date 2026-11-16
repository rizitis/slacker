//! `flatpak remote-ls --updates --columns=commands::UPDATE_COLUMNS REMOTE`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub id: String,
    pub name: String,
    pub version: String,
    pub branch: String,
    pub arch: String,
    pub origin: String,
    pub download: String,
}

impl Update {
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

pub fn parse(out: &str) -> Vec<Update> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let c = super::cells(l, 7);
            if c[0].is_empty() {
                return None;
            }
            Some(Update {
                id: c[0].clone(),
                name: c[1].clone(),
                version: c[2].clone(),
                branch: c[3].clone(),
                arch: c[4].clone(),
                origin: c[5].clone(),
                download: c[6].clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_flatpak_1_14_output() {
        let u = parse(include_str!("../../fixtures/updates-1.14.txt"));
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].id, "org.test.Hello");
        assert_eq!(u[0].name, "Hello");
        assert_eq!(u[0].title(), "Hello");
        assert_eq!(u[0].version, "");
        assert_eq!(u[0].branch, "stable");
        assert_eq!(u[0].origin, "testrepo");
        assert_eq!(u[0].download, "212 bytes");
        assert!(parse("").is_empty());
    }
}
