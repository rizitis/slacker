//! Runs flatpak as the user and collects what it prints.
//!
//! Environment given to flatpak, and why:
//! - FLATPAK_FANCY_OUTPUT=0: plain tables (tab-separated, no title row, no
//!   cutting long names to the terminal width), the same as when piped.
//! - LC_ALL=C.UTF-8, LANG=C.UTF-8: untranslated messages, so they can be read,
//!   while names and sizes keep their UTF-8 (in plain "C" flatpak replaces
//!   every non-ASCII character, even the space inside "18.3 MB", with '?').
//!
//! stdin is /dev/null: nothing slacker-flatpak runs may wait for an answer.

use std::ffi::OsStr;
use std::path::Path;

use gtk::{gio, glib};

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub const ENV: &[(&str, &str)] = &[("FLATPAK_FANCY_OUTPUT", "0"), ("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8")];

pub async fn run(flatpak: &Path, argv: &[String]) -> Output {
    match spawn(flatpak, argv).await {
        Ok(o) => o,
        Err(e) => Output { ok: false, stdout: String::new(), stderr: format!("could not run flatpak: {e}") },
    }
}

async fn spawn(flatpak: &Path, argv: &[String]) -> Result<Output, glib::Error> {
    let launcher =
        gio::SubprocessLauncher::new(gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE);
    for (k, v) in ENV {
        launcher.setenv(k, v, true);
    }
    let mut full: Vec<&OsStr> = vec![flatpak.as_os_str()];
    full.extend(argv.iter().map(OsStr::new));
    let proc = launcher.spawn(&full)?;
    let (out, err) = proc.communicate_future(None).await?;
    let text = |b: Option<glib::Bytes>| b.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let ok = proc.has_exited() && proc.exit_status() == 0;
    Ok(Output { ok, stdout: text(out), stderr: text(err) })
}

#[cfg(test)]
mod tests {
    use super::ENV;

    #[test]
    fn flatpak_gets_plain_utf8_output() {
        assert!(ENV.contains(&("FLATPAK_FANCY_OUTPUT", "0")));
        assert!(ENV.contains(&("LC_ALL", "C.UTF-8")));
        assert!(!ENV.iter().any(|(_, v)| *v == "C"));
    }
}
