/// Behavioral tuning knobs — loaded from ~/.config/mars/tuning.json.
///
/// Every knob is stored as `{ "value": ..., "description": "..." }` so that a
/// human or an agent editing the file can see what each number does. Defaults
/// are written on first run; user values are layered over defaults, so new
/// knobs appear in old files and unknown keys are ignored.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Tuning {
    pub poll_interval_ms: u64,
    pub which_key_delay_ms: u64,
    pub nudge_threshold: u32,
    pub max_panes: usize,
    pub scroll_margin: usize,
    pub page_overlap: usize,
    pub wheel_scroll_lines: usize,
    pub dropdown_max_rows: u16,
    pub panel_max_height_pct: u16,
    pub ask_panel_max_pct: u16,
    pub spinner_speed_ticks: u64,
    pub which_key_panel_width: u16,
    pub travel_panel_width: u16,
    pub binding_badge_width: usize,
    pub selection_bg: [u8; 3],
    pub search_match_bg: [u8; 3],
    // ── Theme (Mars palette: Claude-Code clay + rust) ──
    pub theme_accent: [u8; 3],
    pub theme_accent_bright: [u8; 3],
    pub theme_accent_dark: [u8; 3],
    pub theme_chip_fg: [u8; 3],
    pub theme_terminal: [u8; 3],
    /// Show the line-number gutter in editor panes (position always lives in
    /// the status bar).
    pub line_numbers: bool,
    /// Seconds before the agent may auto-name a default-named tab (0 = off).
    pub auto_name_secs: u64,
    pub agent_max_tokens: u32,
    pub agent_temperature: f64,
    pub terminal_default_rows: u16,
    pub terminal_default_cols: u16,
    pub terminal_scrollback_lines: usize,
    pub autosave_secs: u64,
    pub project_index_max: usize,
    pub project_ignore: Vec<String>,
    pub tree_width: u16,
    pub watch_quiet_secs: u64,
    pub agent_scrollback_context: usize,
    pub memory_cwd_boost: f64,
    pub memory_recency_boost: f64,
    pub memory_recency_halflife_days: f64,
    pub mission_refresh_secs: u64,
    pub worklog_max_lines: u64,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            poll_interval_ms: 16,
            which_key_delay_ms: 200,
            nudge_threshold: 3,
            max_panes: 4,
            scroll_margin: 3,
            page_overlap: 2,
            wheel_scroll_lines: 3,
            dropdown_max_rows: 20,
            panel_max_height_pct: 60,
            ask_panel_max_pct: 30,
            spinner_speed_ticks: 3,
            which_key_panel_width: 30,
            travel_panel_width: 46,
            binding_badge_width: 9,
            selection_bg: [74, 42, 31],      // deep rust-brown
            search_match_bg: [138, 84, 20],  // amber
            theme_accent: [217, 119, 87],        // #D97757 terracotta/clay
            theme_accent_bright: [233, 161, 120], // #E9A178 light sand
            theme_accent_dark: [183, 65, 14],     // #B7410E rust
            theme_chip_fg: [31, 20, 16],          // dark text on accent chips
            theme_terminal: [13, 115, 119],       // #0D7377 dark teal
            line_numbers: false,
            auto_name_secs: 45,
            agent_max_tokens: 1024, // headroom for reasoning models (Qwen3/R1)
            agent_temperature: 0.3,
            terminal_default_rows: 24,
            terminal_default_cols: 80,
            terminal_scrollback_lines: 10_000,
            autosave_secs: 30,
            project_index_max: 20_000,
            project_ignore: ["target", "node_modules", ".git", "dist", "build", ".venv"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            tree_width: 30,
            watch_quiet_secs: 20,
            agent_scrollback_context: 200,
            memory_cwd_boost: 0.25,
            memory_recency_boost: 0.15,
            memory_recency_halflife_days: 14.0,
            mission_refresh_secs: 600,
            worklog_max_lines: 4000,
        }
    }
}

impl Tuning {
    /// which-key delay expressed in frame ticks of the main loop.
    pub fn which_key_delay_ticks(&self) -> u64 {
        (self.which_key_delay_ms / self.poll_interval_ms.max(1)).max(1)
    }
}

// ── File format ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Knob {
    value: serde_json::Value,
    description: String,
}

fn knob(value: serde_json::Value, description: &str) -> Knob {
    Knob { value, description: description.to_string() }
}

/// The tunable knobs as self-contained, actionable retrieval lines — each names
/// the knob, WHERE to set it, and the default — so the agent answers
/// self-reconfiguration questions with the exact `knob = value in tuning.json`
/// rather than hallucinating the file or the knob name.
#[cfg_attr(not(feature = "memory"), allow(dead_code))] // sole consumer is the docs corpus
pub fn knob_descriptions() -> Vec<String> {
    default_knobs()
        .into_iter()
        .map(|(name, k)| {
            format!(
                "To change {}: set `{name}` in ~/.config/mars/tuning.json (default {}).",
                k.description, k.value
            )
        })
        .collect()
}

/// The default knob map, with the semantic explanations that make the file
/// safely editable by a human or an agent.
fn default_knobs() -> Vec<(&'static str, Knob)> {
    use serde_json::json;
    let d = Tuning::default();
    vec![
        ("poll_interval_ms", knob(json!(d.poll_interval_ms),
            "Main loop tick in milliseconds. Lower = snappier UI + spinner, higher CPU.")),
        ("which_key_delay_ms", knob(json!(d.which_key_delay_ms),
            "Hesitation on a prefix (C-x…) before the which-key panel pops. \
             Lower teaches eagerly; higher keeps it invisible to fast typists.")),
        ("nudge_threshold", knob(json!(d.nudge_threshold),
            "How many times an action is run from the command bar before the \
             '💡 next time: <key>' graduation hint appears in the status line.")),
        ("max_panes", knob(json!(d.max_panes),
            "Maximum panes per tab. Splits beyond this are refused.")),
        ("scroll_margin", knob(json!(d.scroll_margin),
            "Lines kept visible above/below the cursor when scrolling.")),
        ("page_overlap", knob(json!(d.page_overlap),
            "Lines of overlap kept on PageUp/PageDown so context isn't lost.")),
        ("wheel_scroll_lines", knob(json!(d.wheel_scroll_lines),
            "Lines moved per mouse-wheel step.")),
        ("dropdown_max_rows", knob(json!(d.dropdown_max_rows),
            "Maximum visible rows in the command-bar dropdown.")),
        ("panel_max_height_pct", knob(json!(d.panel_max_height_pct),
            "Maximum height of pop-up panels (dropdown) as % of the editor area.")),
        ("ask_panel_max_pct", knob(json!(d.ask_panel_max_pct),
            "Maximum height of the ask/chat panel as % of the workspace — it hugs the bottom; scroll up (Up key or wheel) for older turns.")),
        ("spinner_speed_ticks", knob(json!(d.spinner_speed_ticks),
            "Frame ticks per spinner animation step while the agent thinks. Lower = faster spin.")),
        ("which_key_panel_width", knob(json!(d.which_key_panel_width),
            "Width (columns) of the which-key continuation panel.")),
        ("travel_panel_width", knob(json!(d.travel_panel_width),
            "Width (columns) of the C-t travel-mode cheat panel.")),
        ("binding_badge_width", knob(json!(d.binding_badge_width),
            "Column width reserved for keybinding badges in the dropdown.")),
        ("selection_bg", knob(json!(d.selection_bg),
            "RGB background of the active selection highlight.")),
        ("search_match_bg", knob(json!(d.search_match_bg),
            "RGB background of isearch match highlights.")),
        ("theme_accent", knob(json!(d.theme_accent),
            "RGB brand accent (Mars terracotta): focused borders, active tab, \
             command bar, selected rows, EDIT chip.")),
        ("theme_accent_bright", knob(json!(d.theme_accent_bright),
            "RGB bright accent (sand): which-key keys, keybinding badges, \
             teaching surfaces.")),
        ("theme_accent_dark", knob(json!(d.theme_accent_dark),
            "RGB dark accent (rust): splash gradient, secondary emphasis.")),
        ("theme_chip_fg", knob(json!(d.theme_chip_fg),
            "RGB text color on accent-colored chips/badges.")),
        ("theme_terminal", knob(json!(d.theme_terminal),
            "RGB for live terminal panes: focused border + TERM mode chip.")),
        ("line_numbers", knob(json!(d.line_numbers),
            "Show the line-number gutter in editor panes. The cursor position \
             is always in the status bar, so this defaults to off for width.")),
        ("auto_name_secs", knob(json!(d.auto_name_secs),
            "With an agent configured, tabs still wearing their default numeric \
             name get an auto-generated label after this many seconds (0 = off). \
             Renaming a tab yourself always wins and opts it out.")),
        ("agent_max_tokens", knob(json!(d.agent_max_tokens),
            "Max tokens the ask-agent may generate per answer.")),
        ("agent_temperature", knob(json!(d.agent_temperature),
            "Sampling temperature for the ask-agent. Lower = more deterministic.")),
        ("terminal_default_rows", knob(json!(d.terminal_default_rows),
            "Initial PTY rows before the first render sizes the terminal pane.")),
        ("terminal_default_cols", knob(json!(d.terminal_default_cols),
            "Initial PTY columns before the first render sizes the terminal pane.")),
        ("terminal_scrollback_lines", knob(json!(d.terminal_scrollback_lines),
            "Scrollback history kept per terminal pane. Scroll with the wheel or \
             Shift+PageUp/PageDown; any keystroke snaps back to live.")),
        ("autosave_secs", knob(json!(d.autosave_secs),
            "Seconds between silent autosaves of modified buffers that have a file \
             path (also fires on session detach/disconnect). 0 disables.")),
        ("project_index_max", knob(json!(d.project_index_max),
            "Max files the `@` picker indexes from the project (bounds memory on huge \
             trees).")),
        ("project_ignore", knob(json!(d.project_ignore),
            "Directory names the file index/tree skip (plus all dotdirs). Does not yet \
             read a repo's .gitignore.")),
        ("tree_width", knob(json!(d.tree_width),
            "Column width of the left file-tree sidebar (@ / C-x d).")),
        ("watch_quiet_secs", knob(json!(d.watch_quiet_secs),
            "Seconds a watched terminal (C-t w) must be silent before Mars summarizes it \
             (W6). Also fires immediately on process exit. Generous by design — a false \
             'done' costs more than the feature earns.")),
        ("agent_scrollback_context", knob(json!(d.agent_scrollback_context),
            "Lines of a watched/focused terminal's screen sent to the agent for a summary \
             or triage.")),
        ("memory_cwd_boost", knob(json!(d.memory_cwd_boost),
            "How much a remembered command from the CURRENT working directory outranks a \
             lexical tie from elsewhere (0 = off). Same-project memories answer \
             project-specific requests.")),
        ("memory_recency_boost", knob(json!(d.memory_recency_boost),
            "How much a RECENT remembered command outranks a lexical tie from long ago \
             (0 = off); decays with memory_recency_halflife_days.")),
        ("memory_recency_halflife_days", knob(json!(d.memory_recency_halflife_days),
            "Days for the recency boost to halve. Smaller = the agent prefers this \
             week's habits; larger = long memory.")),
        ("mission_refresh_secs", knob(json!(d.mission_refresh_secs),
            "How often (at most) the agent re-infers your one-line mission from the \
             work journal of watch verdicts; shown by `mars ls`. 0 disables.")),
        ("worklog_max_lines", knob(json!(d.worklog_max_lines),
            "Work-journal size bound (~/.mars/worklog.jsonl): past twice this many \
             lines it is compacted to the newest this-many at startup. 0 = never.")),
    ]
}

// ── load() ────────────────────────────────────────────────────────────────────

pub fn tuning_path() -> Option<std::path::PathBuf> {
    crate::config::state_path().map(|p| p.with_file_name("tuning.json"))
}

/// Write the annotated default knobs to `path` (used on first run and reset).
fn write_default_knobs(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let map: serde_json::Map<String, serde_json::Value> = default_knobs()
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::to_value(v).unwrap()))
        .collect();
    if let Ok(json) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, json);
    }
}

/// Restore tuning.json to defaults, backing up the current file.
pub fn reset() {
    if let Some(p) = tuning_path() {
        if p.exists() {
            let _ = std::fs::rename(&p, p.with_extension("json.bak"));
        }
        write_default_knobs(&p);
    }
}

pub fn load() -> Tuning {
    let path = tuning_path();
    let user: Option<HashMap<String, Knob>> = path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok());

    if user.is_none() {
        // First run: write the annotated defaults so they're discoverable/editable.
        if let Some(p) = &path {
            write_default_knobs(p);
        }
    }

    let mut t = Tuning::default();
    if let Some(map) = user {
        let get_u64 = |m: &HashMap<String, Knob>, k: &str, d: u64| {
            m.get(k).and_then(|e| e.value.as_u64()).unwrap_or(d)
        };
        let get_f64 = |m: &HashMap<String, Knob>, k: &str, d: f64| {
            m.get(k).and_then(|e| e.value.as_f64()).unwrap_or(d)
        };
        let get_rgb = |m: &HashMap<String, Knob>, k: &str, d: [u8; 3]| {
            m.get(k)
                .and_then(|e| e.value.as_array())
                .and_then(|a| {
                    if a.len() == 3 {
                        Some([a[0].as_u64()? as u8, a[1].as_u64()? as u8, a[2].as_u64()? as u8])
                    } else {
                        None
                    }
                })
                .unwrap_or(d)
        };
        t.poll_interval_ms      = get_u64(&map, "poll_interval_ms", t.poll_interval_ms).max(1);
        t.which_key_delay_ms    = get_u64(&map, "which_key_delay_ms", t.which_key_delay_ms);
        t.nudge_threshold       = get_u64(&map, "nudge_threshold", t.nudge_threshold as u64) as u32;
        t.max_panes             = get_u64(&map, "max_panes", t.max_panes as u64) as usize;
        t.scroll_margin         = get_u64(&map, "scroll_margin", t.scroll_margin as u64) as usize;
        t.page_overlap          = get_u64(&map, "page_overlap", t.page_overlap as u64) as usize;
        t.wheel_scroll_lines    = get_u64(&map, "wheel_scroll_lines", t.wheel_scroll_lines as u64) as usize;
        t.dropdown_max_rows     = get_u64(&map, "dropdown_max_rows", t.dropdown_max_rows as u64) as u16;
        t.panel_max_height_pct  = get_u64(&map, "panel_max_height_pct", t.panel_max_height_pct as u64) as u16;
        t.ask_panel_max_pct     = get_u64(&map, "ask_panel_max_pct", t.ask_panel_max_pct as u64) as u16;
        t.spinner_speed_ticks   = get_u64(&map, "spinner_speed_ticks", t.spinner_speed_ticks).max(1);
        t.which_key_panel_width = get_u64(&map, "which_key_panel_width", t.which_key_panel_width as u64) as u16;
        t.travel_panel_width    = get_u64(&map, "travel_panel_width", t.travel_panel_width as u64) as u16;
        t.binding_badge_width   = get_u64(&map, "binding_badge_width", t.binding_badge_width as u64) as usize;
        t.selection_bg          = get_rgb(&map, "selection_bg", t.selection_bg);
        t.search_match_bg       = get_rgb(&map, "search_match_bg", t.search_match_bg);
        t.theme_accent          = get_rgb(&map, "theme_accent", t.theme_accent);
        t.theme_accent_bright   = get_rgb(&map, "theme_accent_bright", t.theme_accent_bright);
        t.theme_accent_dark     = get_rgb(&map, "theme_accent_dark", t.theme_accent_dark);
        t.theme_chip_fg         = get_rgb(&map, "theme_chip_fg", t.theme_chip_fg);
        t.theme_terminal        = get_rgb(&map, "theme_terminal", t.theme_terminal);
        t.line_numbers = map
            .get("line_numbers")
            .and_then(|e| e.value.as_bool())
            .unwrap_or(t.line_numbers);
        t.auto_name_secs = get_u64(&map, "auto_name_secs", t.auto_name_secs);
        t.agent_max_tokens      = get_u64(&map, "agent_max_tokens", t.agent_max_tokens as u64) as u32;
        t.agent_temperature     = get_f64(&map, "agent_temperature", t.agent_temperature);
        t.terminal_default_rows = get_u64(&map, "terminal_default_rows", t.terminal_default_rows as u64) as u16;
        t.terminal_default_cols = get_u64(&map, "terminal_default_cols", t.terminal_default_cols as u64) as u16;
        t.terminal_scrollback_lines =
            get_u64(&map, "terminal_scrollback_lines", t.terminal_scrollback_lines as u64) as usize;
        t.autosave_secs = get_u64(&map, "autosave_secs", t.autosave_secs);
        t.project_index_max = get_u64(&map, "project_index_max", t.project_index_max as u64) as usize;
        t.tree_width = get_u64(&map, "tree_width", t.tree_width as u64) as u16;
        t.watch_quiet_secs = get_u64(&map, "watch_quiet_secs", t.watch_quiet_secs);
        t.agent_scrollback_context =
            get_u64(&map, "agent_scrollback_context", t.agent_scrollback_context as u64) as usize;
        t.memory_cwd_boost = get_f64(&map, "memory_cwd_boost", t.memory_cwd_boost);
        t.memory_recency_boost = get_f64(&map, "memory_recency_boost", t.memory_recency_boost);
        t.memory_recency_halflife_days =
            get_f64(&map, "memory_recency_halflife_days", t.memory_recency_halflife_days);
        t.mission_refresh_secs = get_u64(&map, "mission_refresh_secs", t.mission_refresh_secs);
        t.worklog_max_lines = get_u64(&map, "worklog_max_lines", t.worklog_max_lines);
        if let Some(list) = map.get("project_ignore").and_then(|e| e.value.as_array()) {
            let dirs: Vec<String> =
                list.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            if !dirs.is_empty() {
                t.project_ignore = dirs;
            }
        }
    }
    t
}
