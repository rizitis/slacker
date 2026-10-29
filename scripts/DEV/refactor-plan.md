# slacker-cli: πλάνο refactor του main.rs

Τελευταία ενημέρωση: 29 Οκτ 2026, 21:00. Το refactor είναι σε δικό του τοπικό branch, χωρίς commits μέχρι το release.

## Πού είμαστε (ΟΛΟΚΛΗΡΩΘΗΚΕ)

Το `main.rs` ήταν 12.245 γραμμές. Τώρα είναι 3.922. Έγιναν τα βήματα 1–14. Το βήμα 15 (κοινός πυρήνας) αποφασίστηκε να ΜΗΝ γίνει (βλ. `refactor-core-plan.md`).

Έγιναν (με τη σειρά):

1. `cli.rs`: `Cli`, `Cmd`, `HELP_STYLES` (ορισμός του CLI)
2. `commands/templates.rs`: generate/install/remove/delete-template
3. `commands/repos.rs`: add/del/pri-repo, add/del-tag, vet/trust/distrust-repo (μαζί με το test του `repos_text_set_priority`)
4. `commands/blacklist.rs`: frozen, unfrozen, pin, unpin (μαζί με τα 7 tests του blacklist)
5. `commands/logs.rs`: history, show-changelog (μαζί με τα `render_history`, `source_of`, `page_output`)
6. `commands/query.rs`: search, file-search, info (μαζί με το test `file_search_marks_only_the_exact_installed_build`. Το `collect_tests` μένει στο `main.rs` και κάνει import το `search_display`)
7. `commands/find_mirror.rs`: find-mirror (όχι `mirrors.rs`, γιατί υπάρχει ήδη `src/mirrors.rs`)
8. `commands/revert_pkg.rs`: revert-pkg (μαζί με `danger_banner_foundational`, `revert_fetch_and_gpg_verify`, `release_archive_url`)
9. `commands/clean.rs`: clean-system, clean-cache (μαζί με το `is_pinned_from_its_repo` και το test του `pinned_from_its_repo_predicate`, που βγήκε από το `collect_tests`). Το upgrade-dist (Phase 3) καλεί το `cmd_clean_system` μέσω import.
10. `commands/new_config.rs`: new-config (μαζί με `PROTECTED_CONFIGS`, `is_protected_config`, `protected_reason`, `choice_line`, τα overwrite/merge/diff helpers, και το test `identity_and_network_configs_are_never_auto_replaced`, που βγήκε από το `foundational_tests`). Το upgrade-dist (Phase 5) καλεί το `cmd_new_config(cli, true)` μέσω import.
11. `commands/update.rs`: update και `update gpg` (μαζί με το `refresh_stock_db`). Τα `update_one_repo`, `UpdateOutcomes`, `malicious_filename_count`, `warn_unverified_repos` μένουν στο `main.rs`, γιατί τα χρησιμοποιεί και το upgrade-dist ή άλλες εντολές.
12. `commands/status.rs`: status, list-repos, check-updates (μαζί με `status_full`, attribution, path audit, locate DB, pkgtools/tools, cache scan, freshness URLs, `insecure_http_repos`). Tests που πήγαν μαζί: ολόκληρα τα `attribution_tests` και `locate_db_tests`, και 6 μεμονωμένα από `collect_tests` και `freshness_tests`, σε `status::tests`. Μένουν στο `main.rs`, γιατί τα χρησιμοποιούν και `find_mirror.rs`, `mirrors.rs` ή `update.rs`: `unverified_repo_names`, `warn_unverified_repos`, `osuosl_tree_prefix`, `FRESHNESS_TIMEOUT_SECS`, `FRESHNESS_MAX_LAG_SECS`, `parse_packages_date`, `month_num`, `mirror_is_stale`, `humanize_lag`.
13. `commands/upgrade_dist.rs`: upgrade-dist και 19 δικά του items (`DIST_CRITICAL`, `DIST_GPG_LAST`, `DIST_BATCH`, `dist_upgrade_sets`, `dist_install_new`, disk gate/preflight, `DistReport`, `dist_execute`, `dist_backup`, `backup_stamp`, `dist_transform`, `dist_rewrite_text`, `set_active_mirror_line`, `dist_validate_local_mirror`, `comment_nonmirror_repos`, `refuse_through_symlink`). 4 tests πήγαν μαζί, από το `freshness_tests`. Τα 2 `dist_upgrade_sets_*` μένουν στο `collect_tests` (fixture `av()`) με import, γι' αυτό το `dist_upgrade_sets` είναι `pub(crate)`.
14. `commands/install.rs`: install, upgrade, reinstall, remove, download, upgrade-all, install-new και 20 δικά τους helpers (`note_frozen_*`, `note_pin_excluded`, `major_token`, `advise_console_for_foundational`, `plan_alternatives`, `dep_delta`, `fetch_dep_names`, `show_plan_alternatives`, `fix_pin_repo`, `abort_on_misses`, `selector_miss_hint`, `report_pkg_misses`, `report_installed_misses`, `collect_installed_targets`, `kept_detail`, `select_packages`, `select_packages_pkgid`, `note_selected`, `report_pending_configs`). 4 tests πήγαν μαζί (`fix_pin_repo_*`, `dep_delta_*`, τα δύο `major_token`). Μένουν στο `collect_tests` με import όσα χρησιμοποιούν fixtures, γι' αυτό τα `abort_on_misses`, `selector_miss_hint`, `plan_alternatives` είναι `pub(crate)`. Στο `main.rs` το import έγινε `use std::path::Path;`, γιατί το `PathBuf` δεν χρησιμοποιείται πια εκεί.

Στο `main.rs` έμειναν: η είσοδος (`main`, `run`/dispatch, `Outcome`, lock, privilege, `migrate_state`) και ο κοινός πυρήνας (resolver με το `MAX_DEP_DEPTH`, conflicts, `show_plan`, `execute_plan`, fetch/verify, επιλογή/`collect`, `update_one_repo`, `vet_repo`, `confirm`, `hilite_keys`, `print_table`), μαζί με τα tests του.

Η τελευταία κατάσταση είναι στο `slacker-src-refactor.tar.gz` (16 αρχεία, md5 `a1fa6afe...`). Τα scripts ελέγχου είναι στο `slacker-refactor-harness.tar.gz`. Αν το workspace έχει αλλάξει, ανέβασε πρώτα και τα δύο.

## Η διαδικασία σε κάθε βήμα

1. **Baseline πριν την αλλαγή:** όλα τα help outputs (79 αρχεία: `--help`, και για κάθε εντολή τα `-h` και `help <cmd>`), συν ένα script που τρέχει τις εντολές της ομάδας σε ψεύτικο σύστημα. Το script τρέχει δύο φορές, για να βεβαιωθούμε ότι το baseline είναι σταθερό. Για εντολές που δείχνουν ώρες (π.χ. `history`), το ψεύτικο σύστημα φτιάχνεται μία φορά και το πριν/μετά τρέχουν πάνω στα ίδια αρχεία, γιατί το ctime δεν ελέγχεται.
2. **Χαρτογράφηση:** ποιες συναρτήσεις και ποια helpers χρησιμοποιεί **μόνο** η ομάδα. Από το βήμα 12 με το `closure.py`, που ελέγχει αναφορές και από τα άλλα αρχεία (`src/*.rs`, `src/commands/*.rs`), όχι μόνο από το `main.rs`, και αφαιρεί σχόλια και strings με κανονικό tokenizer. Ποια tests τα ελέγχουν, με το `testmap.py`.
3. **Μετακίνηση αυτούσια:** μόνο το `pub(crate)` στα `cmd_*` (και σε helpers που χρειάζονται tests που μένουν στο `main.rs`), και τα imports όπως τα ζητά ο compiler. Τα tests πάνε μαζί με τον κώδικα που ελέγχουν. Τα σώματα των tests δεν αλλάζουν και ο αριθμός μένει 219.
4. **Έλεγχος:** build χωρίς warnings, 219/219 tests, ίδια help outputs, ίδια συμπεριφορά της ομάδας, και ξανά έλεγχος όλων των ομάδων που έχουν ήδη μετακινηθεί (`check_all.sh`).
5. **Παράδοση:** tarball με τη σωστή δομή φακέλων, που χτίζεται και ελέγχεται ξανά σε καθαρό αντίγραφο του repo.

Για επικίνδυνες εντολές ο έλεγχος περιλαμβάνει επιπλέον: σύγκριση κειμένου γραμμή προς γραμμή με το αρχικό, έλεγχο ότι το `main.rs` είναι ακριβώς το αρχικό χωρίς τα κομμάτια που βγήκαν (συν μόνο τις γραμμές import), έλεγχο ότι κάθε όνομα επιλύεται στο ίδιο item, και πλήρη εκτέλεση του επικίνδυνου μονοπατιού με το binary πριν και μετά, με ψεύτικα pkgtools που μόνο καταγράφουν. Από το βήμα 12 και μετά, και σύγκριση με το binary του κανονικού branch (0.12.4 με cap+quarantine, help, dry-run fix).

- revert-pkg: πραγματικό κλειδί GPG και πραγματικές υπογραφές, ψεύτικο `/etc/os-release` σε ιδιωτικό mount namespace. 11 σενάρια.
- clean-system/clean-cache: 36 εκτελέσεις.
- new-config: σε αντίγραφο του `/etc` και του `/usr/share/vim` (bind mount σε ιδιωτικό namespace). 17 εκτελέσεις, και το dist_mode με 22 εκτελέσεις σε δύο προσωρινά builds με την ίδια μία γραμμή test hook, που δεν υπάρχει στο tarball.
- update: τοπικά file:// mirrors με πραγματικό GPG. 25 εκτελέσεις.
- status/list-repos/check-updates: 29 εκτελέσεις.
- upgrade-dist: 31 εκτελέσεις σε ιδιωτικό namespace mount+net, με αντίγραφο του `/etc` (os-release 15.0), τοπικά mirrors 15.0 και -current (44 πακέτα, GPG, patches), και ψεύτικα pkgtools/`df`/`date`. Περιλαμβάνει: dry-run, άκυρους στόχους, πλήρες `-y`, διαλογικό, παράλληλα downloads, ξανά τρέξιμο, όλες τις αποτυχίες χώρου, core που λείπει/αποτυγχάνει/δεν καταγράφεται, ημιτελές και συνέχεια, τοπικό mirror (4 περιπτώσεις), symlink, γραφικό session, self-upgrade.
- εντολές πακέτων: 105 εκτελέσεις με το ίδιο είδος namespace (os-release -current) και πραγματικό GPG στο official tree. Περιλαμβάνει: εξαρτήσεις, dependency conflict (s/r/a/b), protected deps (k/r/a/q), conflicts, suggests, alternatives, frozen/pin, επιλογή με αριθμούς, misses, release mismatch, kernel/perl reminders, foundational σε κονσόλα και σε γραφικό session, self-upgrade, αποτυχία upgradepkg, `.new` configs, remove με @repo/@tag, download με `-o`, install-new. Και το DoS fix: αλυσίδα 600 εξαρτήσεων, κύκλος, quarantine και συνέχεια, dry-run, upgrade-all με βαθιά αλυσίδα.

**Τελικός έλεγχος (29 Οκτ, βράδυ):** το tarball του βήματος 14 χτίστηκε σε καθαρό αντίγραφο του repo (build χωρίς warnings, 219/219 tests). Όλα τα scripts (79 help, 14 ομάδες, revert πλήρες) βγήκαν ίδια με το binary του κανονικού branch.

## Ανοιχτά θέματα (εκτός refactor)

- **Δύο `arch_compatible` με ανάποδη σειρά ορισμάτων:** `main.rs` `(sys_arch, pkg_arch)` και `repo.rs` `(pkg_arch, arch)`. Κάνουν το ίδιο πράγμα, οπότε είναι παγίδα για μελλοντικό λάθος. Δεν είναι bug σήμερα. Αξίζει να ενοποιηθούν κάποια στιγμή, ως ξεχωριστή αλλαγή.
- **Διόρθωση `--dry-run` στα frozen, unfrozen, pin, unpin:** έγινε και στα δύο branches (κανονικό και refactor). Επιβεβαιώθηκε στο πραγματικό σύστημα με v0.12.5.
- **Ημερομηνίες του `history` μετά από restore (ΔΕΝ διορθώθηκε, σκόπιμα):** το `history.rs` βρίσκει τη ζώνη ώρας συγκρίνοντας την ώρα στο όνομα κάθε `-upgraded-<T>` εγγραφής με το ctime του αρχείου. Το ctime χάνεται σε επαναφορά από backup και τότε μετατοπίζονται οι ημερομηνίες. Αφορά μόνο εμφάνιση και μόνο επαναφερμένα συστήματα. Αποφασίστηκε να μην αγγιχτεί.
- **MSRV:** το `Cargo.toml` του 0.12.5 έλεγε `rust-version = "1.85.1"`, αλλά τα `icu_*` (μέσω ureq → url → idna) θέλουν 1.88, ήδη από το icu 2.3. Προτάθηκε `rust-version = "1.88"`.
- **Doc comments σε λάθος θέση** (τρία, μόνο rustdoc): πάνω από το `unverified_repo_names`, το doc του `cmd_status` πάνω από το `MLOCATE_DB`, και ένα «Parse a keep-selection» πάνω από το `select_packages_pkgid` (τώρα στο `install.rs`). Μένουν όπως είναι.
