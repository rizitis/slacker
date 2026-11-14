//! Reading what flatpak prints during update and uninstall, on the pty.

/// A question flatpak is waiting on, read from the unfinished last line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// "... [Y/n]: " (Enter means yes) or "... [y/n]: " (no default).
    YesNo { question: String, default_yes: bool },
    /// Any other "...]: " or "...?: " line: a number choice or something new.
    /// The window cannot answer it; the user can only stop.
    Other(String),
}

/// The question in `partial`, the text after the last newline, if flatpak is
/// waiting on one.
pub fn prompt(partial: &str) -> Option<Prompt> {
    let p = partial.trim_start_matches('\r');
    if let Some(q) = p.strip_suffix(" [Y/n]: ") {
        return Some(Prompt::YesNo { question: q.trim().to_string(), default_yes: true });
    }
    if let Some(q) = p.strip_suffix(" [y/n]: ") {
        return Some(Prompt::YesNo { question: q.trim().to_string(), default_yes: false });
    }
    if p.ends_with("]: ") || p.ends_with("?: ") {
        return Some(Prompt::Other(p.trim().to_string()));
    }
    None
}

/// The "New ID permissions:" (update) and "ID permissions:" (install) blocks in a transcript, each as its heading and
/// the lines under it, up to the first blank line. An update that gives an
/// application new permissions says so here, and only here.
pub fn new_permissions(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    while let Some(l) = lines.next() {
        let t = l.trim();
        // "New ID permissions:" on update, "ID permissions:" on install.
        let id = t.strip_suffix(" permissions:").map(|x| x.strip_prefix("New ").unwrap_or(x));
        if id.is_some_and(|id| !id.is_empty() && !id.contains(' ')) {
            let body: Vec<String> = lines
                .by_ref()
                .map(str::trim)
                .take_while(|x| !x.is_empty())
                // flatpak puts several permissions on one line, tab-separated.
                .map(|x| x.split('\t').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", "))
                .collect();
            out.push((t.trim_end_matches(':').to_string(), body));
        }
    }
    out
}

/// Lines that report an error. flatpak can print them and still exit 0
/// (seen on Slackware: an appstream error for one remote after a good update).
pub fn error_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| {
            let low = l.to_ascii_lowercase();
            low.starts_with("error:") || low.starts_with("error ") || low.contains(" error:")
        })
        .map(String::from)
        .collect()
}

/// A short explanation for a known flatpak failure, if there is one. The
/// original message stays in the output; this only adds where to look.
pub fn hint(text: &str) -> Option<&'static str> {
    if text.contains("not allowed for user") {
        return Some(
            "polkit did not allow this change to the system-wide Flatpak installation. Usual reasons: no polkit \
             agent is running in this session, the session is not active and local for elogind \
             (see `loginctl show-session`), or the polkit rules need a group (flatpak's own rules \
             use wheel).",
        );
    }
    None
}

/// Turns pty output into text for display: CR-LF becomes LF, and a bare CR
/// (a progress line redrawn in place) starts the line over.
pub fn clean(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut line = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => line.clear(),
            '\n' => {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            '\u{1b}' => {
                // Drop terminal escape sequences, should any appear.
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for n in chars.by_ref() {
                        if n.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            }
            _ => line.push(c),
        }
    }
    out.push_str(&line);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output of `flatpak update --user` on a pty, with
    /// FLATPAK_FANCY_OUTPUT=0 (flatpak 1.14.6, test repo).
    const PTY: &str = "Looking for updates\u{2026}\r\n\r\nNew org.test.Hello permissions:\r\n    devices\r\n\r\n\r\n\r\n 1.\t   \torg.test.Hello\tstable\tu\ttestrepo\t< 212 bytes\r\n\r\nProceed with these changes to the user installation? [Y/n]: ";

    #[test]
    fn the_yes_no_question_is_found() {
        let text = clean(PTY);
        let partial = text.rsplit('\n').next().unwrap();
        assert_eq!(
            prompt(partial),
            Some(Prompt::YesNo {
                question: "Proceed with these changes to the user installation?".into(),
                default_yes: true
            })
        );
        assert_eq!(
            prompt("Do you want to install it? [y/n]: "),
            Some(Prompt::YesNo { question: "Do you want to install it?".into(), default_yes: false })
        );
        assert!(matches!(prompt("Which do you want to use (0 to abort)? [0-2]: "), Some(Prompt::Other(_))));
        assert_eq!(prompt("Updating\u{2026}  40%"), None);
        assert_eq!(prompt(""), None);
    }

    #[test]
    fn new_permissions_are_found() {
        let p = new_permissions(&clean(PTY));
        assert_eq!(p, vec![("New org.test.Hello permissions".to_string(), vec!["devices".to_string()])]);
        // As seen on Slackware for Sober.
        let sober = "New org.vinegarhq.Sober permissions:\n    tags [1]\n\n    [1] proprietary\n";
        assert_eq!(new_permissions(sober)[0].1, vec!["tags [1]"]);
        // On install flatpak says it without "New".
        let inst = "org.test.Hello permissions:\n    network\tx11\tdevices\tfile access [1]\n\n    [1] home\n";
        assert_eq!(
            new_permissions(inst),
            vec![("org.test.Hello permissions".to_string(), vec!["network, x11, devices, file access [1]".to_string()])]
        );
        assert!(new_permissions("Some other permissions: here\n").is_empty());
        // Several on one line, as flatpak 1.14 printed them.
        let two = "New org.test.Hello permissions:\n    x11\tdevices\n\n";
        assert_eq!(new_permissions(two)[0].1, vec!["x11, devices"]);
    }

    #[test]
    fn errors_are_found_even_after_success() {
        let text = "Changes complete.\nUpdating appstream data for user remote sober\nError updating: Error updating appstream2: No such ref 'appstream2/x86_64' in remote sober\nerror: There were one or more errors\n";
        let e = error_lines(text);
        assert_eq!(e.len(), 2);
        assert!(e[0].starts_with("Error updating:"));
        assert!(error_lines("Updates complete.\n").is_empty());
    }

    #[test]
    fn a_polkit_refusal_gets_a_hint() {
        assert!(hint("Error: Flatpak system operation Deploy not allowed for user\n").is_some());
        assert!(hint("Updates complete.\n").is_none());
    }

    #[test]
    fn progress_redraws_collapse() {
        assert_eq!(clean("Updating\u{2026}  0%\rUpdating\u{2026} 100%\r\nDone\r\n"), "Updating\u{2026} 100%\nDone\n");
        assert_eq!(clean("a\u{1b}[2Kb\n"), "ab\n");
    }
}
