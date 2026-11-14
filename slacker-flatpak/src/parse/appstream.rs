//! A remote's appstream catalog (appstream.xml), as flatpak keeps it under
//! BASE/appstream/REMOTE/ARCH/active/. Only what the Browse page shows is
//! read: id, name, summary, categories, the cached icon and the flatpak ref.
//! Translations (xml:lang) and long descriptions are skipped.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// The flatpak application ID (from the bundle ref when there is one).
    pub id: String,
    pub name: String,
    pub summary: String,
    pub categories: Vec<String>,
    /// File name of the cached icon, under icons/128x128 or icons/64x64.
    pub icon: Option<String>,
    pub branch: String,
}

/// The component types the Browse page lists; add-ons, runtimes, fonts and
/// the like are left out.
const KINDS: &[&str] = &["desktop", "desktop-application", "console-application"];

#[derive(Default)]
struct Partial {
    kind: String,
    id: String,
    name: String,
    summary: String,
    categories: Vec<String>,
    icon: Option<(u32, String)>,
    bundle: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Id,
    Name,
    Summary,
    Category,
    Icon(u32),
    Bundle,
}

fn attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

pub fn parse(xml: &str) -> Vec<App> {
    let mut r = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut cur: Option<Partial> = None;
    // Depth inside the current <component>: 1 is its direct children.
    let mut depth = 0usize;
    let mut field: Option<Field> = None;
    let mut text = String::new();

    loop {
        let ev = match r.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        match ev {
            Event::Start(e) => {
                let name = e.local_name();
                let name = name.as_ref();
                if cur.is_none() {
                    if name == "component" {
                        cur = Some(Partial { kind: attr(&e, "type").unwrap_or_default(), ..Default::default() });
                        depth = 0;
                    }
                    continue;
                }
                depth += 1;
                let untranslated = attr(&e, "xml:lang").is_none();
                field = match (depth, name) {
                    (1, "id") => Some(Field::Id),
                    (1, "name") if untranslated => Some(Field::Name),
                    (1, "summary") if untranslated => Some(Field::Summary),
                    (2, "category") => Some(Field::Category),
                    (1, "icon") if attr(&e, "type").as_deref() == Some("cached") => {
                        Some(Field::Icon(attr(&e, "height").and_then(|h| h.parse().ok()).unwrap_or(0)))
                    }
                    (1, "bundle") if attr(&e, "type").as_deref() == Some("flatpak") => Some(Field::Bundle),
                    _ => None,
                };
                text.clear();
            }
            Event::Text(t) if field.is_some() => text.push_str(&t.xml10_content()),
            Event::CData(t) if field.is_some() => text.push_str(&t.xml10_content()),
            Event::GeneralRef(g) if field.is_some() => {
                if let Ok(s) = quick_xml::escape::unescape(&format!("&{};", g.xml10_content())) {
                    text.push_str(&s);
                }
            }
            Event::End(e) => {
                let Some(c) = cur.as_mut() else { continue };
                if depth == 0 {
                    if e.local_name().as_ref() == "component" {
                        if let Some(app) = finish(cur.take().unwrap()) {
                            out.push(app);
                        }
                    }
                    continue;
                }
                if let Some(f) = field.take() {
                    let v = text.trim().to_string();
                    match f {
                        Field::Id => c.id = v,
                        Field::Name => c.name = v,
                        Field::Summary => c.summary = v,
                        Field::Category => {
                            if !v.is_empty() {
                                c.categories.push(v)
                            }
                        }
                        Field::Icon(h) => {
                            // Keep the largest cached icon.
                            if !v.is_empty() && c.icon.as_ref().is_none_or(|(old, _)| h > *old) {
                                c.icon = Some((h, v));
                            }
                        }
                        Field::Bundle => c.bundle = v,
                    }
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    out
}

fn finish(p: Partial) -> Option<App> {
    if !KINDS.contains(&p.kind.as_str()) {
        return None;
    }
    // Bundle: app/ID/ARCH/BRANCH.
    let parts: Vec<&str> = p.bundle.split('/').collect();
    let (id, branch) = if parts.len() == 4 && parts[0] == "app" {
        (parts[1].to_string(), parts[3].to_string())
    } else {
        (p.id.strip_suffix(".desktop").unwrap_or(&p.id).to_string(), String::new())
    };
    if crate::commands::valid_id(&id).is_err() {
        return None;
    }
    Some(App { id, name: p.name, summary: p.summary, categories: p.categories, icon: p.icon.map(|(_, f)| f), branch })
}

/// The groups the Browse page offers, each with the freedesktop main
/// categories it covers.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Audio & Video", &["AudioVideo", "Audio", "Video"]),
    ("Development", &["Development"]),
    ("Education", &["Education"]),
    ("Games", &["Game"]),
    ("Graphics", &["Graphics"]),
    ("Internet", &["Network"]),
    ("Office", &["Office"]),
    ("Science", &["Science"]),
    ("System", &["System"]),
    ("Utilities", &["Utility"]),
];

pub fn in_group(app: &App, group: &str) -> bool {
    GROUPS
        .iter()
        .find(|(g, _)| *g == group)
        .is_some_and(|(_, cats)| app.categories.iter().any(|c| cats.contains(&c.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<components version="0.8" origin="flathub">
  <component type="desktop-application">
    <id>org.gnome.baobab.desktop</id>
    <name>Disk Usage Analyzer</name>
    <name xml:lang="el">Αναλυτής χρήσης δίσκου</name>
    <summary>Check folder sizes &amp; available disk space</summary>
    <summary xml:lang="el">Έλεγχος μεγέθους φακέλων</summary>
    <description><p>Long text<name>not this</name></p></description>
    <categories><category>GTK</category><category>System</category><category>Utility</category></categories>
    <icon type="cached" height="64" width="64">org.gnome.baobab.png</icon>
    <icon type="cached" height="128" width="128">org.gnome.baobab.png</icon>
    <icon type="remote" height="128">https://example.org/x.png</icon>
    <bundle type="flatpak" runtime="org.gnome.Platform/x86_64/51">app/org.gnome.baobab/x86_64/stable</bundle>
  </component>
  <component type="addon">
    <id>org.gnome.baobab.Locale</id>
    <name>Locale</name>
    <bundle type="flatpak">runtime/org.gnome.baobab.Locale/x86_64/stable</bundle>
  </component>
  <component type="desktop">
    <id>org.test.Hello</id>
    <name><![CDATA[Hello <test>]]></name>
    <summary>Says hello &#x2014; for testing</summary>
    <categories><category>Game</category></categories>
  </component>
</components>"#;

    #[test]
    fn reads_apps_and_skips_addons_and_translations() {
        let a = parse(XML);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].id, "org.gnome.baobab");
        assert_eq!(a[0].branch, "stable");
        assert_eq!(a[0].name, "Disk Usage Analyzer");
        assert_eq!(a[0].summary, "Check folder sizes & available disk space");
        assert_eq!(a[0].categories, vec!["GTK", "System", "Utility"]);
        assert_eq!(a[0].icon.as_deref(), Some("org.gnome.baobab.png"));
        assert_eq!(a[1].id, "org.test.Hello");
        assert_eq!(a[1].name, "Hello <test>");
        assert_eq!(a[1].summary, "Says hello \u{2014} for testing");
        assert_eq!(a[1].icon, None);
        assert!(in_group(&a[0], "System") && in_group(&a[0], "Utilities"));
        assert!(!in_group(&a[0], "Games") && in_group(&a[1], "Games"));
    }

    #[test]
    fn reads_the_appstream_flatpak_builds() {
        let a = parse(include_str!("../../fixtures/appstream-1.14.xml"));
        assert!(a.iter().any(|x| x.id == "org.test.Hello" && x.branch == "stable" && x.icon.is_some()));
    }

    #[test]
    fn broken_xml_gives_what_was_read_and_no_panic() {
        let cut = &XML[..XML.find("<component type=\"addon\">").unwrap()];
        assert_eq!(parse(cut).len(), 1);
        assert!(parse("<<<").is_empty());
        assert!(parse("").is_empty());
    }
}
