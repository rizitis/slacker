//! The Browse page's catalog: every application in the appstream data that
//! flatpak already keeps on disk for the remotes, with its cached icon.
//! Reading only; nothing is downloaded. Runs off the main loop (it is plain
//! std code, so it can go to gio::spawn_blocking).

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::commands;
use crate::parse::appstream::{self, App};

#[derive(Clone, Debug)]
pub struct Entry {
    pub app: App,
    pub remote: String,
    pub icon: Option<PathBuf>,
}

#[derive(Default)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    /// Remotes with no appstream data on disk, or data that could not be read.
    pub notes: Vec<String>,
}

fn active(base: &Path, remote: &str, arch: &str) -> PathBuf {
    base.join("appstream").join(remote).join(arch).join("active")
}

fn read_xml(dir: &Path) -> Result<String, String> {
    let plain = dir.join("appstream.xml");
    if plain.is_file() {
        return std::fs::read_to_string(&plain).map_err(|e| format!("{}: {e}", plain.display()));
    }
    let gz = dir.join("appstream.xml.gz");
    let f = std::fs::File::open(&gz).map_err(|e| format!("{}: {e}", gz.display()))?;
    let mut s = String::new();
    flate2::read::GzDecoder::new(f)
        .read_to_string(&mut s)
        .map_err(|e| format!("{}: {e}", gz.display()))?;
    Ok(s)
}

/// `bases`: the installations, user first. `remotes`: remote names, any
/// order, duplicates allowed (flathub in both installations is read once).
pub fn load(bases: &[PathBuf], remotes: &[String], arch: &str) -> Catalog {
    let mut c = Catalog::default();
    if arch.is_empty() || !arch.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        c.notes.push(format!("unknown architecture {arch:?}"));
        return c;
    }
    let mut seen_remote = HashSet::new();
    let mut seen_app = HashSet::new();
    for remote in remotes {
        if commands::valid_remote(remote).is_err() || !seen_remote.insert(remote.clone()) {
            continue;
        }
        let Some(dir) = bases.iter().map(|b| active(b, remote, arch)).find(|d| d.is_dir()) else {
            c.notes.push(format!("{remote}: no appstream data on disk (flatpak update fetches it)"));
            continue;
        };
        let xml = match read_xml(&dir) {
            Ok(x) => x,
            Err(e) => {
                c.notes.push(e);
                continue;
            }
        };
        for app in appstream::parse(&xml) {
            if !seen_app.insert((remote.clone(), app.id.clone(), app.branch.clone())) {
                continue;
            }
            let icon = app.icon.as_deref().and_then(|f| {
                // A file name only; never a path.
                if f.contains('/') || f.starts_with('.') {
                    return None;
                }
                ["128x128", "64x64"].iter().map(|s| dir.join("icons").join(s).join(f)).find(|p| p.is_file())
            });
            c.entries.push(Entry { app, remote: remote.clone(), icon });
        }
    }
    c.entries.sort_by(|a, b| {
        a.app.name.to_lowercase().cmp(&b.app.name.to_lowercase()).then(a.remote.cmp(&b.remote))
    });
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_icons_skips_duplicates_and_notes_missing_remotes() {
        let tmp = std::env::temp_dir().join(format!("sf-cat-{}", std::process::id()));
        let dir = tmp.join("sys/appstream/flathub/x86_64/abc");
        std::fs::create_dir_all(dir.join("icons/128x128")).unwrap();
        std::fs::write(dir.join("appstream.xml"), include_str!("../fixtures/appstream-1.14.xml")).unwrap();
        std::fs::write(dir.join("icons/128x128/org.test.Hello.png"), b"png").unwrap();
        std::os::unix::fs::symlink("abc", tmp.join("sys/appstream/flathub/x86_64/active")).unwrap();
        let bases = vec![tmp.join("user"), tmp.join("sys")];
        let remotes = vec!["flathub".to_string(), "flathub".to_string(), "sober".to_string()];
        let c = load(&bases, &remotes, "x86_64");
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].app.id, "org.test.Hello");
        assert!(c.entries[0].icon.as_ref().unwrap().ends_with("icons/128x128/org.test.Hello.png"));
        assert_eq!(c.notes.len(), 1);
        assert!(c.notes[0].starts_with("sober:"));
        assert!(load(&bases, &remotes, "../x").entries.is_empty());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn reads_the_gzip_copy_when_there_is_no_plain_one() {
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!("sf-catgz-{}", std::process::id()));
        let dir = tmp.join("appstream/flathub/x86_64/active");
        std::fs::create_dir_all(&dir).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(include_str!("../fixtures/appstream-1.14.xml").as_bytes()).unwrap();
        std::fs::write(dir.join("appstream.xml.gz"), gz.finish().unwrap()).unwrap();
        let c = load(std::slice::from_ref(&tmp), &["flathub".to_string()], "x86_64");
        assert_eq!(c.entries.len(), 1);
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
