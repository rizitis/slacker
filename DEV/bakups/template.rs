//! Template support: snapshot the installed package set to a named list and
//! reproduce it elsewhere. Templates live in `<config_dir>/templates/`.
//!
//! A template is a plain list of package names, one per line, with optional
//! `include OTHER` lines pulling in another template.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn templates_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("templates")
}

fn template_path(config_dir: &Path, name: &str) -> PathBuf {
    // Accept both "mysystem" and "mysystem.template" — strip a redundant
    // extension so we never build "mysystem.template.template".
    let stem = name.strip_suffix(".template").unwrap_or(name);
    templates_dir(config_dir).join(format!("{stem}.template"))
}

/// Write a template listing the given package names.
pub fn generate(config_dir: &Path, name: &str, pkg_names: &[String]) -> Result<PathBuf, String> {
    let dir = templates_dir(config_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let path = template_path(config_dir, name);
    let mut body = String::new();
    body.push_str(&format!("# slacker template '{name}'\n"));
    for n in pkg_names {
        body.push_str(n);
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Delete a template file. Does not touch installed packages. Returns the
/// path that was removed.
pub fn delete(config_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = template_path(config_dir, name);
    if !path.exists() {
        let stem = name.strip_suffix(".template").unwrap_or(name);
        return Err(format!(
            "template '{stem}' not found in {}",
            templates_dir(config_dir).display()
        ));
    }
    std::fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    Ok(path)
}

/// Resolve a template (following includes) into a de-duplicated name list.
pub fn load(config_dir: &Path, name: &str, follow_includes: bool) -> Result<Vec<String>, String> {
    let mut seen_templates = HashSet::new();
    let mut out = Vec::new();
    let mut out_set = HashSet::new();
    load_into(config_dir, name, follow_includes, &mut seen_templates, &mut out, &mut out_set)?;
    Ok(out)
}

fn load_into(
    config_dir: &Path,
    name: &str,
    follow_includes: bool,
    seen_templates: &mut HashSet<String>,
    out: &mut Vec<String>,
    out_set: &mut HashSet<String>,
) -> Result<(), String> {
    if !seen_templates.insert(name.to_string()) {
        return Ok(()); // already processed; avoid include cycles
    }
    let path = template_path(config_dir, name);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            let stem = name.strip_suffix(".template").unwrap_or(name);
            format!(
                "template '{stem}' not found in {}; create it with `slacker generate-template {stem}`",
                templates_dir(config_dir).display()
            )
        } else {
            format!("cannot read template {}: {e}", path.display())
        }
    })?;

    for raw in text.lines() {
        let line = match raw.find('#') {
            Some(i) => &raw[..i],
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        if let Some(inc) = line.strip_prefix("include ") {
            if follow_includes {
                load_into(config_dir, inc.trim(), true, seen_templates, out, out_set)?;
            }
            continue;
        }
        if out_set.insert(line.to_string()) {
            out.push(line.to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_removes_file_only() {
        let dir = std::env::temp_dir().join("slacker_del_test");
        let _ = std::fs::create_dir_all(super::templates_dir(&dir));
        super::generate(&dir, "td", &["pkga".into()]).unwrap();
        assert!(super::template_path(&dir, "td").exists());
        super::delete(&dir, "td").unwrap();
        assert!(!super::template_path(&dir, "td").exists());
        // deleting a missing template errors
        assert!(super::delete(&dir, "td").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_suffix_is_normalized() {
        let dir = std::env::temp_dir().join("slacker_tmpl_test");
        let _ = std::fs::create_dir_all(super::templates_dir(&dir));
        super::generate(&dir, "sys", &["pkga".into(), "pkgb".into()]).unwrap();
        // both "sys" and "sys.template" must load the same file
        let a = super::load(&dir, "sys", false).unwrap();
        let b = super::load(&dir, "sys.template", false).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, vec!["pkga", "pkgb"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_then_load_with_include() {
        let dir = std::env::temp_dir().join("slacker_tpl_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        generate(&dir, "base", &["bash".into(), "vim".into()]).unwrap();
        // a template that includes base plus one extra
        std::fs::write(
            templates_dir(&dir).join("desktop.template"),
            "include base\nfirefox\nbash\n",
        )
        .unwrap();

        let names = load(&dir, "desktop", true).unwrap();
        assert_eq!(names, vec!["bash", "vim", "firefox"]); // deduped, include first

        let no_inc = load(&dir, "desktop", false).unwrap();
        assert_eq!(no_inc, vec!["firefox", "bash"]);
    }
}
