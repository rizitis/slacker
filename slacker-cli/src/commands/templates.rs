//! `generate-template`, `install-template`, `remove-template`, `delete-template`.

use crate::cli::Cli;
use crate::config::Config;
use crate::pkgdb::PkgDb;
use crate::{system, template, ui};
use crate::{
    bl_installed, confirm, confirm_conflicts, detect_conflicts, execute_plan, expand_with_deps,
    hint_freeze_pin, note_optional_suggests, print_plan, print_table, report_conflicts,
    report_pinned_in_plan, InstallAction, Outcome, PlanRow,
};
use std::collections::HashSet;

pub(crate) fn cmd_generate_template(cfg: &Config, name: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // Snapshot installed packages that are known to a repo (skip orphans).
    let orphan_names: HashSet<&str> = db.orphans(&installed).into_iter().map(|p| p.name.as_str()).collect();
    let names: Vec<String> = installed
        .iter()
        .map(|p| p.name.clone())
        .filter(|n| !orphan_names.contains(n.as_str()))
        .collect();
    let path = template::generate(&cfg.config_dir, name, &names)?;
    println!("Wrote template with {} packages: {}", names.len(), path.display());
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_install_template(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let db = PkgDb::load(cfg)?;
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    let names = template::load(&cfg.config_dir, name, true)?;
    let mut todo = Vec::new();
    for n in &names {
        if system::is_installed(&installed, n) {
            continue;
        }
        // Fall through a frozen candidate to the next non-frozen one by
        // priority, like every other install path; only when EVERY candidate is
        // frozen is the package skipped (resolve_unfrozen returns None).
        if let Some(p) = db.resolve_unfrozen(n) {
            todo.push(p);
        } else if db.resolve(n).is_some() {
            // exists but only as frozen candidate(s) -> held, skip quietly
            continue;
        } else {
            eprintln!("template package not found in repos: {n}");
        }
    }
    if todo.is_empty() {
        println!("Nothing to install from template '{name}'.");
        return Ok(Outcome::NothingFound);
    }
    let resolve = cfg.resolve_deps && !cli.no_deps;
    let roots = todo.into_iter().map(|p| (p.clone(), InstallAction::Install)).collect();
    let plan = expand_with_deps(cfg, &db, &installed, roots, resolve, cli.dry_run || cli.yes, cli.dry_run)?;
    print_plan(&plan);
    report_pinned_in_plan(cfg, &plan);
    hint_freeze_pin();
    note_optional_suggests(&plan, resolve);
    let conflicts = detect_conflicts(&plan, &installed, resolve);
    report_conflicts(&conflicts);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    if !confirm_conflicts("Install template packages?", &conflicts, cli.yes)? {
        return Ok(Outcome::Ok);
    }
    execute_plan(cfg, &plan, cli.yes)?;
    Ok(Outcome::Ok)
}

pub(crate) fn cmd_remove_template(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    let installed = system::installed_packages(&cfg.pkg_db_dir)?;
    // Loaded so blacklist series/@repo rules can be evaluated against installed
    // packages (whose series/source aren't recorded locally).
    let db = PkgDb::load(cfg)?;
    let names = template::load(&cfg.config_dir, name, true)?;
    let todo: Vec<&String> = names
        .iter()
        .filter(|n| {
            system::installed_by_name(&installed, n).is_some_and(|i| !bl_installed(cfg, Some(&db), i))
        })
        .collect();
    if todo.is_empty() {
        println!("Nothing to remove from template '{name}'.");
        return Ok(Outcome::NothingFound);
    }
    let rows: Vec<PlanRow> = todo
        .iter()
        .map(|n| {
            let (version, repo) = match installed.iter().find(|i| &i.name == *n) {
                Some(p) => (
                    format!("{}-{}-{}", p.version, p.arch, p.build),
                    {
                        let t = p.build_tag();
                        if t.is_empty() { "-".to_string() } else { t.to_string() }
                    },
                ),
                None => ("-".to_string(), "-".to_string()),
            };
            PlanRow {
                action: "remove",
                color: ui::red,
                name: (*n).clone(),
                version,
                repo,
                note: String::new(),
            }
        })
        .collect();
    print_table(&rows);
    if cli.dry_run {
        println!("(dry-run: nothing changed)");
        return Ok(Outcome::Ok);
    }
    println!(
        "\nThis will UNINSTALL {} package(s) listed in template '{name}' from your system.",
        todo.len()
    );
    println!("(To delete just the template file, use `slacker delete-template {name}`.)");
    if !confirm("Uninstall these packages?", cli.yes) {
        return Ok(Outcome::Ok);
    }
    for n in &todo {
        system::remove_package(n)?;
    }
    Ok(Outcome::Ok)
}

/// Delete a template file. Does not touch installed packages.
pub(crate) fn cmd_delete_template(cli: &Cli, cfg: &Config, name: &str) -> Result<Outcome, String> {
    if cli.dry_run {
        println!("(dry-run) would delete template file '{name}'");
        return Ok(Outcome::Ok);
    }
    let path = template::delete(&cfg.config_dir, name)?;
    println!("Deleted template file: {}", path.display());
    Ok(Outcome::Ok)
}
