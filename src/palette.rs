/// The action palette behind the command bar: a fuzzy-searchable dropdown
/// that is the single "how do I do X?" entry point. Opened by Ctrl+Space / M-x.

use std::collections::HashMap;

// ── Actions ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Action {
    // windows / panes
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    DeleteOtherWindows,
    NextPane,
    PrevPane,
    SwapPane,
    ZoomPane,
    RenamePane,
    // tabs
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    MoveTabLeft,
    MoveTabRight,
    RenameTab,
    TabMode,
    // files / buffers
    Save,
    FindFile,
    QuickOpen,
    ToggleFileTree,
    RefreshIndex,
    RestoreKeybindings,
    SwitchBuffer,
    KillBuffer,
    // edit
    Undo,
    Redo,
    UndoMode,
    KillLine,
    KillRegion,
    CopyRegion,
    Yank,
    YankPop,
    Paste,
    KillWordForward,
    KillWordBackward,
    SelectAll,
    GoTop,
    GoBottom,
    GotoLine,
    JumpBlockPrev,
    JumpBlockNext,
    JumpSymbolPrev,
    JumpSymbolNext,
    MatchBracket,
    Recenter,
    // search / terminal / agent / app
    Search,
    SearchBackward,
    QueryReplace,
    OpenTerminal,
    AskAgent,
    /// Ask the agent to explain what's on screen at the cursor.
    ExplainThis,
    /// Triage: "why did this fail?" grounded in the focused terminal.
    ExplainFailure,
    /// W6: watch this terminal — summarize it when it goes quiet or exits.
    WatchPane,
    /// Leave the session running and disconnect this client.
    Detach,
    RenameSession,
    Quit,
}

impl Action {
    /// Resolve an LLM `RUN: <Name>` directive back into an `Action`.
    pub fn from_name(name: &str) -> Option<Action> {
        serde_json::from_str::<Action>(&format!("{:?}", name.trim())).ok()
    }

    /// Short human label — used by which-key hints and the graduation nudge.
    pub fn label(&self) -> &'static str {
        match self {
            Action::SplitHorizontal    => "split below",
            Action::SplitVertical      => "split right",
            Action::ClosePane          => "close pane",
            Action::DeleteOtherWindows => "only this pane",
            Action::NextPane           => "other pane",
            Action::PrevPane           => "prev pane",
            Action::SwapPane           => "move pane",
            Action::ZoomPane           => "zoom pane",
            Action::RenamePane         => "rename pane",
            Action::NewTab             => "new tab",
            Action::CloseTab           => "close tab",
            Action::NextTab            => "next tab",
            Action::PrevTab            => "prev tab",
            Action::MoveTabLeft        => "move tab left",
            Action::MoveTabRight       => "move tab right",
            Action::RenameTab          => "rename tab",
            Action::TabMode            => "tab mode",
            Action::Save               => "save",
            Action::FindFile           => "open file",
            Action::QuickOpen          => "go to file",
            Action::ToggleFileTree     => "toggle file tree",
            Action::RefreshIndex       => "refresh file index",
            Action::RestoreKeybindings => "restore default keybindings",
            Action::SwitchBuffer       => "switch buffer",
            Action::KillBuffer         => "kill buffer",
            Action::Undo               => "undo",
            Action::Redo               => "redo",
            Action::UndoMode           => "time-travel (undo history)",
            Action::KillLine           => "kill line",
            Action::KillRegion         => "kill region",
            Action::CopyRegion         => "copy region",
            Action::Yank               => "yank",
            Action::YankPop            => "yank pop",
            Action::Paste              => "paste",
            Action::KillWordForward    => "kill word",
            Action::KillWordBackward   => "kill word back",
            Action::SelectAll          => "select all",
            Action::GoTop              => "top of file",
            Action::GoBottom           => "bottom of file",
            Action::GotoLine           => "go to line",
            Action::JumpBlockPrev      => "previous block",
            Action::JumpBlockNext      => "next block",
            Action::JumpSymbolPrev     => "previous definition",
            Action::JumpSymbolNext     => "next definition",
            Action::MatchBracket       => "matching bracket",
            Action::Recenter           => "recenter",
            Action::Search             => "search",
            Action::QueryReplace       => "search & replace",
            Action::SearchBackward     => "search back",
            Action::OpenTerminal       => "terminal",
            Action::AskAgent           => "ask agent",
            Action::ExplainThis        => "explain this",
            Action::ExplainFailure     => "why did this fail?",
            Action::WatchPane          => "watch this pane",
            Action::Detach             => "detach session",
            Action::RenameSession      => "rename session",
            Action::Quit               => "quit",
        }
    }

    /// Destructive actions the agent's `RUN:` directive must confirm first.
    pub fn is_destructive(&self) -> bool {
        matches!(self, Action::Quit | Action::CloseTab | Action::KillBuffer | Action::ClosePane)
    }
}

// ── Menu structure ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ItemKind {
    Run(Action),
    Submenu(&'static str), // name passed to menu_for()
}

pub struct MenuItem {
    pub label: &'static str,
    pub kind: ItemKind,
    pub description: &'static str,
}

impl MenuItem {
    fn run_desc(label: &'static str, action: Action, description: &'static str) -> Self {
        MenuItem { label, kind: ItemKind::Run(action), description }
    }
    fn sub(label: &'static str, name: &'static str) -> Self {
        MenuItem { label, kind: ItemKind::Submenu(name), description: "Open submenu" }
    }
}

// Keybindings are NOT hardcoded here — the UI shows each row's live binding
// via `KeyBindings::binding_for`, so hints stay honest after a remap.
fn root_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::run_desc("Save",          Action::Save,           "Save the current buffer"),
        MenuItem::run_desc("File tree",     Action::ToggleFileTree, "Browse/filter project files in the left sidebar (also @)"),
        MenuItem::run_desc("Open file…",    Action::FindFile,       "Open a file by path"),
        MenuItem::run_desc("Switch buffer", Action::SwitchBuffer,   "Switch to another open buffer"),
        MenuItem::run_desc("Search",        Action::Search,       "Incremental search in this buffer"),
        MenuItem::run_desc("Search & replace…", Action::QueryReplace, "Find and replace, stepping y/n through each match"),
        MenuItem::run_desc("Undo",          Action::Undo,         "Undo the last edit"),
        MenuItem::run_desc("Redo",          Action::Redo,         "Redo the undone edit"),
        MenuItem::run_desc("Time-travel…",  Action::UndoMode,     "Scrub edit history: ←/→ back and forward, Home to the start (C-u)"),
        MenuItem::run_desc("Paste",         Action::Paste,        "Paste from the system clipboard"),
        MenuItem::run_desc("Select all",    Action::SelectAll,    "Select the whole buffer"),
        MenuItem::sub("Edit ▸",             "edit"),
        MenuItem::sub("Window ▸",           "window"),
        MenuItem::sub("Tab ▸",              "tab"),
        MenuItem::sub("Go ▸",               "go"),
        MenuItem::run_desc("Open terminal", Action::OpenTerminal, "Open or re-attach a shell in this pane"),
        MenuItem::run_desc("Ask agent",     Action::AskAgent,     "Ask the LLM how to do something"),
        MenuItem::run_desc("Explain this",  Action::ExplainThis,  "Explain what's on screen at the cursor"),
        MenuItem::run_desc("Why did this fail?", Action::ExplainFailure, "Triage the error in the focused terminal"),
        MenuItem::run_desc("Watch this pane", Action::WatchPane, "Summarize this terminal when it goes quiet or exits (even detached)"),
        MenuItem::run_desc("Detach session", Action::Detach,      "Disconnect; the session keeps running (reattach: mars attach)"),
        MenuItem::run_desc("Rename session", Action::RenameSession, "Rename this session (also: mars rename <old> <new>)"),
        MenuItem::run_desc("Refresh file index", Action::RefreshIndex, "Re-scan the project for the file tree/picker"),
        MenuItem::run_desc("Restore default keys", Action::RestoreKeybindings, "Reset keybindings to defaults (backs up keys.json)"),
        MenuItem::run_desc("Quit",          Action::Quit,         "Quit the editor (ends the session)"),
    ]
}

fn edit_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::run_desc("Cut / kill region", Action::KillRegion,     "Cut the selection to the kill-ring"),
        MenuItem::run_desc("Copy region",       Action::CopyRegion,     "Copy the selection (or line) to the kill-ring + clipboard"),
        MenuItem::run_desc("Yank (paste)",      Action::Yank,           "Paste the last kill from the kill-ring"),
        MenuItem::run_desc("Yank-pop",          Action::YankPop,        "Cycle to an earlier kill after a yank"),
        MenuItem::run_desc("Kill to line end",  Action::KillLine,       "Cut from the cursor to the end of the line"),
        MenuItem::run_desc("Kill word forward", Action::KillWordForward, "Cut the word after the cursor"),
        MenuItem::run_desc("Kill word back",    Action::KillWordBackward, "Cut the word before the cursor"),
    ]
}

fn window_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::run_desc("Split below ─",  Action::SplitHorizontal,    "Split the pane below"),
        MenuItem::run_desc("Split right │",  Action::SplitVertical,      "Split the pane right"),
        MenuItem::run_desc("Other window",   Action::NextPane,           "Focus the next pane (also Ctrl-arrows)"),
        MenuItem::run_desc("Previous window", Action::PrevPane,          "Focus the previous pane"),
        MenuItem::run_desc("Zoom pane",      Action::ZoomPane,           "Maximize this pane / restore (also C-t z)"),
        MenuItem::run_desc("Move pane",      Action::SwapPane,           "Swap this pane with the next"),
        MenuItem::run_desc("Rename pane",    Action::RenamePane,         "Set a custom title for this pane"),
        MenuItem::run_desc("Only this",      Action::DeleteOtherWindows, "Close the other panes"),
        MenuItem::run_desc("Close pane",     Action::ClosePane,          "Close this pane"),
    ]
}

fn tab_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::run_desc("New tab",        Action::NewTab,       "Open a new tab with a scratch buffer"),
        MenuItem::run_desc("Close tab",      Action::CloseTab,     "Close the current tab"),
        MenuItem::run_desc("Rename tab",     Action::RenameTab,    "Name this tab (also r in C-t travel mode)"),
        MenuItem::run_desc("Next tab",       Action::NextTab,      "Switch to the next tab (also M-1..9)"),
        MenuItem::run_desc("Prev tab",       Action::PrevTab,      "Switch to the previous tab"),
        MenuItem::run_desc("Move tab right", Action::MoveTabRight, "Reorder: move this tab right"),
        MenuItem::run_desc("Move tab left",  Action::MoveTabLeft,  "Reorder: move this tab left"),
    ]
}

fn go_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::run_desc("Top of file",    Action::GoTop,    "Jump to the first line"),
        MenuItem::run_desc("Bottom of file", Action::GoBottom, "Jump to the last line"),
        MenuItem::run_desc("Go to line…",    Action::GotoLine, "Jump to a line number"),
        MenuItem::run_desc("Next definition", Action::JumpSymbolNext, "Jump to the next fn/def/class"),
        MenuItem::run_desc("Prev definition", Action::JumpSymbolPrev, "Jump to the previous fn/def/class"),
        MenuItem::run_desc("Next block",      Action::JumpBlockNext,  "Jump to the next blank line"),
        MenuItem::run_desc("Prev block",      Action::JumpBlockPrev,  "Jump to the previous blank line"),
        MenuItem::run_desc("Matching bracket", Action::MatchBracket,  "Jump to the matching ( [ {"),
        MenuItem::run_desc("Recenter",       Action::Recenter, "Center the view on the cursor"),
    ]
}

pub fn menu_for(name: &str) -> Vec<MenuItem> {
    match name {
        "edit"   => edit_menu(),
        "window" => window_menu(),
        "tab"    => tab_menu(),
        "go"     => go_menu(),
        _        => root_menu(),
    }
}

/// Flattened list of all leaf (Run) items used for fuzzy search.
fn all_items() -> Vec<(String, ItemKind, String)> {
    let mut result = Vec::new();
    for item in root_menu() {
        match item.kind.clone() {
            ItemKind::Run(_) => {
                result.push((item.label.to_string(), item.kind, item.description.to_string()));
            }
            ItemKind::Submenu(sub_name) => {
                for subitem in menu_for(sub_name) {
                    result.push((subitem.label.to_string(), subitem.kind, subitem.description.to_string()));
                }
            }
        }
    }
    result
}

/// A textual catalog of every runnable action (name + description) that the
/// LLM agent receives as context, so its answers cite real Ares commands and
/// can emit a matching `RUN: <Name>` directive.
pub fn registry_context() -> String {
    let mut out = String::new();
    for (label, kind, description) in all_items() {
        if let ItemKind::Run(a) = kind {
            out.push_str(&format!("- {:?}: {} — {}\n", a, label.trim(), description));
        }
    }
    out
}

/// Frecency weight for a row (0 for submenus / never-used actions).
fn row_frecency(row: &PaletteRow, frecency: &HashMap<String, u32>) -> u32 {
    if let ItemKind::Run(a) = &row.kind {
        *frecency.get(&format!("{:?}", a)).unwrap_or(&0)
    } else {
        0
    }
}

// ── Fuzzy scoring ────────────────────────────────────────────────────────────

/// Returns Some(score) if every char in `query` appears (in order) in
/// `candidate`; None if no subsequence match.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let c: Vec<char> = candidate.to_lowercase().chars().collect();

    let mut score: i64 = 0;
    let mut qi = 0;
    let mut prev_matched = false;

    for (ci, &ch) in c.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if ch == q[qi] {
            score += 1;
            if prev_matched {
                score += 2; // contiguous run bonus
            }
            if ci == 0 || c[ci - 1] == ' ' || c[ci - 1] == '_' {
                score += 3; // word-boundary bonus
            }
            qi += 1;
            prev_matched = true;
        } else {
            prev_matched = false;
        }
    }

    if qi < q.len() {
        None
    } else {
        Some(score)
    }
}

// ── Bar mode ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum BarMode {
    Command,
    Ask,
    /// `!` prefix — the query is a shell command to run in the terminal pane.
    Shell,
}

// ── Palette state ─────────────────────────────────────────────────────────────

pub struct PaletteRow {
    pub label: String,
    pub kind: ItemKind,
    pub description: String,
}

pub struct Palette {
    /// Submenu breadcrumb — "root" is always the first entry.
    pub stack: Vec<&'static str>,
    pub query: String,
    pub selected: usize,
    pub bar_mode: BarMode,
}

impl Palette {
    pub fn root() -> Self {
        Palette { stack: vec!["root"], query: String::new(), selected: 0, bar_mode: BarMode::Command }
    }

    pub fn current_menu(&self) -> &'static str {
        self.stack.last().copied().unwrap_or("root")
    }

    pub fn title(&self) -> String {
        if self.stack.len() <= 1 {
            " ⌕  actions ".to_string()
        } else {
            let crumbs: Vec<&str> = self.stack.iter().copied().skip(1).collect();
            format!(" {} ▸ ", crumbs.join(" ▸ ").to_uppercase())
        }
    }

    /// Items to display given current menu level + query text.
    /// Empty query → **fixed** curated order (positional memory can form).
    /// Non-empty → fuzzy score, with frecency as a tiebreaker.
    pub fn visible_items(&self, frecency: &HashMap<String, u32>) -> Vec<PaletteRow> {
        if self.query.is_empty() {
            // Fixed order — do NOT reorder by frecency (spatial stability, §2.2).
            menu_for(self.current_menu())
                .into_iter()
                .map(|item| PaletteRow {
                    label: item.label.to_string(),
                    kind: item.kind,
                    description: item.description.to_string(),
                })
                .collect()
        } else {
            let mut scored: Vec<(i64, u32, PaletteRow)> = all_items()
                .into_iter()
                .filter_map(|(label, kind, description)| {
                    fuzzy_score(&self.query, &label).map(|s| {
                        let row = PaletteRow { label, kind, description };
                        let f = row_frecency(&row, frecency);
                        (s, f, row)
                    })
                })
                .collect();
            // Fuzzy score first, frecency as tiebreaker.
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            scored.into_iter().map(|(_, _, row)| row).collect()
        }
    }

    /// Move selection up (wrapping).
    pub fn select_up(&mut self, total: usize) {
        if total == 0 {
            return;
        }
        self.selected = if self.selected == 0 { total - 1 } else { self.selected - 1 };
    }

    /// Move selection down (wrapping).
    pub fn select_down(&mut self, total: usize) {
        if total == 0 {
            return;
        }
        self.selected = (self.selected + 1) % total;
    }

    /// Push into a submenu, reset query + selection.
    pub fn push(&mut self, name: &'static str) {
        self.stack.push(name);
        self.query.clear();
        self.selected = 0;
    }

    /// Pop one level. Returns `true` if the palette should stay open, `false` if it should close.
    pub fn pop(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.query.clear();
            self.selected = 0;
            true
        } else {
            false // at root — caller should close
        }
    }
}
