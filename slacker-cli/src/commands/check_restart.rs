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
            ui::white(&format!("{:>w_pid$}  {:<w_user$}  {:<w_cmd$}  FILES", "PID", "USER", "COMMAND"))
        );
        for (p, user) in report.procs.iter().zip(&users) {
            let files: Vec<String> = p
                .files
                .iter()
                .map(|f| {
                    let name = if f.exe {
                        "program".to_string()
                    } else {
                        f.path.rsplit('/').next().unwrap_or(&f.path).to_string()
                    };
                    if f.removed {
                        format!("{name} (removed package {})", f.package)
                    } else {
                        format!("{name} ({})", f.package)
                    }
                })
                .collect();
            println!("  {:>w_pid$}  {:<w_user$}  {:<w_cmd$}  {}", p.pid, user, p.comm, files.join(", "));
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
    use super::fit;

    #[test]
    fn long_command_lines_are_cut_with_an_ellipsis() {
        assert_eq!(fit("/usr/sbin/sshd -D", 40), "/usr/sbin/sshd -D");
        assert_eq!(fit("python3 /usr/share/app/main.py --flag", 12), "python3 /us\u{2026}");
        assert_eq!(fit("αβγδε", 3), "αβ\u{2026}");
    }
}
