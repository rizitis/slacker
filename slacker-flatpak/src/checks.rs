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

#[cfg(test)]
mod tests {
    use super::*;

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
