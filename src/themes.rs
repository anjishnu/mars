//! Color themes: resolve a named theme into a `Palette` of semantic tokens.
//!
//! Themes are flat JSON token→color maps. Bundled themes are embedded at compile
//! time (mirroring `prompts.rs`); user themes live in `~/.mars/themes/*.json` and
//! shadow bundled ones of the same name. Every token value is either `#rrggbb` or a
//! named terminal color (`white gray darkgray black red green yellow blue … reset`).
//! An under-specified theme falls back per-token to Mission Control.

use ratatui::style::Color;
use serde_json::Value;

use crate::tuning::Palette;

const MISSION_CONTROL: &str = include_str!("themes/mission-control.json");
const ECLIPSE: &str = include_str!("themes/eclipse.json");
const PAPER: &str = include_str!("themes/paper.json");
const HACKER: &str = include_str!("themes/hacker.json");

/// (name, embedded JSON) for every bundled theme, in display order.
const BUNDLED: &[(&str, &str)] = &[
    ("mission-control", MISSION_CONTROL),
    ("eclipse", ECLIPSE),
    ("paper", PAPER),
    ("hacker", HACKER),
];

/// Parse a token value: `#rrggbb` hex, or a named terminal color.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    Some(match s.to_ascii_lowercase().as_str() {
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "black" => Color::Black,
        "reset" => Color::Reset,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "cyan" => Color::Cyan,
        "magenta" => Color::Magenta,
        _ => return None,
    })
}

/// Best-effort RGB for a resolved color — for consumers that need raw bytes (the
/// termimad Markdown skin). Named neutrals map to sensible approximations.
pub fn rgb_of(c: Color) -> [u8; 3] {
    match c {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::White => [229, 229, 229],
        Color::Gray => [150, 150, 150],
        Color::DarkGray => [96, 96, 96],
        Color::Black => [0, 0, 0],
        Color::Reset => [180, 180, 180],
        Color::Red => [205, 49, 49],
        Color::Green => [61, 174, 114],
        Color::Yellow => [229, 192, 123],
        Color::Blue => [59, 142, 234],
        Color::Cyan => [41, 184, 219],
        Color::Magenta => [214, 112, 214],
        _ => [200, 200, 200],
    }
}

fn overlay(p: &mut Palette, j: &Value) {
    let set = |cur: &mut Color, key: &str| {
        if let Some(c) = j.get(key).and_then(|v| v.as_str()).and_then(parse_color) {
            *cur = c;
        }
    };
    set(&mut p.accent, "accent");
    set(&mut p.accent_bright, "accent-bright");
    set(&mut p.accent_dark, "accent-dark");
    set(&mut p.on_accent, "on-accent");
    set(&mut p.info, "info");
    set(&mut p.success, "success");
    set(&mut p.warning, "warning");
    set(&mut p.danger, "danger");
    set(&mut p.text, "text");
    set(&mut p.text_dim, "text-dim");
    set(&mut p.text_faint, "text-faint");
    set(&mut p.border, "border");
    set(&mut p.surface, "surface");
    set(&mut p.select_row_bg, "select-row-bg");
    set(&mut p.selection_bg, "selection-bg");
    set(&mut p.search_bg, "search-bg");
    set(&mut p.current_line, "current-line");
}

/// The raw JSON for a theme by name: a user file (`~/.mars/themes/<name>.json`)
/// shadows a bundled theme of the same name.
fn theme_json(name: &str) -> Option<Value> {
    if let Some(dir) = crate::sys::paths::home_dir() {
        let p = dir.join(".mars").join("themes").join(format!("{name}.json"));
        if let Ok(s) = std::fs::read_to_string(&p) {
            return serde_json::from_str(&s).ok();
        }
    }
    BUNDLED
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, src)| serde_json::from_str(src).ok())
}

/// Resolve a theme name into a full palette. `None`/empty/"mission-control" → the
/// compiled default. An unknown name also falls back to the default (never panics).
pub fn resolve(name: Option<&str>) -> Palette {
    let mut pal = Palette::mission_control();
    let name = match name {
        Some(n) if !n.is_empty() => n,
        _ => return pal,
    };
    if name == "mission-control" {
        return pal;
    }
    if let Some(j) = theme_json(name) {
        overlay(&mut pal, &j);
    }
    pal
}

/// One entry in `mars theme list`.
pub struct ThemeInfo {
    pub name: String,
    pub about: String,
    pub dark: bool,
    pub user: bool,
}

/// Every available theme: bundled first (in order), then any user themes in
/// `~/.mars/themes/` not shadowing a bundled name.
pub fn list() -> Vec<ThemeInfo> {
    let mut out: Vec<ThemeInfo> = Vec::new();
    let info = |name: &str, j: &Value, user: bool| ThemeInfo {
        name: name.to_string(),
        about: j.get("about").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        dark: j.get("dark").and_then(|v| v.as_bool()).unwrap_or(true),
        user,
    };
    for (name, src) in BUNDLED {
        if let Ok(j) = serde_json::from_str::<Value>(src) {
            out.push(info(name, &j, false));
        }
    }
    if let Some(dir) = crate::sys::paths::home_dir() {
        let tdir = dir.join(".mars").join("themes");
        if let Ok(rd) = std::fs::read_dir(&tdir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let name = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if out.iter().any(|t| t.name == name) {
                    continue; // a user file shadowing a bundled name — already listed
                }
                if let Ok(s) = std::fs::read_to_string(&path) {
                    if let Ok(j) = serde_json::from_str::<Value>(&s) {
                        out.push(info(&name, &j, true));
                    }
                }
            }
        }
    }
    out
}

/// Whether a theme name resolves (bundled or a user file) — for the CLI to reject
/// unknown names before writing config.
pub fn exists(name: &str) -> bool {
    name == "mission-control" || theme_json(name).is_some()
}
