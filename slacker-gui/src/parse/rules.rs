//! `slacker frozen`, `slacker pin` and `slacker nodep` without arguments
//! (cmd_frozen, cmd_pin, cmd_nodep): the current freeze rules, pins and
//! dependency exclusions.

#[derive(Debug, Default, PartialEq)]
pub struct Frozen {
    pub rules: Vec<String>,
    pub unrecognized: Vec<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Pins {
    /// (package, repo)
    pub pins: Vec<(String, String)>,
    pub unrecognized: Vec<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Nodeps {
    /// (package, dependency); package is `*` for any package.
    pub rules: Vec<(String, String)>,
    pub unrecognized: Vec<String>,
}

pub fn parse_frozen(text: &str) -> Frozen {
    let mut out = Frozen::default();
    let mut in_list = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t == "No frozen rules set." || t.starts_with("to add a rule:") {
            continue;
        }
        if t == "Current frozen rules:" {
            in_list = true;
            continue;
        }
        match line.strip_prefix("  ") {
            Some(rule) if in_list => out.rules.push(rule.trim_end().to_string()),
            _ => out.unrecognized.push(line.to_string()),
        }
    }
    out
}

pub fn parse_pins(text: &str) -> Pins {
    let mut out = Pins::default();
    let mut in_list = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t == "No pins set." || t.starts_with("to add a pin:") {
            continue;
        }
        if t == "Current pins (package -> repo):" {
            in_list = true;
            continue;
        }
        let pin = line
            .strip_prefix("  ")
            .filter(|_| in_list)
            .and_then(|l| l.split_once(" -> "))
            .map(|(p, r)| (p.trim().to_string(), r.trim().to_string()))
            .filter(|(p, r)| !p.is_empty() && !r.is_empty() && !p.contains(' ') && !r.contains(' '));
        match pin {
            Some(p) => out.pins.push(p),
            None => out.unrecognized.push(line.to_string()),
        }
    }
    out
}

/// `  !package:dependency  ('dependency' is not pulled in for ...)`, one per
/// rule, under "Current dependency exclusions:".
pub fn parse_nodeps(text: &str) -> Nodeps {
    let mut out = Nodeps::default();
    let mut in_list = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t == "No dependency exclusions set." || t.starts_with("to add:") {
            continue;
        }
        if t == "Current dependency exclusions:" {
            in_list = true;
            continue;
        }
        let rule = line
            .strip_prefix("  ")
            .filter(|_| in_list)
            .and_then(|l| l.split_whitespace().next())
            .and_then(|r| r.strip_prefix('!'))
            .and_then(|r| r.split_once(':'))
            .filter(|(p, d)| !p.is_empty() && !d.is_empty() && !d.contains(':'))
            .map(|(p, d)| (p.to_string(), d.to_string()));
        match rule {
            Some(r) => out.rules.push(r),
            None => out.unrecognized.push(line.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_empty_from_capture() {
        let text = "No frozen rules set.\n  to add a rule: `frozen <rule>`, e.g. `frozen vlc`, `frozen kde/`, `frozen \"@alienbob vlc\"`\n";
        assert_eq!(parse_frozen(text), Frozen::default());
    }

    #[test]
    fn frozen_rules() {
        let text = "Current frozen rules:\n  sbopkg\n  fcitx5*\n  @testing xf86-.*-202.*\n  @alienbob 100% vlc\n  kde/\n  to add a rule: `frozen <rule>`, e.g. `frozen vlc`\n";
        let f = parse_frozen(text);
        assert!(f.unrecognized.is_empty());
        assert_eq!(f.rules, vec!["sbopkg", "fcitx5*", "@testing xf86-.*-202.*", "@alienbob 100% vlc", "kde/"]);
    }

    #[test]
    fn pins_empty_from_capture() {
        let text = "No pins set.\n  to add a pin: `pin repo:package`, e.g. `pin alienbob:vlc`\n";
        assert_eq!(parse_pins(text), Pins::default());
    }

    #[test]
    fn pins_listed() {
        let text = "Current pins (package -> repo):\n  vlc -> alienbob\n  python3.11 -> conraid\n  gtk+ -> slackware\n  to add a pin: `pin repo:package`, e.g. `pin alienbob:vlc`\n";
        let p = parse_pins(text);
        assert!(p.unrecognized.is_empty());
        assert_eq!(
            p.pins,
            vec![
                ("vlc".to_string(), "alienbob".to_string()),
                ("python3.11".to_string(), "conraid".to_string()),
                ("gtk+".to_string(), "slackware".to_string()),
            ]
        );
    }

    #[test]
    fn nodeps_empty_from_capture() {
        assert_eq!(parse_nodeps(&crate::parse::fixture("nodep-empty.txt")), Nodeps::default());
    }

    #[test]
    fn nodeps_listed_from_capture() {
        let n = parse_nodeps(&crate::parse::fixture("nodep-list.txt"));
        assert!(n.unrecognized.is_empty(), "{:?}", n.unrecognized);
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            n.rules,
            vec![s("ghostty", "cargo-auditable"), s("*", "xvfb-run"), s("slackpkg+", "gtk+3")]
        );
    }

    #[test]
    fn nodeps_refused_authorization_is_not_a_list() {
        let n = parse_nodeps("Error executing command as another user: Not authorized\n");
        assert!(n.rules.is_empty());
        assert_eq!(n.unrecognized.len(), 1);
    }

    #[test]
    fn refused_authorization_is_not_a_list() {
        let p = parse_pins("Error executing command as another user: Not authorized\n");
        assert!(p.pins.is_empty());
        assert_eq!(p.unrecognized.len(), 1);
    }
}
