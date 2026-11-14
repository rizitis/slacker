//! Every flatpak command slacker-flatpak runs, in one place. The ones that
//! change an installation (update, uninstall) never get -y or
//! --noninteractive: flatpak asks, and the user answers in the window.

/// Which installation a command looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Inst {
    System,
    User,
}

impl Inst {
    pub fn flag(self) -> &'static str {
        match self {
            Inst::System => "--system",
            Inst::User => "--user",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Inst::System => "system",
            Inst::User => "user",
        }
    }

    /// How the window names the installation. "system" alone could be read
    /// as the Slackware system; these say it is Flatpak's.
    pub fn words(self) -> &'static str {
        match self {
            Inst::System => "system-wide Flatpak",
            Inst::User => "your user's Flatpak",
        }
    }

    pub fn from_label(s: &str) -> Option<Inst> {
        match s {
            "system" => Some(Inst::System),
            "user" => Some(Inst::User),
            _ => None,
        }
    }
}

/// The columns asked of `flatpak list`, in this order. parse::list reads them
/// back in the same order.
pub const LIST_COLUMNS: &str = "application,name,version,branch,arch,installation,origin,size,active,runtime";

/// The columns asked of `flatpak remotes`.
pub const REMOTE_COLUMNS: &str = "name,title,url,priority,options";

pub fn version() -> Vec<String> {
    args(&["--version"])
}

pub fn installations() -> Vec<String> {
    args(&["--installations"])
}

pub fn list_apps() -> Vec<String> {
    args(&["list", "--app", &format!("--columns={LIST_COLUMNS}")])
}

pub fn list_runtimes() -> Vec<String> {
    args(&["list", "--runtime", &format!("--columns={LIST_COLUMNS}")])
}

pub fn remotes() -> Vec<String> {
    args(&["remotes", &format!("--columns={REMOTE_COLUMNS}")])
}

/// `flatpak info --show-permissions --user|--system ID//BRANCH`.
pub fn permissions(inst: Inst, id: &str, branch: &str) -> Result<Vec<String>, String> {
    Ok(args(&["info", "--show-permissions", inst.flag(), &app_ref(id, branch)?]))
}

/// `flatpak info --user|--system ID//BRANCH`.
pub fn info(inst: Inst, id: &str, branch: &str) -> Result<Vec<String>, String> {
    Ok(args(&["info", inst.flag(), &app_ref(id, branch)?]))
}

/// The columns asked of `flatpak remote-ls --updates`.
pub const UPDATE_COLUMNS: &str = "application,name,version,branch,arch,origin,download-size";

/// `flatpak remote-ls --updates --user|--system REMOTE`. One remote at a
/// time: a remote that cannot be reached then hides only its own updates.
pub fn check_updates(inst: Inst, remote: &str) -> Result<Vec<String>, String> {
    valid_remote(remote)?;
    Ok(args(&["remote-ls", "--updates", inst.flag(), &format!("--columns={UPDATE_COLUMNS}"), remote]))
}

/// `flatpak update --user|--system`: everything in one installation.
pub fn update(inst: Inst) -> Vec<String> {
    args(&["update", inst.flag()])
}

/// `flatpak uninstall --user|--system ID//BRANCH`. The application's data in
/// ~/.var/app stays (no --delete-data).
pub fn uninstall(inst: Inst, id: &str, branch: &str) -> Result<Vec<String>, String> {
    Ok(args(&["uninstall", inst.flag(), &app_ref(id, branch)?]))
}

/// `flatpak uninstall --unused --user|--system`.
pub fn uninstall_unused(inst: Inst) -> Vec<String> {
    args(&["uninstall", "--unused", inst.flag()])
}

/// The columns asked of `flatpak search` (the same as --columns=all on 1.18).
pub const SEARCH_COLUMNS: &str = "name,description,application,version,branch,remotes";

/// `flatpak --default-arch`, for finding the appstream icons.
pub fn default_arch() -> Vec<String> {
    args(&["--default-arch"])
}

/// `flatpak search TEXT`: looks in the appstream data flatpak already has,
/// no network.
pub fn search(text: &str) -> Result<Vec<String>, String> {
    valid_search(text)?;
    Ok(args(&["search", &format!("--columns={SEARCH_COLUMNS}"), text]))
}

/// `flatpak remote-info --user|--system --show-metadata REMOTE ID//BRANCH`:
/// what an application asks for, before it is installed.
pub fn remote_info(inst: Inst, remote: &str, id: &str, branch: &str) -> Result<Vec<String>, String> {
    valid_remote(remote)?;
    Ok(args(&["remote-info", inst.flag(), "--show-metadata", remote, &app_ref(id, branch)?]))
}

/// `flatpak install --user|--system REMOTE ID//BRANCH`. flatpak lists what
/// it will install (runtimes too) and asks.
pub fn install(inst: Inst, remote: &str, id: &str, branch: &str) -> Result<Vec<String>, String> {
    valid_remote(remote)?;
    Ok(args(&["install", inst.flag(), remote, &app_ref(id, branch)?]))
}

/// Search text: 1 to 100 characters, no control characters, not starting
/// with '-' (it would be read as an option).
pub fn valid_search(t: &str) -> Result<(), String> {
    let n = t.chars().count();
    if n == 0 || n > 100 || t.starts_with('-') || t.chars().any(char::is_control) {
        return Err("search text: 1 to 100 characters, not starting with '-'".to_string());
    }
    Ok(())
}

/// `flatpak repair --user|--system`, shown after a transaction was stopped.
pub fn repair(inst: Inst) -> Vec<String> {
    args(&["repair", inst.flag()])
}

/// The command line as it would be typed, for display.
pub fn display(argv: &[String]) -> String {
    let mut s = String::from("flatpak");
    for a in argv {
        s.push(' ');
        s.push_str(a);
    }
    s
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// `ID//BRANCH`, after checking both. The values come from flatpak's own
/// output, but they still pass the same check before going back to it.
fn app_ref(id: &str, branch: &str) -> Result<String, String> {
    valid_id(id)?;
    valid_branch(branch)?;
    Ok(format!("{id}//{branch}"))
}

/// A flatpak application ID: at least three dot-separated parts of
/// [A-Za-z0-9_-], none empty, the first character not a digit or '-'.
pub fn valid_id(id: &str) -> Result<(), String> {
    let bad = || Err(format!("not a flatpak application ID: {id:?}"));
    if id.is_empty() || id.len() > 255 || id.starts_with('-') {
        return bad();
    }
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() < 3 {
        return bad();
    }
    for p in parts {
        let Some(first) = p.chars().next() else { return bad() };
        if first.is_ascii_digit() || first == '-' {
            return bad();
        }
        if !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return bad();
        }
    }
    Ok(())
}

/// A remote name: [A-Za-z0-9_.-], not empty, not starting with '-' or '.'.
pub fn valid_remote(r: &str) -> Result<(), String> {
    if r.is_empty()
        || r.starts_with('-')
        || r.starts_with('.')
        || !r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return Err(format!("not a flatpak remote name: {r:?}"));
    }
    Ok(())
}

/// A branch: [A-Za-z0-9_.-], not empty, not starting with '-'.
pub fn valid_branch(b: &str) -> Result<(), String> {
    if b.is_empty()
        || b.starts_with('-')
        || !b.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return Err(format!("not a flatpak branch: {b:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_read_only_and_exact() {
        assert_eq!(display(&list_apps()), format!("flatpak list --app --columns={LIST_COLUMNS}"));
        assert_eq!(display(&remotes()), format!("flatpak remotes --columns={REMOTE_COLUMNS}"));
        assert_eq!(
            permissions(Inst::User, "org.vinegarhq.Sober", "stable").unwrap(),
            vec!["info", "--show-permissions", "--user", "org.vinegarhq.Sober//stable"]
        );
        assert_eq!(
            info(Inst::System, "org.gnome.Platform", "51").unwrap(),
            vec!["info", "--system", "org.gnome.Platform//51"]
        );
    }

    #[test]
    fn ids_and_branches_are_checked() {
        for ok in ["org.vinegarhq.Sober", "com.obsproject.Studio", "org.freedesktop.Platform.GL.nvidia-595-99-02", "dev.zed.Zed-Preview"] {
            assert!(valid_id(ok).is_ok(), "{ok}");
        }
        for bad in ["", "sober", "org.sober", "-org.a.b", "org..a", "org.a.b c", "org.a.b;rm", "org.1a.b", "org.a.b/x", "--user"] {
            assert!(valid_id(bad).is_err(), "{bad}");
        }
        for ok in ["stable", "24.08", "26.08-extra", "24.08extra", "master", "3.22"] {
            assert!(valid_branch(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "a b", "a/b", "a//b"] {
            assert!(valid_branch(bad).is_err(), "{bad}");
        }
        assert!(permissions(Inst::User, "--system", "stable").is_err());
        for ok in ["flathub", "sober", "breezydesktop-origin", "flathub-beta"] {
            assert!(valid_remote(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "--system", ".hidden", "a b", "a/b"] {
            assert!(valid_remote(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn search_and_install_commands() {
        assert_eq!(
            search("sober").unwrap(),
            vec!["search", &format!("--columns={SEARCH_COLUMNS}"), "sober"]
        );
        for bad in ["", "-y", "--system", "a\nb", &"x".repeat(101)] {
            assert!(search(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            display(&install(Inst::User, "flathub", "org.gnome.baobab", "stable").unwrap()),
            "flatpak install --user flathub org.gnome.baobab//stable"
        );
        assert_eq!(
            display(&remote_info(Inst::System, "flathub", "org.gnome.baobab", "stable").unwrap()),
            "flatpak remote-info --system --show-metadata flathub org.gnome.baobab//stable"
        );
        assert!(install(Inst::User, "-x", "org.gnome.baobab", "stable").is_err());
        assert!(install(Inst::User, "flathub", "baobab", "stable").is_err());
    }

    #[test]
    fn changes_never_answer_for_the_user() {
        let all = [
            install(Inst::User, "flathub", "org.gnome.baobab", "stable").unwrap(),
            update(Inst::User),
            update(Inst::System),
            uninstall(Inst::User, "org.vinegarhq.Sober", "stable").unwrap(),
            uninstall_unused(Inst::System),
        ];
        for a in &all {
            for forbidden in ["-y", "--assumeyes", "--noninteractive", "--delete-data", "--no-gpg-verify"] {
                assert!(!a.iter().any(|x| x == forbidden), "{forbidden} in {a:?}");
            }
        }
        assert_eq!(display(&update(Inst::System)), "flatpak update --system");
        assert_eq!(
            display(&uninstall(Inst::User, "org.vinegarhq.Sober", "stable").unwrap()),
            "flatpak uninstall --user org.vinegarhq.Sober//stable"
        );
        assert_eq!(
            check_updates(Inst::User, "flathub").unwrap(),
            vec!["remote-ls", "--updates", "--user", &format!("--columns={UPDATE_COLUMNS}"), "flathub"]
        );
        assert!(check_updates(Inst::User, "--system").is_err());
    }
}
