//! stockdb.rs — `resolve-stock` support for slacker.
//!
//! Reads a minotavros `depgraph.db` and returns the STOCK dependencies of a
//! package from its PRECOMPUTED `closure` table: "what this package needs to
//! RUN on a bare system". The closure is computed at database-build time from
//! the real ELF files (level-1 = the whole package's NEEDED + its runtime
//! hints; deeper = ONLY the library-loader chains, so tool-only deps of other
//! packages are never dragged in). slacker itself does NO graph walking and
//! NO guessing — one SELECT, the repository's data is the resolver.
//!
//! The database is a special stock-only source, not a repo: opened READ-ONLY,
//! never part of repo verification, priority, or install. Self-contained
//! (only `rusqlite` + std). Networking and slacker's ChangeLog access stay in
//! `main.rs`; this module only does the local, testable pieces.

use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// The full runtime closure of `pkg` from the depgraph DB at `db_path`:
/// one row per needed package, precomputed (see module docs), de-duplicated,
/// self excluded, sorted. Empty on any problem (missing file, unreadable,
/// old-schema DB without a `closure` table, unknown package). QUIET on a
/// missing file — the one-time "no database" warning is printed by the
/// caller — but reports a present-yet-unusable DB.
pub fn deps_for(db_path: &Path, pkg: &str) -> Vec<String> {
    if !db_path.exists() {
        return Vec::new(); // caller warns once; don't spam per package
    }
    match query(db_path, pkg) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  warning: resolve-stock: {e}");
            Vec::new()
        }
    }
}

fn query(db_path: &Path, pkg: &str) -> Result<Vec<String>, String> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", db_path.display()))?;
    let mut stmt = conn
        .prepare("SELECT to_pkg FROM closure WHERE from_pkg=?1")
        .map_err(|e| format!("{e} (stock-db too old? run `slacker update`)"))?;
    let rows = stmt
        .query_map([pkg], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out: Vec<String> = Vec::new();
    for r in rows {
        let dep = r.map_err(|e| e.to_string())?;
        if dep != pkg && !out.contains(&dep) {
            out.push(dep);
        }
    }
    out.sort();
    Ok(out)
}

/// Is the file at `path` a usable stock DB (opens as sqlite, has a `closure`
/// table)? An older depgraph.db that predates the closure table is NOT usable
/// by resolve-stock — status reports it and `update` refetches it.
pub fn is_valid_db(path: &Path) -> bool {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return false;
    };
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='closure'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Validate freshly-downloaded `bytes` as a depgraph DB (opens as sqlite, has
/// `closure`), then atomically put it at `dest` (temp + rename, so a partial
/// or bad download never replaces a good DB). Err if the bytes are not usable.
pub fn validate_and_write(bytes: &[u8], dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = dest.with_extension("db.new");
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    if !is_valid_db(&tmp) {
        // leave the .new behind (overwritten next attempt); never touch `dest`.
        return Err("downloaded file is not a usable depgraph database (no closure table)".into());
    }
    std::fs::rename(&tmp, dest).map_err(|e| format!("install {}: {e}", dest.display()))
}

/// The stock-db's sync stamp from its README.md first line
/// (`ChangeLog.txt: <date>`) — returns the `<date>` part.
pub fn readme_stamp(readme: &str) -> Option<String> {
    let first = readme.lines().next()?;
    let (_, rest) = first.split_once("ChangeLog.txt:")?;
    let s = rest.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The -current head: first non-empty line of the official ChangeLog.txt.
pub fn changelog_head(changelog: &str) -> Option<String> {
    changelog
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    // A tiny in-file DB with the closure schema: precomputed rows, so the
    // tests exercise exactly what slacker reads — no network, no real db.
    fn make_db(path: &std::path::Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE closure(from_pkg TEXT, to_pkg TEXT, via TEXT);
             INSERT INTO closure VALUES ('foo','glibc','linked');
             INSERT INTO closure VALUES ('foo','zlib','linked');
             INSERT INTO closure VALUES ('foo','nettle','lib');
             INSERT INTO closure VALUES ('foo','helper','hint:exec');
             INSERT INTO closure VALUES ('foo','zlib','lib');",
        )
        .unwrap();
    }

    // An OLD-schema DB (edges/hints only, pre-closure) — must be rejected.
    fn make_old_db(path: &std::path::Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE edges(from_pkg TEXT, soname TEXT, to_pkg TEXT);
             INSERT INTO edges VALUES ('foo','libc.so.6','glibc');",
        )
        .unwrap();
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("stockdb_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn deps_come_from_closure_sorted_and_deduped() {
        let db = tmp("closure.db");
        make_db(&db);
        let d = deps_for(&db, "foo");
        // the duplicate zlib row (linked + lib) collapses to one entry
        assert_eq!(
            d,
            vec!["glibc".to_string(), "helper".to_string(), "nettle".to_string(), "zlib".to_string()]
        );
    }

    #[test]
    fn deps_for_unknown_pkg_is_empty() {
        let db = tmp("unknown.db");
        make_db(&db);
        assert!(deps_for(&db, "does-not-exist").is_empty());
    }

    #[test]
    fn deps_for_missing_file_is_empty_and_quiet() {
        // A missing DB must not panic and must return empty (caller warns once).
        assert!(deps_for(std::path::Path::new("/no/such/stockdb.db"), "foo").is_empty());
    }

    #[test]
    fn old_schema_db_is_invalid_and_yields_no_deps() {
        // A pre-closure depgraph.db must be reported invalid (status) and give
        // no deps (install falls back to warn-and-continue via the caller).
        let db = tmp("oldschema.db");
        make_old_db(&db);
        assert!(!is_valid_db(&db), "old edges-only schema must NOT validate");
        assert!(deps_for(&db, "foo").is_empty());
    }

    #[test]
    fn is_valid_db_true_for_closure_false_for_garbage() {
        let db = tmp("valid.db");
        make_db(&db);
        assert!(is_valid_db(&db));

        let junk = tmp("junk.txt");
        std::fs::write(&junk, b"404: Not Found").unwrap();
        assert!(!is_valid_db(&junk));
    }

    #[test]
    fn validate_and_write_accepts_good_rejects_garbage_and_keeps_old() {
        let src = tmp("src.db");
        make_db(&src);
        let good = std::fs::read(&src).unwrap();

        let dest = tmp("dest.db");
        validate_and_write(&good, &dest).expect("a valid db must be accepted");
        assert!(is_valid_db(&dest));

        // Garbage is rejected AND the existing good DB is left intact.
        assert!(validate_and_write(b"not a database", &dest).is_err());
        assert!(is_valid_db(&dest), "a rejected download must not clobber the good db");

        // An OLD-schema download is also rejected (no closure table).
        let old = tmp("olddl.db");
        make_old_db(&old);
        let old_bytes = std::fs::read(&old).unwrap();
        assert!(validate_and_write(&old_bytes, &dest).is_err());
        assert!(is_valid_db(&dest), "an old-schema download must not clobber the good db");
    }

    #[test]
    fn readme_stamp_parses_first_line_only() {
        assert_eq!(
            readme_stamp("ChangeLog.txt: Fri Jul 10 22:25:39 UTC 2026\n---\nrest"),
            Some("Fri Jul 10 22:25:39 UTC 2026".to_string())
        );
        assert_eq!(readme_stamp("some other readme\nChangeLog.txt: x"), None);
        assert_eq!(readme_stamp("ChangeLog.txt:   "), None);
    }

    #[test]
    fn changelog_head_is_first_nonempty_line() {
        assert_eq!(
            changelog_head("\n\nFri Jul 10 22:25:39 UTC 2026\n+----+\n"),
            Some("Fri Jul 10 22:25:39 UTC 2026".to_string())
        );
        assert_eq!(changelog_head("   \n\n"), None);
    }

    #[test]
    fn stamp_and_head_agree_when_in_sync() {
        let readme = "ChangeLog.txt: Fri Jul 10 22:25:39 UTC 2026\n";
        let clog = "Fri Jul 10 22:25:39 UTC 2026\n+----+\n";
        assert_eq!(readme_stamp(readme), changelog_head(clog));
    }
}
