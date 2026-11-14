//! Readers for flatpak's output. runner.rs sets FLATPAK_FANCY_OUTPUT=0, so
//! tables come without a title row, one row per line, cells separated by tabs
//! (flatpak's table printer, app/flatpak-table-printer.c). An empty trailing
//! cell may be left out, so a short row is not an error.

pub mod appstream;
pub mod list;
pub mod metadata;
pub mod pins;
pub mod remotes;
pub mod search;
pub mod transaction;
pub mod updates;

/// The tab-separated cells of a line, padded with empty cells up to `n`.
pub fn cells(line: &str, n: usize) -> Vec<String> {
    let mut v: Vec<String> = line.split('\t').map(|s| s.trim().to_string()).collect();
    while v.len() < n {
        v.push(String::new());
    }
    v
}

/// The lines of `flatpak --installations`: system installation paths.
pub fn installations(out: &str) -> Vec<String> {
    out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
}

/// "Flatpak 1.18.4" -> "1.18.4".
pub fn version(out: &str) -> Option<String> {
    let l = out.lines().next()?.trim();
    let v = l.strip_prefix("Flatpak ").unwrap_or(l).trim();
    (!v.is_empty()).then(|| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_rows_are_padded() {
        assert_eq!(cells("a\tb", 4), vec!["a", "b", "", ""]);
        assert_eq!(cells("a\t\tc", 3), vec!["a", "", "c"]);
    }

    #[test]
    fn version_and_installations() {
        assert_eq!(version("Flatpak 1.18.4\n").as_deref(), Some("1.18.4"));
        assert_eq!(version(""), None);
        let real = include_str!("../../fixtures/installations-1.14.txt");
        assert_eq!(installations(real), vec!["/var/lib/flatpak"]);
    }
}
