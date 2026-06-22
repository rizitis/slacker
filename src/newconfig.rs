//! `new-config`: locate leftover `*.new` configuration files so the user can
//! merge them. slackpkg scans /etc and /usr/share/vim; the actual keep/
//! overwrite/diff decisions are driven interactively by the caller.

use std::path::{Path, PathBuf};

/// A discovered .new file alongside the original it would replace.
pub struct NewConfig {
    pub new_file: PathBuf,
    pub target: PathBuf,
}

/// Default roots slackpkg searches.
pub fn default_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/etc"), PathBuf::from("/usr/share/vim")]
}

/// Recursively find `*.new` files under the given roots.
pub fn find_new_configs(roots: &[PathBuf]) -> Vec<NewConfig> {
    let mut out = Vec::new();
    for root in roots {
        walk(root, &mut out);
    }
    out.sort_by(|a, b| a.new_file.cmp(&b.new_file));
    out
}

fn walk(dir: &Path, out: &mut Vec<NewConfig>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("new") {
            let target = strip_new_suffix(&path);
            out.push(NewConfig { new_file: path, target });
        }
    }
}

/// Map `/etc/foo.conf.new` -> `/etc/foo.conf`.
fn strip_new_suffix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(s.strip_suffix(".new").unwrap_or(&s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_new_files_recursively() {
        let root = std::env::temp_dir().join("slacker_newcfg_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.conf.new"), "x").unwrap();
        std::fs::write(root.join("sub/b.conf.new"), "y").unwrap();
        std::fs::write(root.join("normal.conf"), "z").unwrap();

        let found = find_new_configs(&[root.clone()]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].target, root.join("a.conf"));
    }
}
