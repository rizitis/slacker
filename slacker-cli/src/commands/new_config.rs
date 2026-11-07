//! new-config: review the *.new configuration files.

use crate::cli::Cli;
use crate::config::NewConfigRules;
use crate::{confirm, Outcome};
use crate::{config, newconfig, ui};
use std::io::Write;

/// Byte-for-byte comparison. A missing/unreadable file counts as "not
/// identical" so the caller falls through to the interactive path.
fn files_identical(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Show a coloured unified diff between the installed file and the .new one,
/// shelling out to the system `diff` (as slacker already does for bzip2/gpg).
fn show_config_diff(old: &std::path::Path, new: &std::path::Path) {
    match std::process::Command::new("diff").arg("-u").arg(old).arg(new).output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().is_empty() {
                println!("    {}", ui::dim("(no differences)"));
                return;
            }
            for line in text.lines() {
                let painted = if line.starts_with("+++") || line.starts_with("---") {
                    ui::dim(line)
                } else if line.starts_with("@@") {
                    ui::cyan(line)
                } else if line.starts_with('+') {
                    ui::green(line)
                } else if line.starts_with('-') {
                    ui::red(line)
                } else {
                    line.to_string()
                };
                println!("    {painted}");
            }
        }
        Err(_) => println!("    {}", ui::dim("(system `diff` not available)")),
    }
}

/// Open an external merge tool on (installed, .new). Honours $SLACKER_MERGE,
/// defaulting to vimdiff. Ok only if the tool ran successfully.
fn merge_config(old: &std::path::Path, new: &std::path::Path) -> Result<(), String> {
    let tool = std::env::var("SLACKER_MERGE").unwrap_or_else(|_| "vimdiff".to_string());
    match std::process::Command::new(&tool).arg(old).arg(new).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!("'{tool}' exited with an error")),
        Err(_) => Err(format!("merge tool '{tool}' not found (set $SLACKER_MERGE)")),
    }
}

/// Build an interactive choice prompt whose bracketed KEY letters are WHITE (so
/// they stand out) and whose surrounding text is blue. Each entry is
/// `(KEY, text-after-the-bracket)`; `default_key` is shown at the end as `? [X]`.
/// Centralised so every letter-choice prompt highlights its keys consistently.
fn choice_line(prefix: &str, entries: &[(&str, &str)], default_key: &str) -> String {
    let mut s = String::new();
    if !prefix.is_empty() {
        s.push_str(&ui::blue(prefix));
        s.push(' ');
    }
    for (i, (key, after)) in entries.iter().enumerate() {
        if i > 0 {
            s.push_str("  ");
        }
        s.push_str(&ui::white(&format!("[{key}]")));
        if !after.is_empty() {
            s.push_str(&ui::blue(after));
        }
    }
    s.push_str(&ui::blue(" ? "));
    s.push_str(&ui::white(&format!("[{default_key}]")));
    s
}

/// Config files that are NEVER replaced wholesale by a `.new`, whatever the user
/// answers, each with the reason it is exempt.
///
/// They hold settings unique to this machine, and the `etc` and
/// `network-scripts` packages ship a `.new` for each on every upgrade — so they
/// turn up in ordinary `-current` updates, not only in a dist-upgrade.
/// Patrick's own 15.0→16.0 script skips the first five. `rc.local_shutdown` is
/// slacker's addition: it is the shutdown-side twin of `rc.local` (run by
/// `rc.6`), holds the same kind of user-written commands, and deserves the same
/// protection whether or not a package ever ships a `.new` for it.
///
/// The reason is carried per file rather than described in one sentence,
/// because these are NOT one kind of thing: three are the account database,
/// one is the network bring-up, and `rc.local` / `rc.local_shutdown` are the
/// user's own start-up and shutdown commands. Saying "accounts and network
/// setup" about either of those is simply untrue, and a warning the reader can
/// see is wrong is a warning they stop reading.
///
/// They are still reported and can be merged by hand (or with `M` in the
/// per-file review, which edits rather than replaces).
const PROTECTED_CONFIGS: &[(&str, &str)] = &[
    ("/etc/passwd", "your user accounts — replacing it deletes every account on this system"),
    ("/etc/shadow", "the passwords for those accounts — replacing it locks everyone out"),
    ("/etc/group", "your groups and their members — replacing it drops every membership"),
    (
        "/etc/rc.d/rc.inet1.conf",
        "your network configuration — replacing it can leave this machine offline",
    ),
    (
        "/etc/rc.d/rc.local",
        "your own start-up commands — replacing it discards everything you put there",
    ),
    (
        "/etc/rc.d/rc.local_shutdown",
        "your own shutdown commands — replacing it discards everything you put there",
    ),
];

fn is_protected_config(target: &std::path::Path) -> bool {
    protected_reason(target).is_some()
}

/// Why `target` is exempt, or `None` if it is an ordinary config.
fn protected_reason(target: &std::path::Path) -> Option<&'static str> {
    PROTECTED_CONFIGS
        .iter()
        .find(|(p, _)| target == std::path::Path::new(p))
        .map(|(_, why)| *why)
}

/// What a NEWCONFIG_* rule in slacker.conf decides for one file.
#[derive(Debug, PartialEq)]
enum Preset {
    Overwrite,
    Remove,
    /// Keep the installed file and rename the .new to .new.kept, so nothing
    /// is left pending as .new and nothing is lost.
    Keep,
    /// Only a glob in NEWCONFIG_OVERWRITE matched a protected file: it goes to
    /// the menu as usual, with a note.
    Protected,
}

/// Shell-style match of one path: `*` is any run of characters and `?` any one
/// character, neither crossing a `/`; everything else is literal.
fn glob_match(pat: &str, path: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len())
                .take_while(|&i| i == 0 || s[i - 1] != '/')
                .any(|i| go(&p[1..], &s[i..])),
            Some('?') => s.first().is_some_and(|c| *c != '/') && go(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    let (p, s): (Vec<char>, Vec<char>) = (pat.chars().collect(), path.chars().collect());
    go(&p, &s)
}

fn is_glob(pat: &str) -> bool {
    pat.contains(['*', '?'])
}

/// The rule for `target`, if any. The safer action wins when several match:
/// keep, then remove, then overwrite. A glob never overwrites a protected file;
/// only its exact path does.
fn preset_for(target: &std::path::Path, rules: &NewConfigRules) -> Option<Preset> {
    let t = target.to_str()?;
    let hit = |list: &[String]| list.iter().any(|p| glob_match(p, t));
    if hit(&rules.keep) {
        return Some(Preset::Keep);
    }
    if hit(&rules.remove) {
        return Some(Preset::Remove);
    }
    if rules.overwrite.iter().any(|p| !is_glob(p) && p == t) {
        return Some(Preset::Overwrite);
    }
    if hit(&rules.overwrite) {
        return Some(if is_protected_config(target) { Preset::Protected } else { Preset::Overwrite });
    }
    None
}

/// Carry out the NEWCONFIG_* rules on the files that need a decision, report
/// each one, and return those still left for the menu.
fn apply_presets<'a>(
    rules: &NewConfigRules,
    conflicts: Vec<&'a newconfig::NewConfig>,
    dry_run: bool,
) -> Result<Vec<&'a newconfig::NewConfig>, String> {
    let mut left = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    // Padded before colouring, so the paths line up in both runs.
    let would = |done: &str, dry: &str| format!("{:<15}", if dry_run { dry } else { done });
    for nc in conflicts {
        let target = nc.target.display().to_string();
        let new_file = nc.new_file.display().to_string();
        match preset_for(&nc.target, rules) {
            None => left.push(nc),
            Some(Preset::Overwrite) => {
                if !dry_run {
                    overwrite_with_orig(nc)?;
                }
                lines.push(format!(
                    "  {}  {target}  {}",
                    ui::green(&would("overwrote", "would overwrite")),
                    ui::dim("(previous saved as .orig)")
                ));
            }
            Some(Preset::Remove) => {
                if !dry_run {
                    std::fs::remove_file(&nc.new_file).map_err(|e| format!("remove {new_file}: {e}"))?;
                }
                lines.push(format!(
                    "  {}  {new_file}  {}",
                    ui::green(&would("removed", "would remove")),
                    ui::dim("(kept your current file)")
                ));
            }
            Some(Preset::Keep) => {
                // Keep the user's file and move the .new aside as .new.kept:
                // nothing stays pending as .new, and nothing is lost.
                let kept = format!("{new_file}.kept");
                if !dry_run {
                    std::fs::rename(&nc.new_file, &kept).map_err(|e| format!("rename {new_file}: {e}"))?;
                }
                lines.push(format!(
                    "  {}  {target}  {}",
                    ui::green(&would("kept", "would keep")),
                    ui::dim(&format!("(new version saved as {kept})"))
                ));
            }
            Some(Preset::Protected) => {
                lines.push(format!(
                    "  {}  {new_file}  {}",
                    ui::yellow(&would("skipped", "would skip")),
                    ui::dim("(protected: name it exactly in NEWCONFIG_OVERWRITE to overwrite)")
                ));
                left.push(nc);
            }
        }
    }
    if !lines.is_empty() {
        println!();
        println!("{}", ui::blue("Pre-set in slacker.conf:"));
        for l in lines {
            println!("{l}");
        }
    }
    Ok(left)
}

/// Warn about `.new` files that were deliberately left alone, naming them so the
/// user knows there is still something to merge by hand.
fn report_protected_skips(skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    println!(
        "{}",
        ui::yellow(&format!(
            "  {} file(s) NOT replaced — they hold settings unique to this machine:",
            skipped.len()
        ))
    );
    for s in skipped {
        println!("      {}", ui::white(s));
        if let Some(why) = protected_reason(std::path::Path::new(s)) {
            println!("{}", ui::dim(&format!("          {why}")));
        }
    }
    println!(
        "{}",
        ui::dim("    slacker never replaces these. Their .new files are left in place.")
    );
}

/// Install the `.new` and keep the previous file as `.bak`, as the script in
/// Patrick's upgrade HOWTO does (`cp -a` then `mv`). A separate suffix from the
/// `.orig` of the ordinary overwrite, so it is obvious which pass produced it.
fn overwrite_with_bak(nc: &newconfig::NewConfig) -> Result<(), String> {
    if nc.target.exists() {
        let mut bak = nc.target.as_os_str().to_os_string();
        bak.push(".bak");
        let bak = std::path::PathBuf::from(bak);
        std::fs::copy(&nc.target, &bak)
            .map_err(|e| format!("back up {} -> {}: {e}", nc.target.display(), bak.display()))?;
    }
    std::fs::rename(&nc.new_file, &nc.target)
        .map_err(|e| format!("install {}: {e}", nc.target.display()))?;
    Ok(())
}

/// Overwrite `target` with the `.new` file, first saving the existing `target`
/// as `<target>.orig` (slackpkg-style), so the previous config stays recoverable.
/// If a `.orig` already exists it is replaced (latest superseded config wins).
fn overwrite_with_orig(nc: &newconfig::NewConfig) -> Result<(), String> {
    if nc.target.exists() {
        let mut orig = nc.target.as_os_str().to_os_string();
        orig.push(".orig");
        let orig = std::path::PathBuf::from(orig);
        std::fs::copy(&nc.target, &orig)
            .map_err(|e| format!("back up {} -> {}: {e}", nc.target.display(), orig.display()))?;
    }
    std::fs::rename(&nc.new_file, &nc.target)
        .map_err(|e| format!("overwrite {}: {e}", nc.target.display()))?;
    Ok(())
}

/// Interactive per-file resolution for one differing config file: show the diff,
/// then [K]eep both / [O]verwrite (old saved as .orig) / [R]emove .new / [M]erge /
/// [D]iff. Used by the "(P)rompt one by one" path of `cmd_new_config`. `idx`/
/// `total` drive a per-file header so it's obvious which file is under review
/// and where one ends and the next begins.
fn review_one_config(nc: &newconfig::NewConfig, idx: usize, total: usize) -> Result<(), String> {
    let bar = "─".repeat(60);
    println!("\n{}", ui::orange(&bar));
    println!(
        "{}  {}",
        ui::cyan(&format!("[{idx}/{total}]")),
        ui::white(&nc.target.display().to_string())
    );
    println!("{}", ui::orange(&bar));
    show_config_diff(&nc.target, &nc.new_file);
    loop {
        print!(
            "  {} ",
            choice_line(
                "",
                &[
                    ("K", "eep both"),
                    ("O", "verwrite"),
                    ("R", "emove .new"),
                    ("M", "erge"),
                    ("D", "iff"),
                ],
                "K",
            )
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            break;
        }
        match line.trim().to_lowercase().as_str() {
            "" | "k" => {
                println!("    {}", ui::dim("kept both — decide later"));
                break;
            }
            "o" => {
                if let Some(why) = protected_reason(&nc.target) {
                    println!(
                        "    {}",
                        ui::yellow(&format!(
                            "not replaced — this file holds {why}. Use M to merge \
                             the parts you want."
                        ))
                    );
                    continue;
                }
                overwrite_with_orig(nc)?;
                println!("    {}", ui::dim("overwritten with the new file (old saved as .orig)"));
                break;
            }
            "r" => {
                std::fs::remove_file(&nc.new_file).map_err(|e| format!("remove: {e}"))?;
                println!("    {}", ui::dim("kept your current config — removed .new"));
                break;
            }
            "m" => {
                match merge_config(&nc.target, &nc.new_file) {
                    Ok(()) => {
                        if confirm("  merge done — remove the .new file?", false) {
                            std::fs::remove_file(&nc.new_file).map_err(|e| format!("remove: {e}"))?;
                            println!("    {}", ui::dim("merged, .new removed"));
                        } else {
                            println!("    {}", ui::dim("merged, .new left in place"));
                        }
                    }
                    Err(e) => println!("    {}", ui::red(&e)),
                }
                break;
            }
            "d" => {
                show_config_diff(&nc.target, &nc.new_file);
            }
            other => {
                println!("    {}", ui::dim(&format!("'{other}'? choose K, O, R, M or D")));
            }
        }
    }
    Ok(())
}

/// `dist_mode` adds the "(A)ll new" bulk choice: install every `.new` and keep
/// the previous file as `.bak`, the shape of the script in Patrick's upgrade
/// HOWTO. It is offered only during `upgrade-dist`, where taking the target
/// release's configs wholesale is a reasonable default; in ordinary use the
/// answer is almost always to keep what you have.
pub(crate) fn cmd_new_config(cli: &Cli, dist_mode: bool) -> Result<Outcome, String> {
    let found = newconfig::find_new_configs(&newconfig::default_roots());
    if found.is_empty() {
        println!("No .new configuration files found.");
        return Ok(Outcome::Ok);
    }

    // Classify up front so we can show the whole picture before asking anything:
    //  * broken    — a .new with no installed counterpart (a broken package; we
    //                never touch it, just warn);
    //  * identical — a .new byte-identical to the installed file (redundant, drop);
    //  * conflicts — a .new that differs (the ones that actually need a decision).
    let mut broken: Vec<&newconfig::NewConfig> = Vec::new();
    let mut identical: Vec<&newconfig::NewConfig> = Vec::new();
    let mut conflicts: Vec<&newconfig::NewConfig> = Vec::new();
    for nc in &found {
        if !nc.target.exists() {
            broken.push(nc);
        } else if files_identical(&nc.target, &nc.new_file) {
            identical.push(nc);
        } else {
            conflicts.push(nc);
        }
    }

    // Broken packages: warn loudly, never touch.
    for nc in &broken {
        let bar = "=".repeat(66);
        println!("{}", ui::red(&format!("  {bar}")));
        println!("{}", ui::red("  !! WARNING: this package looks broken"));
        println!("{}", ui::red("  !! a .new config file was installed but no previous version exists:"));
        println!("{}{}", ui::red("  !!   "), ui::white(&nc.new_file.display().to_string()));
        println!("{}", ui::red("  !! slacker cannot diff or merge it. Please review it manually,"));
        println!("{}", ui::red("  !! at your own responsibility."));
        println!("{}", ui::red(&format!("  {bar}")));
    }

    // Redundant .new identical to the installed file: drop them (report in dry-run).
    if !identical.is_empty() {
        if cli.dry_run {
            println!(
                "{}",
                ui::dim(&format!(
                    "{} .new identical to the installed file (would remove)",
                    identical.len()
                ))
            );
        } else {
            let mut removed = 0usize;
            for nc in &identical {
                if std::fs::remove_file(&nc.new_file).is_ok() {
                    removed += 1;
                }
            }
            if removed > 0 {
                println!(
                    "{}",
                    ui::dim(&format!("removed {removed} redundant .new (identical to installed)"))
                );
            }
        }
    }

    // Files the user decided about in advance (NEWCONFIG_* in slacker.conf)
    // are handled here; only the rest reach the list and the menu.
    // Not in a distribution upgrade: like the blacklist, these rules are for
    // ordinary updates, and a release jump is when every config should be seen.
    let rules = if dist_mode {
        let r = config::newconfig_rules(&cli.config_dir);
        if r != NewConfigRules::default() {
            println!(
                "{}",
                ui::dim("NEWCONFIG_* rules in slacker.conf are not applied in a distribution upgrade.")
            );
        }
        NewConfigRules::default()
    } else {
        config::newconfig_rules(&cli.config_dir)
    };
    let had = conflicts.len();
    let conflicts = apply_presets(&rules, conflicts, cli.dry_run)?;

    if conflicts.is_empty() {
        if had > 0 {
            println!("{}", ui::green("Nothing else needs a decision."));
        } else {
            println!("{}", ui::green("No config files need merging."));
        }
        return Ok(Outcome::Ok);
    }

    // ---- Phase 1: show the whole list of differing files up front ----
    println!();
    println!(
        "{}",
        ui::blue(&format!("{} config file(s) differ from the new version:", conflicts.len()))
    );
    for (i, nc) in conflicts.iter().enumerate() {
        let name = ui::white(&nc.target.display().to_string());
        if is_protected_config(&nc.target) {
            println!("  {:>3}) {}  {}", i + 1, name, ui::red("← PROTECTED, will not be replaced"));
        } else {
            println!("  {:>3}) {}", i + 1, name);
        }
    }

    // Say this BEFORE offering any choice: seeing /etc/shadow in a list with an
    // "overwrite all" option next to it is alarming, and the user has no way to
    // know it is exempt until after answering.
    let protected: Vec<&newconfig::NewConfig> =
        conflicts.iter().copied().filter(|nc| is_protected_config(&nc.target)).collect();
    if !protected.is_empty() {
        let bar = "=".repeat(66);
        println!();
        println!("{}", ui::red(&bar));
        println!("{}", ui::red("  WARNING — these files hold settings unique to this machine:"));
        // The reason comes from the file itself: the protected files are not one
        // kind of thing, and a blanket "accounts and network" is plainly wrong
        // for rc.local / rc.local_shutdown — which is exactly the sort of error
        // that teaches a reader to skip warnings.
        for nc in &protected {
            println!("{}{}", ui::red("    "), ui::white(&nc.target.display().to_string()));
            if let Some(why) = protected_reason(&nc.target) {
                println!("{}", ui::red(&format!("        {why}")));
            }
        }
        println!(
            "{}",
            ui::red("  slacker will NOT replace them, whatever you answer below.")
        );
        println!(
            "{}",
            ui::red(
                "  If you need something from the new version, merge it BY HAND:\n  \
                 answer P, then M on that file — and be sure of the change before\n  \
                 you make it. When in doubt, change nothing: keeping your current\n  \
                 file is always the safe answer."
            )
        );
        println!("{}", ui::red(&bar));
    }

    if cli.dry_run {
        println!("{}", ui::dim("dry run — re-run without --dry-run to choose what to do"));
        return Ok(Outcome::Ok);
    }

    // --yes: non-interactive, keep the current configs (drop the .new) — the safe
    // default, and what the dist phase needs so it never blocks on a prompt.
    if cli.yes {
        let mut n = 0usize;
        for nc in &conflicts {
            if std::fs::remove_file(&nc.new_file).is_ok() {
                n += 1;
            }
        }
        println!("{}", ui::dim(&format!("--yes: kept current configs, removed {n} .new file(s)")));
        return Ok(Outcome::Ok);
    }

    // ---- Phase 2: one bulk choice for ALL files (slackpkg-style K/O/R/P) ----
    println!();
    loop {
        let mut choices: Vec<(&str, &str)> = vec![
            ("K", "eep current (.new left for later)"),
            ("O", "verwrite all (old saved as .orig)"),
            ("R", "emove all .new"),
            ("P", "rompt one by one"),
        ];
        if dist_mode {
            choices.push(("A", "ll new — take the release's configs (old saved as .bak)"));
        }
        print!(
            "{} ",
            choice_line(&format!("ALL {} files:", conflicts.len()), &choices, "P")
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return Ok(Outcome::Ok);
        }
        match line.trim().to_lowercase().as_str() {
            "o" => {
                if !confirm(
                    &format!(
                        "Overwrite all {} files with the new versions (old saved as .orig)?",
                        conflicts.len()
                    ),
                    false,
                ) {
                    println!("{}", ui::blue("cancelled — nothing changed."));
                    return Ok(Outcome::Ok);
                }
                let mut n = 0usize;
                let mut skipped: Vec<String> = Vec::new();
                for nc in &conflicts {
                    if is_protected_config(&nc.target) {
                        skipped.push(nc.target.display().to_string());
                        continue;
                    }
                    overwrite_with_orig(nc)?;
                    n += 1;
                }
                println!(
                    "{}",
                    ui::green(&format!(
                        "overwrote {n} file(s) with the new versions (previous saved as .orig)."
                    ))
                );
                report_protected_skips(&skipped);
                return Ok(Outcome::Ok);
            }
            "a" if dist_mode => {
                // Patrick's HOWTO script: install every incoming config, keeping
                // the previous one as .bak — minus the identity/network files,
                // which his script skips too.
                if !confirm(
                    &format!(
                        "Take the new release's version of all {} files (old saved as .bak)?",
                        conflicts.len()
                    ),
                    false,
                ) {
                    println!("{}", ui::blue("cancelled — nothing changed."));
                    return Ok(Outcome::Ok);
                }
                let mut n = 0usize;
                let mut skipped: Vec<String> = Vec::new();
                for nc in &conflicts {
                    if is_protected_config(&nc.target) {
                        skipped.push(nc.target.display().to_string());
                        continue;
                    }
                    overwrite_with_bak(nc)?;
                    n += 1;
                }
                println!(
                    "{}",
                    ui::green(&format!(
                        "installed {n} new config file(s); the previous ones are alongside as .bak."
                    ))
                );
                report_protected_skips(&skipped);
                return Ok(Outcome::Ok);
            }
            "k" => {
                // slackpkg (K)eep: keep the current files, LEAVE the .new files in
                // place to deal with later. (Use R to discard them.)
                println!(
                    "{}",
                    ui::green(&format!(
                        "kept your current configs — {} .new file(s) left in place for later.",
                        conflicts.len()
                    ))
                );
                return Ok(Outcome::Ok);
            }
            "r" => {
                if !confirm(
                    &format!("Remove all {} .new file(s) and keep your current configs?", conflicts.len()),
                    false,
                ) {
                    println!("{}", ui::blue("cancelled — nothing changed."));
                    return Ok(Outcome::Ok);
                }
                let mut n = 0usize;
                for nc in &conflicts {
                    std::fs::remove_file(&nc.new_file)
                        .map_err(|e| format!("remove {}: {e}", nc.new_file.display()))?;
                    n += 1;
                }
                println!(
                    "{}",
                    ui::green(&format!("kept your current configs — removed {n} .new file(s)."))
                );
                return Ok(Outcome::Ok);
            }
            "" | "p" => {
                // ---- Phase 3: prompt for each one individually ----
                let total = conflicts.len();
                for (i, nc) in conflicts.iter().enumerate() {
                    review_one_config(nc, i + 1, total)?;
                }
                println!("\n{}", ui::blue(&format!("done — reviewed {total} file(s).")));
                return Ok(Outcome::Ok);
            }
            other => {
                println!("    {}", ui::dim(&format!("'{other}'? choose K, O, R or P")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_are_shell_like() {
        assert!(glob_match("/etc/rc.d/rc.*", "/etc/rc.d/rc.S"));
        assert!(glob_match("/etc/rc.d/rc.?", "/etc/rc.d/rc.M"));
        assert!(!glob_match("/etc/rc.d/rc.?", "/etc/rc.d/rc.local"));
        assert!(glob_match("/etc/*", "/etc/fstab"));
        assert!(!glob_match("/etc/*", "/etc/rc.d/rc.S")); // * stops at /
        assert!(glob_match("/etc/*/*", "/etc/rc.d/rc.S"));
        assert!(glob_match("/etc/ntp.conf", "/etc/ntp.conf"));
        assert!(!glob_match("/etc/ntp.conf", "/etc/ntp.conf2"));
    }

    #[test]
    fn presets_prefer_the_safer_action_and_spare_protected_files() {
        use std::path::Path;
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let rules = NewConfigRules {
            overwrite: s(&["/etc/rc.d/rc.*", "/etc/rc.d/rc.local_shutdown", "/etc/ntp.conf"]),
            remove: s(&["/etc/ntp.conf"]),
            keep: s(&["/etc/rc.d/rc.modules.local"]),
        };
        let p = |t: &str| preset_for(Path::new(t), &rules);
        assert_eq!(p("/etc/rc.d/rc.S"), Some(Preset::Overwrite));
        assert_eq!(p("/etc/rc.d/rc.modules.local"), Some(Preset::Keep)); // keep beats overwrite
        assert_eq!(p("/etc/ntp.conf"), Some(Preset::Remove)); // remove beats overwrite
        assert_eq!(p("/etc/rc.d/rc.local"), Some(Preset::Protected)); // a glob spares it
        assert_eq!(p("/etc/rc.d/rc.local_shutdown"), Some(Preset::Overwrite)); // named exactly
        assert_eq!(p("/etc/fstab"), None);
    }

    #[test]
    fn identity_and_network_configs_are_never_auto_replaced() {
        use std::path::Path;
        // These carry the accounts and the network bring-up. The `etc` and
        // `network-scripts` packages ship a .new for each on ordinary upgrades,
        // so "overwrite all" must never take them: replacing /etc/passwd and
        // /etc/shadow with the stock versions locks the user out, and the .orig
        // backup is useless once login is impossible.
        for p in [
            "/etc/passwd",
            "/etc/shadow",
            "/etc/group",
            "/etc/rc.d/rc.inet1.conf",
            "/etc/rc.d/rc.local",
            "/etc/rc.d/rc.local_shutdown",
        ] {
            assert!(is_protected_config(Path::new(p)), "{p} must be protected");
        }
        // Everything else is fair game for an overwrite.
        for p in ["/etc/fstab", "/etc/slackpkg/mirrors", "/usr/share/vim/vimrc", "/etc/passwd.d/x"] {
            assert!(!is_protected_config(Path::new(p)), "{p} must not be protected");
        }
        // Match is on the resolved target path, not a suffix: a same-named file
        // elsewhere is not protected.
        assert!(!is_protected_config(Path::new("/home/user/passwd")));

        // Every exempt file states its OWN reason. They are not one kind of
        // thing — three are the account database, one is the network bring-up,
        // and rc.local / rc.local_shutdown are the user's own start-up and
        // shutdown commands — so a blanket "accounts and network setup" would be
        // visibly untrue for those two, and a warning a reader can see is wrong
        // is one they stop reading.
        for (path, why) in PROTECTED_CONFIGS {
            assert!(!why.is_empty(), "{path} needs a reason");
            assert_eq!(protected_reason(Path::new(path)), Some(*why));
        }
        assert!(protected_reason(Path::new("/etc/rc.d/rc.local"))
            .unwrap()
            .contains("start-up commands"));
        // The shutdown twin must say shutdown, not start-up: a copy-pasted reason
        // would be the same kind of visible untruth this table exists to avoid.
        assert!(protected_reason(Path::new("/etc/rc.d/rc.local_shutdown"))
            .unwrap()
            .contains("shutdown commands"));
        assert!(protected_reason(Path::new("/etc/rc.d/rc.inet1.conf"))
            .unwrap()
            .contains("network"));
        assert!(protected_reason(Path::new("/etc/passwd")).unwrap().contains("accounts"));
        assert_eq!(protected_reason(Path::new("/etc/fstab")), None);
    }
}
