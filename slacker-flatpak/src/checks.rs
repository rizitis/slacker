//! Checks made before and around running flatpak. They only look; nothing is
//! changed.

use std::path::{Path, PathBuf};

/// The D-Bus service file of flatpak's system helper. Its Exec= line names
/// the helper itself, wherever the packager put it.
pub const HELPER_SERVICE: &str = "/usr/share/dbus-1/system-services/org.freedesktop.Flatpak.SystemHelper.service";

/// `flatpak` on PATH, else /usr/bin/flatpak if it is there.
pub fn flatpak_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join("flatpak"))
        .chain(std::iter::once(PathBuf::from("/usr/bin/flatpak")))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// What is known about the system helper.
#[derive(Debug, PartialEq, Eq)]
pub enum Helper {
    /// The service file is there and the program it names exists.
    Present(PathBuf),
    /// No service file: flatpak cannot ask for privileges, so the system
    /// installation can only be changed as root.
    NoService,
    /// The service file names a program that is not there.
    Missing(PathBuf),
}

pub fn helper() -> Helper {
    match std::fs::read_to_string(HELPER_SERVICE) {
        Err(_) => Helper::NoService,
        Ok(text) => match helper_exec(&text) {
            None => Helper::NoService,
            Some(p) if is_executable(&p) => Helper::Present(p),
            Some(p) => Helper::Missing(p),
        },
    }
}

/// The program on the service file's `Exec=` line.
fn helper_exec(service: &str) -> Option<PathBuf> {
    service
        .lines()
        .find_map(|l| l.trim().strip_prefix("Exec="))
        .and_then(|e| e.split_whitespace().next())
        .map(PathBuf::from)
}

/// The `exports/share` directories of the user installation and of every
/// system installation that are not on XDG_DATA_DIRS. Applications from those
/// do not show in the desktop's menus. flatpak's /etc/profile.d/flatpak.sh
/// adds them, but only login shells read it.
pub fn missing_exports(xdg_data_dirs: &str, user_data_home: &Path, system_installs: &[String]) -> Vec<PathBuf> {
    let on_path: Vec<PathBuf> = xdg_data_dirs
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(s.trim_end_matches('/')))
        .collect();
    let mut want = vec![user_data_home.join("flatpak/exports/share")];
    want.extend(system_installs.iter().map(|i| Path::new(i).join("exports/share")));
    want.into_iter().filter(|w| !on_path.contains(w)).collect()
}

/// $XDG_DATA_HOME, or ~/.local/share.
pub fn user_data_home() -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"),
    }
}

/// The icon flatpak keeps for an application in a remote's appstream data:
/// BASE/appstream/REMOTE/ARCH/active/icons/{128x128,64x64}/ID.png, in the
/// first installation that has it. These come with the appstream update
/// flatpak already does; nothing is downloaded for them.
pub fn appstream_icon(bases: &[PathBuf], remote: &str, arch: &str, id: &str) -> Option<PathBuf> {
    // The parts come from flatpak's output; still, none may leave the tree.
    if crate::commands::valid_remote(remote).is_err()
        || crate::commands::valid_id(id).is_err()
        || arch.is_empty()
        || !arch.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    for base in bases {
        for size in ["128x128", "64x64"] {
            let p = base
                .join("appstream")
                .join(remote)
                .join(arch)
                .join("active/icons")
                .join(size)
                .join(format!("{id}.png"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The icon an installed application exports, in BASE/exports/share/icons
/// (what the desktop menus use too). PNG sizes first, then the scalable SVG.
pub fn installed_icon(base: &Path, id: &str) -> Option<PathBuf> {
    if crate::commands::valid_id(id).is_err() {
        return None;
    }
    let hicolor = base.join("exports/share/icons/hicolor");
    ["128x128", "96x96", "64x64", "256x256", "48x48"]
        .iter()
        .map(|s| hicolor.join(s).join("apps").join(format!("{id}.png")))
        .chain(std::iter::once(hicolor.join("scalable/apps").join(format!("{id}.svg"))))
        .find(|p| p.is_file())
}

/// Folders under `data_root` (~/.var/app) named like an application ID that
/// is not installed in any installation: data an uninstall kept. Only real
/// directories; a symlink is never listed.
pub fn leftover_data(data_root: &Path, installed: &std::collections::HashSet<String>) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(data_root) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let path = e.path();
            let meta = std::fs::symlink_metadata(&path).ok()?;
            (meta.is_dir() && crate::commands::valid_id(&name).is_ok() && !installed.contains(&name))
                .then_some((name, path))
        })
        .collect();
    out.sort();
    out
}

/// Bytes under `path`, symlinks counted as themselves and never followed.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path).map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

/// Deletes one leftover folder, after checking again that it is exactly
/// `data_root/ID`, a real directory, and not an installed application's.
pub fn delete_leftover(data_root: &Path, id: &str, installed: &std::collections::HashSet<String>) -> Result<(), String> {
    crate::commands::valid_id(id)?;
    if installed.contains(id) {
        return Err(format!("{id} is installed; its data is not deleted"));
    }
    let path = data_root.join(id);
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    // remove_dir_all does not follow symlinks inside the folder.
    std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftover_data_is_only_real_folders_of_apps_not_installed() {
        let tmp = std::env::temp_dir().join(format!("sf-left-{}", std::process::id()));
        let root = tmp.join(".var/app");
        for d in ["org.gnome.baobab", "com.onepassword.OnePassword", "not-an-id"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("com.onepassword.OnePassword/x"), b"12345").unwrap();
        // A symlink named like an app, pointing outside: never listed, never followed.
        let outside = tmp.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"data").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("org.evil.Link")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("com.onepassword.OnePassword/link")).unwrap();
        let installed: std::collections::HashSet<String> = ["org.gnome.baobab".to_string()].into();
        let left = leftover_data(&root, &installed);
        assert_eq!(left.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec!["com.onepassword.OnePassword"]);
        assert!(dir_size(&root.join("com.onepassword.OnePassword")) >= 5);
        assert!(delete_leftover(&root, "org.gnome.baobab", &installed).is_err());
        assert!(delete_leftover(&root, "org.evil.Link", &installed).is_err());
        assert!(delete_leftover(&root, "../outside", &installed).is_err());
        delete_leftover(&root, "com.onepassword.OnePassword", &installed).unwrap();
        assert!(!root.join("com.onepassword.OnePassword").exists());
        // The link inside was removed, not what it pointed to.
        assert!(outside.join("keep").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn installed_icons_prefer_png_then_svg() {
        let tmp = std::env::temp_dir().join(format!("sf-inst-{}", std::process::id()));
        let svg = tmp.join("exports/share/icons/hicolor/scalable/apps");
        std::fs::create_dir_all(&svg).unwrap();
        std::fs::write(svg.join("org.gnome.baobab.svg"), b"<svg/>").unwrap();
        assert!(installed_icon(&tmp, "org.gnome.baobab").unwrap().ends_with("scalable/apps/org.gnome.baobab.svg"));
        let png = tmp.join("exports/share/icons/hicolor/64x64/apps");
        std::fs::create_dir_all(&png).unwrap();
        std::fs::write(png.join("org.gnome.baobab.png"), b"png").unwrap();
        assert!(installed_icon(&tmp, "org.gnome.baobab").unwrap().ends_with("64x64/apps/org.gnome.baobab.png"));
        assert_eq!(installed_icon(&tmp, "../../etc"), None);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn appstream_icons_are_found_and_paths_stay_inside() {
        let tmp = std::env::temp_dir().join(format!("sf-icons-{}", std::process::id()));
        let dir = tmp.join("sys/appstream/flathub/x86_64/c871/icons/64x64");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("org.gnome.baobab.png"), b"png").unwrap();
        std::os::unix::fs::symlink("c871", tmp.join("sys/appstream/flathub/x86_64/active")).unwrap();
        let bases = vec![tmp.join("user"), tmp.join("sys")];
        let found = appstream_icon(&bases, "flathub", "x86_64", "org.gnome.baobab").unwrap();
        assert!(found.ends_with("active/icons/64x64/org.gnome.baobab.png"));
        assert_eq!(appstream_icon(&bases, "flathub", "x86_64", "org.gnome.Nothing"), None);
        assert_eq!(appstream_icon(&bases, "../x", "x86_64", "org.gnome.baobab"), None);
        assert_eq!(appstream_icon(&bases, "flathub", "../..", "org.gnome.baobab"), None);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn the_helper_is_found_through_its_service_file() {
        let svc = "[D-BUS Service]\nName=org.freedesktop.Flatpak.SystemHelper\nExec=/usr/libexec/flatpak-system-helper\nUser=root\n";
        assert_eq!(helper_exec(svc), Some(PathBuf::from("/usr/libexec/flatpak-system-helper")));
        assert_eq!(helper_exec("[D-BUS Service]\n"), None);
    }

    #[test]
    fn exports_missing_from_xdg_data_dirs_are_reported() {
        let home = Path::new("/home/omen/.local/share");
        let sys = vec!["/var/lib/flatpak".to_string()];
        // What r-tz's session had: neither directory.
        let m = missing_exports("/usr/local/share:/usr/share", home, &sys);
        assert_eq!(
            m,
            vec![PathBuf::from("/home/omen/.local/share/flatpak/exports/share"), PathBuf::from("/var/lib/flatpak/exports/share")]
        );
        // After /etc/profile.d/flatpak.sh: both, one with a trailing slash.
        let ok = "/home/omen/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share/:/usr/share";
        assert!(missing_exports(ok, home, &sys).is_empty());
    }
}
