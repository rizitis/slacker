//! check-restart: running programs that still use files an upgrade replaced.
//! The scan is in `stale.rs`; this prints it. Read-only, run as root.

use crate::config::Config;
use crate::{stale, term_width, ui, Outcome};
use std::io::IsTerminal;

/// `s` cut to `width` characters, with "…" when cut.
fn fit(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// How a package is named in the table: its name, and "removed" for one that
/// is no longer installed (its full name is listed once under the table).
fn package_label(f: &stale::StaleFile) -> String {
    let name = crate::pkg::PkgId::parse(&f.package).map(|id| id.name).unwrap_or_else(|| f.package.clone());
    if f.removed {
        format!("{name} removed")
    } else {
        name
    }
}

/// One process's files as packages: the program's package first, then the
/// removed packages (the process runs a version no longer on the system),
/// then every other package, each with the number of its files, most first. With `width`, the
/// list stops before it would pass it and says how many packages are left.
fn packages_summary(files: &[stale::StaleFile], width: Option<usize>) -> String {
    let program = files.iter().find(|f| f.exe).map(package_label);
    let mut counts: Vec<(String, usize, bool)> = Vec::new();
    for f in files.iter().filter(|f| !f.exe) {
        let label = package_label(f);
        match counts.iter_mut().find(|(l, _, _)| *l == label) {
            Some(c) => c.1 += 1,
            None => counts.push((label, 1, f.removed)),
        }
    }
    counts.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then_with(|| a.0.cmp(&b.0)));
    let mut parts: Vec<String> = program.map(|p| format!("program: {p}")).into_iter().collect();
    let first_lib = parts.len();
    parts.extend(counts.iter().map(|(label, n, _)| format!("{label} ({n})")));
    let join = |items: &[String]| -> String {
        let mut out = String::new();
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(if i == first_lib && first_lib > 0 { "; " } else { ", " });
            }
            out.push_str(item);
        }
        out
    };
    let whole = join(&parts);
    let Some(w) = width else { return whole };
    if whole.chars().count() <= w {
        return whole;
    }
    // Keep as many as fit together with the "… +N packages" note.
    let mut keep = parts.len();
    while keep > 1 {
        keep -= 1;
        let left = parts.len() - keep;
        let note = format!(", \u{2026} +{left} package{}", if left == 1 { "" } else { "s" });
        let head = join(&parts[..keep]);
        if head.chars().count() + note.chars().count() <= w {
            return head + &note;
        }
    }
    let left = parts.len() - 1;
    format!("{}, \u{2026} +{left} package{}", parts[0], if left == 1 { "" } else { "s" })
}

pub(crate) fn cmd_check_restart(cfg: &Config) -> Result<Outcome, String> {
    let report = stale::scan(&cfg.pkg_db_dir, &cfg.adm_dir.join("removed_packages"));
    if report.procs.is_empty() {
        println!("No running program uses a file an upgrade replaced.");
    } else {
        println!("{}", ui::blue("Running programs that still use files an upgrade replaced:"));
        let w_pid = report.procs.iter().map(|p| p.pid.to_string().len()).max().unwrap_or(3).max(3);
        let users: Vec<String> = report.procs.iter().map(|p| stale::user_name(p.uid)).collect();
        let w_user = users.iter().map(String::len).max().unwrap_or(4).max(4);
        let w_cmd = report.procs.iter().map(|p| p.comm.len()).max().unwrap_or(7).max(7);
        println!(
            "  {}",
            ui::white(&format!("{:>w_pid$}  {:<w_user$}  {:<w_cmd$}  PACKAGES (files)", "PID", "USER", "COMMAND"))
        );
        let tty = std::io::stdout().is_terminal();
        let head = 2 + w_pid + 2 + w_user + 2 + w_cmd + 2;
        for (p, user) in report.procs.iter().zip(&users) {
            // On a terminal the list is cut to the width; in a pipe or log it
            // is printed whole.
            let width = tty.then(|| term_width().saturating_sub(head).max(30));
            println!(
                "  {:>w_pid$}  {:<w_user$}  {:<w_cmd$}  {}",
                p.pid,
                user,
                p.comm,
                packages_summary(&p.files, width)
            );
            // The command line says which program it is (comm is cut at 15
            // characters and is often just the interpreter). On a terminal it
            // is cut to the width; in a pipe or log it is printed whole.
            if !p.cmdline.is_empty() {
                let indent = 2 + w_pid + 2;
                let line = if std::io::stdout().is_terminal() {
                    fit(&p.cmdline, term_width().saturating_sub(indent).max(20))
                } else {
                    p.cmdline.clone()
                };
                println!("{:indent$}{}", "", ui::dim(&line));
            }
        }
        // The removed packages by their full name, once.
        let mut removed: Vec<&str> = report
            .procs
            .iter()
            .flat_map(|p| p.files.iter())
            .filter(|f| f.removed)
            .map(|f| f.package.as_str())
            .collect();
        removed.sort();
        removed.dedup();
        if !removed.is_empty() {
            println!("\n{}", ui::white("Removed packages still in use (no longer installed):"));
            for r in &removed {
                println!("  {r}");
            }
        }
        println!(
            "\n{}",
            ui::yellow("Restart these programs (or reboot) so they use the files that are installed now.")
        );
    }
    if report.other_ns > 0 {
        println!(
            "{}",
            ui::dim(&format!(
                "{} process(es) in another mount namespace (containers) use deleted files; not checked.",
                report.other_ns
            ))
        );
    }
    println!(
        "{}",
        ui::dim("Checked: programs and shared libraries. Scripts (python, perl, shell) are not seen.")
    );
    Ok(if report.procs.is_empty() { Outcome::Ok } else { Outcome::Pending })
}

#[cfg(test)]
mod tests {
    use super::{fit, packages_summary};
    use crate::stale::StaleFile;

    fn file(path: &str, package: &str, removed: bool, exe: bool) -> StaleFile {
        StaleFile { path: path.into(), package: package.into(), removed, exe }
    }

    #[test]
    fn files_are_counted_by_package() {
        let files = vec![
            file("/usr/lib64/libKF6Kio.so.6", "kio-6.32.0-x86_64-2", false, false),
            file("/usr/bin/dolphin", "dolphin-26.08.3-x86_64-2", false, true),
            file("/usr/lib64/libQt6Core.so.6.11.3", "qt6-6.11.3_20261021_3bc7aa89-x86_64-2", true, false),
            file("/usr/lib64/libQt6Gui.so.6.11.3", "qt6-6.11.3_20261021_3bc7aa89-x86_64-2", true, false),
            file("/usr/lib64/libKF6KIOGui.so.6", "kio-6.32.0-x86_64-2", false, false),
            file("/usr/lib64/libKF6Solid.so.6", "solid-6.32.0-x86_64-2", false, false),
        ];
        // removed first, then by number of files
        assert_eq!(packages_summary(&files, None), "program: dolphin; qt6 removed (2), kio (2), solid (1)");
        // cut to the width, saying how many are left
        let cut = packages_summary(&files, Some(50));
        assert_eq!(cut, "program: dolphin; qt6 removed (2), \u{2026} +2 packages");
        assert!(cut.chars().count() <= 50);
        let narrow = packages_summary(&files, Some(30));
        assert_eq!(narrow, "program: dolphin, \u{2026} +3 packages");
        // a library only, no program
        assert_eq!(packages_summary(&files[..1], None), "kio (1)");
    }

    #[test]
    fn long_command_lines_are_cut_with_an_ellipsis() {
        assert_eq!(fit("/usr/sbin/sshd -D", 40), "/usr/sbin/sshd -D");
        assert_eq!(fit("python3 /usr/share/app/main.py --flag", 12), "python3 /us\u{2026}");
        assert_eq!(fit("αβγδε", 3), "αβ\u{2026}");
    }
}
