//! slacker — Cretan-labyrinth emblem & banner ("banner No. 2").
//!
//! Ariadne's thread through the dependency labyrinth. The classical labyrinth is
//! *unicursal*: one path, no dead ends — like a resolver that finds the single
//! correct way through your dependencies. The heart is the **labrys** (double axe),
//! the etymological root of `labyrinthos`.
//!
//! The emblem is *generated*, not stored: the same kind of pathfinding the resolver
//! does traces the thread that becomes the logo.
//!
//! `show()` prints the horizontal banner at the top of a command, on a terminal
//! only (never into a pipe or log). No dependencies (std only).

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;

type P = (i32, i32);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role { Blank, Wall, Thread, Heart }

/// Emblem size (circuits) for the horizontal banner, and the version string shown
/// in it (the real crate version, kept in sync at compile time).
pub const K_BANNER: i32 = 3;
pub const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

const WORD: [&str; 4] = [
    r"    _         _           ",
    r" __| |__ _ __| |_____ _ _ ",
    r"(_-< / _` / _| / / -_) '_|",
    r"/__/_\__,_\__|_\_\___|_|  ",
];
const TAGLINE: &str = "one thread, no dead ends";

// ── labyrinth generation ───────────────────────────────────────────────────────

fn build(k: i32) -> (Vec<Vec<u8>>, i32, i32, i32) {
    let r = 2 * k + 1;
    let (cx, cy) = (r + 1, r + 1);
    let s = (2 * r + 3) as usize;
    let mut g = vec![vec![b'.'; s]; s];
    // concentric square walls at odd radii
    let mut rad = 1;
    while rad <= r {
        for x in cx - rad..=cx + rad {
            g[(cy - rad) as usize][x as usize] = b'#';
            g[(cy + rad) as usize][x as usize] = b'#';
        }
        for y in cy - rad..=cy + rad {
            g[y as usize][(cx - rad) as usize] = b'#';
            g[y as usize][(cx + rad) as usize] = b'#';
        }
        rad += 2;
    }
    // one gap per wall, alternating bottom/top → a single corridor
    let (mut i, mut rad) = (0, 1);
    while rad <= r {
        if i % 2 == 0 { g[(cy + rad) as usize][cx as usize] = b'.'; }
        else          { g[(cy - rad) as usize][cx as usize] = b'.'; }
        i += 1; rad += 2;
    }
    g[(cy + r) as usize][cx as usize] = b'.';       // entrance
    g[(cy + r + 1) as usize][cx as usize] = b'.';   // mouth
    (g, cx, cy, r)
}

/// Walk the single corridor from the mouth to the heart — Ariadne's thread.
fn trace(g: &[Vec<u8>], cx: i32, cy: i32, r: i32) -> Vec<P> {
    let s = g.len() as i32;
    let open = |x: i32, y: i32| x >= 0 && x < s && y >= 0 && y < s && g[y as usize][x as usize] == b'.';
    let (mut cur, mut prev): (P, Option<P>) = ((cx, cy + r + 1), None);
    let mut path = vec![cur];
    let mut seen: HashSet<P> = HashSet::from([cur]);
    while cur != (cx, cy) {
        let mut next = None;
        for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
            let np = (cur.0 + dx, cur.1 + dy);
            if open(np.0, np.1) && Some(np) != prev && !seen.contains(&np) {
                next = Some(np);
                break;
            }
        }
        match next {
            Some(np) => { prev = Some(cur); cur = np; path.push(cur); seen.insert(cur); }
            None => break,
        }
    }
    path
}

// ── glyph selection ──────────────────────────────────────────────────────────--

fn glyph(cells: &HashSet<P>, x: i32, y: i32, pretty: bool) -> char {
    let n = cells.contains(&(x, y - 1));
    let s = cells.contains(&(x, y + 1));
    let e = cells.contains(&(x + 1, y));
    let w = cells.contains(&(x - 1, y));
    if pretty {
        // thin end-caps for single-neighbour terminals
        match (n, s, e, w) {
            (true, false, false, false) => return '╵',
            (false, true, false, false) => return '╷',
            (false, false, true, false) => return '╶',
            (false, false, false, true) => return '╴',
            _ => {}
        }
    }
    let base = match (n, s, e, w) {
        (false, false, true, true) => '─',
        (true, true, false, false) => '│',
        (true, false, true, false) => '└',
        (true, false, false, true) => '┘',
        (false, true, true, false) => '┌',
        (false, true, false, true) => '┐',
        (true, true, true, false) => '├',
        (true, true, false, true) => '┤',
        (true, false, true, true) => '┴',
        (false, true, true, true) => '┬',
        (true, true, true, true) => '┼',
        (true, false, false, false) | (false, true, false, false) => '│',
        (false, false, true, false) | (false, false, false, true) => '─',
        _ => ' ',
    };
    if pretty {
        match base { '┌' => '╭', '┐' => '╮', '└' => '╰', '┘' => '╯', o => o }
    } else {
        base
    }
}

const LAB_SAFE: [((i32, i32), char); 5] =
    [((0, -1), '│'), ((-1, 0), '◄'), ((0, 0), '┼'), ((1, 0), '►'), ((0, 1), '│')];
const LAB_PRETTY: [((i32, i32), char); 5] =
    [((0, -1), '┃'), ((-1, 0), '◀'), ((0, 0), '╋'), ((1, 0), '▶'), ((0, 1), '┃')];

/// Render the emblem to a grid of (char, role), cropped to content. Returns rows + width.
fn emblem_cells(k: i32, pretty: bool) -> (Vec<Vec<(char, Role)>>, i32) {
    let (g, cx, cy, r) = build(k);
    let th: HashSet<P> = trace(&g, cx, cy, r).into_iter().collect();
    let s = g.len() as i32;
    let mut wall: HashSet<P> = HashSet::new();
    for y in 0..s {
        for x in 0..s {
            if g[y as usize][x as usize] == b'#' {
                wall.insert((x, y));
            }
        }
    }
    let lab = if pretty { LAB_PRETTY } else { LAB_SAFE };
    let heart: HashMap<P, char> = lab.iter().map(|&((dx, dy), c)| ((cx + dx, cy + dy), c)).collect();

    let pts: Vec<P> = wall.iter().chain(th.iter()).copied().collect();
    let x0 = pts.iter().map(|p| p.0).min().unwrap();
    let x1 = pts.iter().map(|p| p.0).max().unwrap();
    let y0 = pts.iter().map(|p| p.1).min().unwrap();
    let y1 = pts.iter().map(|p| p.1).max().unwrap();

    let mut rows = Vec::new();
    for y in y0..=y1 {
        let mut row = Vec::new();
        for x in x0..=x1 {
            let cell = if let Some(&c) = heart.get(&(x, y)) {
                (c, Role::Heart)
            } else if (x - cx).abs() <= 1 && (y - cy).abs() <= 1 {
                (' ', Role::Blank) // clear the chamber around the labrys
            } else if th.contains(&(x, y)) {
                (glyph(&th, x, y, pretty), Role::Thread)
            } else if wall.contains(&(x, y)) {
                (glyph(&wall, x, y, pretty), Role::Wall)
            } else {
                (' ', Role::Blank)
            };
            row.push(cell);
        }
        rows.push(row);
    }
    (rows, x1 - x0 + 1)
}

// ── colour ─────────────────────────────────────────────────────────────────────

/// Colour only if requested AND stdout is a tty AND NO_COLOR is unset.
pub fn should_color(want: bool) -> bool {
    want && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn term_256(pretty: bool) -> bool {
    if pretty { return true; }
    std::env::var("TERM").map(|t| {
        t.ends_with("256color") || t.contains("kitty") || t.contains("alacritty")
            || t.contains("foot") || t.contains("direct")
    }).unwrap_or(false)
}

fn role_code(role: Role, c256: bool) -> &'static str {
    match (role, c256) {
        (Role::Wall, true) => "\x1b[38;5;238m",
        (Role::Wall, false) => "\x1b[90m",
        (Role::Thread, true) => "\x1b[1;38;5;208m",
        (Role::Thread, false) => "\x1b[1;33m",
        (Role::Heart, true) => "\x1b[1;38;5;220m",
        (Role::Heart, false) => "\x1b[1;31m",
        (Role::Blank, _) => "",
    }
}
fn word_code(c256: bool) -> &'static str { if c256 { "\x1b[1;38;5;208m" } else { "\x1b[1;33m" } }
fn dim_code(c256: bool) -> &'static str { if c256 { "\x1b[38;5;245m" } else { "\x1b[37m" } }
const RST: &str = "\x1b[0m";

fn wrap(s: &str, code: &str, color: bool) -> String {
    if color && !code.is_empty() { format!("{code}{s}{RST}") } else { s.to_string() }
}

/// One painted line per row, each exactly `width` columns wide (visually).
fn paint_rows(rows: &[Vec<(char, Role)>], width: i32, color: bool, c256: bool, trim: bool) -> Vec<String> {
    rows.iter().map(|row| {
        let last = if trim {
            row.iter().rposition(|&(_, r)| r != Role::Blank).map(|i| i + 1).unwrap_or(0)
        } else { width as usize };
        let mut line = String::new();
        for i in 0..(width as usize) {
            if i >= last { break; }
            let (ch, role) = row[i];
            if color && role != Role::Blank {
                line.push_str(role_code(role, c256));
                line.push(ch);
                line.push_str(RST);
            } else {
                line.push(ch);
            }
        }
        line
    }).collect()
}

// ── public API ───────────────────────────────────────────────────────────────--

/// Horizontal banner: small labyrinth on the left, wordmark + tagline + version on the right.
pub fn banner(pretty: bool, want_color: bool) -> String {
    let color = should_color(want_color);
    let c256 = term_256(pretty);
    let (cells, ew) = emblem_cells(K_BANNER, pretty);
    let left = paint_rows(&cells, ew, color, c256, false); // keep full width for alignment

    let mut right: Vec<String> = WORD.iter().map(|l| wrap(l, word_code(c256), color)).collect();
    right.push(String::new());
    right.push(wrap(TAGLINE, dim_code(c256), color));
    right.push(wrap(&format!("binary packages · {VERSION}"), dim_code(c256), color));

    let h = left.len();
    let top = ((h as i32 - right.len() as i32) / 2).max(0) as usize;
    let mut rblock = vec![String::new(); top];
    rblock.extend(right);
    rblock.resize(h, String::new());

    (0..h).map(|i| {
        let line = format!("{}   {}", left[i], rblock[i]);
        line.trim_end().to_string()
    }).collect::<Vec<_>>().join("\n")
}

/// Print banner No. 2 (the labyrinth masthead) at the top of a command — only on
/// an interactive terminal, so the multi-byte art never lands in a pipe or log.
/// Colour follows the same rules as the rest of slacker (NO_COLOR / non-tty off).
pub fn show() {
    if !std::io::stdout().is_terminal() {
        return;
    }
    println!("{}", banner(false, true));
}
