use std::{collections::HashMap, io, sync::mpsc, time::Duration};

use anyhow::Result;
use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};

use crate::{
    agent::{self, AgentEvent},
    buffer::{Buffer, BufferId},
    config::{self, chord_of, KeyBindings, KeyChord},
    layout::PaneLayout,
    mode::Mode,
    palette::{self, Action, BarMode, ItemKind, Palette},
    pane::{Pane, PaneContent, PaneId},
    project,
    tab::{Tab, TabId},
    terminal::{self, Term, TermEvent, TermId},
    tuning::{self, Tuning},
    ui,
};

/// One unit of user input, source-agnostic: the real TTY in standalone mode,
/// or deserialized frames from a session client.
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    /// New client viewport size — handled by the session server (standalone
    /// mode relies on ratatui autoresize).
    Resize(u16, u16),
}

/// The left file-tree sidebar's state (@ / C-x d).
pub struct FileTree {
    /// Directory the tree is rooted at (`../` re-roots to the parent).
    pub root: std::path::PathBuf,
    /// Folders the user has expanded (full paths).
    pub expanded: std::collections::HashSet<std::path::PathBuf>,
    pub selected: usize,
    /// Type-to-filter query; non-empty switches the sidebar to a fuzzy shortlist.
    pub filter: String,
}

/// One flattened, visible line in the tree sidebar.
pub struct TreeRow {
    pub path: std::path::PathBuf,
    pub label: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// The `../` go-up row.
    pub updir: bool,
}

/// A minibuffer prompt (find-file, switch-buffer, incremental search).
#[derive(Clone)]
pub struct Prompt {
    pub label: String,
    pub input: String,
    pub kind: PromptKind,
}

#[derive(Clone, PartialEq)]
pub enum PromptKind {
    SaveAs,
    GotoLine,
    RenameTab,
    RenamePane,
    RenameSession,
    /// Live incremental search (C-s / C-r navigate, Enter accepts, C-g restores).
    Search,
    /// Quit with modified buffers: s = save all & quit, q = quit anyway.
    ConfirmQuit,
    /// Confirm a destructive agent-proposed action: y runs it, anything else cancels.
    ConfirmAction(Action),
}

/// Per-terminal watch state (W6): the daemon summarizes a watched pane when it
/// goes quiet or its process exits — even while you're detached.
#[derive(Default)]
pub struct WatchState {
    pub watched: bool,
    pub last_output_tick: u64,
    /// Quiet/exit already fired → don't re-fire until new output arrives.
    pub triggered: bool,
    /// The last one-line verdict (kept for the W7 reattach diff later).
    pub verdict: Option<String>,
}

/// Why a watch fired.
#[derive(Clone, Copy)]
pub enum WatchReason { Exit, Quiet }

/// A pull-rendered proactive notice — the agent's only path to the screen. The
/// renderer reads it; the agent never pushes. Failures sort before info.
pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
}

#[derive(PartialEq, PartialOrd, Eq, Ord)]
pub enum NoticeKind { Failure, Info }

/// A cheap counts-and-flags snapshot taken at detach; diffed at reattach (W7).
/// Deterministic — the facts (what exited, what changed) are the value; no LLM.
#[derive(Default)]
pub struct Snapshot {
    exited: std::collections::HashSet<TermId>,
    dirty: std::collections::HashSet<String>,
    verdicts: HashMap<TermId, String>,
}

pub struct App {
    pub buffers: HashMap<BufferId, Buffer>,
    pub panes: HashMap<PaneId, Pane>,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub mode: Mode,
    pub palette: Option<Palette>,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub keys: KeyBindings,
    pub frecency: HashMap<String, u32>,
    // ── Non-modal editing state ──
    pub pending_prefix: Vec<KeyChord>,
    /// frame_tick when the prefix was armed — which-key pops after a short delay.
    pub prefix_tick: u64,
    pub prompt: Option<Prompt>,
    pub kill_ring: Vec<String>,
    /// (buffer, start char idx, len, kill_ring index) of the last yank — M-y target.
    last_yank: Option<(BufferId, usize, usize, usize)>,
    // ── Incremental search ──
    pub search_origin: Option<(usize, usize)>,
    /// Highlighted matches as (row, col, len) — rendered like selections.
    pub search_hl: Vec<(usize, usize, usize)>,
    /// Teleport labels over matches (row, col, label) while picking (Tab).
    pub search_labels: Vec<(usize, usize, char)>,
    /// True when the next key selects a labeled match instead of extending the query.
    pub search_pick: bool,
    // ── Command bar ──
    /// Mode to return to when the bar closes (Terminal keeps its focus).
    pub bar_return: Mode,
    /// Per-action bar-invocation counts — drives the graduation nudge.
    pub bar_uses: HashMap<String, u32>,
    // ── Mouse ──
    /// Pane screen rects from the last render (pane_id, rect).
    pub pane_rects: Vec<(PaneId, Rect)>,
    /// Focused pane's cursor screen position from the last render — anchors the
    /// W3 shell-translate overlay.
    pub cursor_screen: Option<(u16, u16)>,
    // ── System clipboard (None if unavailable, e.g. headless) ──
    clipboard: Option<arboard::Clipboard>,
    // ── Behavioral tuning knobs (~/.config/mars/tuning.json) ──
    pub tuning: Tuning,
    /// Show the MARS banner in the empty scratch until the first keypress.
    pub show_splash: bool,
    /// Directory new terminals open in — the parent of the first opened file.
    startup_cwd: Option<std::path::PathBuf>,
    /// Directory `mars` was launched from — the terminal's cwd when no file set one.
    run_cwd: Option<std::path::PathBuf>,
    /// Lazily-built project file index (feeds the tree's type-to-filter).
    project_index: Option<project::Index>,
    /// How often each file has been opened — ranks the filter shortlist.
    pub file_frecency: HashMap<String, u32>,
    /// Left file-tree sidebar (@ / C-x d); visible whenever `tree_open`.
    pub file_tree: Option<FileTree>,
    pub tree_open: bool,
    /// Flattened visible rows, recomputed on every tree mutation.
    pub tree_rows: Vec<TreeRow>,
    // ── Session (daemon) state ──
    /// Set when running inside a session daemon (`mars --session <name>`).
    pub session_name: Option<String>,
    /// Action::Detach sets this; the session server consumes it.
    pub detach_requested: bool,
    /// Action::RenameSession sets this; the session server consumes it.
    pub rename_session_to: Option<String>,
    // ── LLM agent ──
    pub agent_tx: mpsc::Sender<AgentEvent>,
    pub agent_rx: mpsc::Receiver<AgentEvent>,
    pub agent_pending: bool,
    /// Transient notices only (errors, no-key) — answers live in the history.
    pub agent_answer: Option<String>,
    /// Confirm-gated action the model proposed (RUN:/TYPE: directive).
    pub agent_directive: Option<agent::AgentDirective>,
    /// The selection (buf, start, end) captured when an agent query was asked —
    /// the target a proposed refactor would replace.
    pub refactor_target: Option<(BufferId, usize, usize)>,
    /// A code-block the agent returned to replace `refactor_target` (confirm-gated).
    pub refactor_replacement: Option<String>,
    /// The last question asked, replayed verbatim when the model emits a `NEED:`.
    last_question: String,
    /// How many `NEED:` expansions this ask has done (hard cap 1 — never a loop).
    need_depth: u8,
    // ── Watch / notices (W6) ──
    /// Per-terminal watch state, keyed by TermId.
    pub watches: HashMap<TermId, WatchState>,
    /// Proactive notices the renderer reads (failures first). The agent can only append.
    pub notices: Vec<Notice>,
    /// An exit trigger queued from the term_rx drain, fired next tick.
    pending_watch: Option<(TermId, WatchReason)>,
    /// State captured at detach; diffed on reattach for the "where was I?" briefing (W7).
    detach_snapshot: Option<Snapshot>,
    /// The conversation: ("user"/"assistant", text). Survives bar close; C-l clears.
    pub agent_history: Vec<(String, String)>,
    /// Ask-panel scroll: lines scrolled up from the bottom of the transcript.
    pub ask_scroll: usize,
    /// Auto-naming state: one request in flight; tabs already tried.
    bg_busy: bool,
    auto_name_attempted: std::collections::HashSet<TabId>,
    /// Shell composer: the query is a ready-to-run command (translated or
    /// typed literally with no key) — the next Enter runs it.
    pub shell_ready: bool,
    /// Session auto-naming: fired once per still-numeric session.
    session_name_attempted: bool,
    pub frame_tick: u64,
    // ── Terminal panes ──
    pub terms: HashMap<TermId, Term>,
    pub term_tx: mpsc::Sender<TermEvent>,
    pub term_rx: mpsc::Receiver<TermEvent>,
    next_buffer_id: usize,
    next_pane_id: usize,
    next_tab_id: usize,
    next_term_id: usize,
}

impl App {
    pub fn new(file: Option<String>) -> Result<Self> {
        let keys = config::load();
        let state = PersistedState::load();
        let (agent_tx, agent_rx) = mpsc::channel();
        let (term_tx, term_rx) = mpsc::channel();
        let mut app = App {
            buffers: HashMap::new(),
            panes: HashMap::new(),
            tabs: vec![],
            active_tab: 0,
            mode: Mode::Edit,
            palette: None,
            status_msg: None,
            should_quit: false,
            keys,
            frecency: state.frecency,
            pending_prefix: Vec::new(),
            prefix_tick: 0,
            prompt: None,
            kill_ring: Vec::new(),
            last_yank: None,
            search_origin: None,
            search_hl: Vec::new(),
            search_labels: Vec::new(),
            search_pick: false,
            bar_return: Mode::Edit,
            bar_uses: state.bar_uses,
            pane_rects: Vec::new(),
            cursor_screen: None,
            // Env gate keeps selfchecks from touching the user's real clipboard.
            clipboard: if std::env::var("MARS_NO_SYSTEM_CLIPBOARD").is_ok()
                || std::env::var("ARES_NO_SYSTEM_CLIPBOARD").is_ok()
            {
                None
            } else {
                arboard::Clipboard::new().ok()
            },
            tuning: tuning::load(),
            show_splash: file.is_none(),
            startup_cwd: file
                .as_ref()
                .and_then(|f| std::path::Path::new(f).parent().map(|p| p.to_path_buf()))
                .filter(|p| !p.as_os_str().is_empty()),
            run_cwd: std::env::current_dir().ok(),
            project_index: None,
            file_frecency: state.file_frecency,
            file_tree: None,
            tree_open: false,
            tree_rows: Vec::new(),
            session_name: None,
            detach_requested: false,
            rename_session_to: None,
            agent_tx,
            agent_rx,
            agent_pending: false,
            agent_answer: None,
            agent_directive: None,
            refactor_target: None,
            refactor_replacement: None,
            last_question: String::new(),
            need_depth: 0,
            watches: HashMap::new(),
            notices: Vec::new(),
            pending_watch: None,
            detach_snapshot: None,
            agent_history: Vec::new(),
            ask_scroll: 0,
            bg_busy: false,
            auto_name_attempted: std::collections::HashSet::new(),
            shell_ready: false,
            session_name_attempted: false,
            frame_tick: 0,
            terms: HashMap::new(),
            term_tx,
            term_rx,
            next_buffer_id: 0,
            next_pane_id: 0,
            next_tab_id: 0,
            next_term_id: 0,
        };
        let buf_id = match file {
            Some(ref path) => app.open_file(path)?,
            None => app.new_scratch(),
        };
        let pane_id = app.alloc_pane(buf_id);
        let tab = Tab::new(app.alloc_tab_id(), "1".into(), pane_id);
        app.tabs.push(tab);
        Ok(app)
    }

    // ── ID allocators ────────────────────────────────────────────────────────

    fn alloc_buf_id(&mut self) -> BufferId {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        id
    }
    fn alloc_pane_id(&mut self) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        id
    }
    fn alloc_tab_id(&mut self) -> TabId {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        id
    }

    // ── Buffer management ────────────────────────────────────────────────────

    pub fn new_scratch(&mut self) -> BufferId {
        let id = self.alloc_buf_id();
        self.buffers.insert(id, Buffer::new_scratch(id));
        id
    }

    pub fn open_file(&mut self, path: &str) -> Result<BufferId> {
        let id = self.alloc_buf_id();
        let buf = Buffer::from_file(id, std::path::PathBuf::from(path))?;
        self.buffers.insert(id, buf);
        // First file opened sets the cwd new terminals inherit.
        if self.startup_cwd.is_none() {
            self.startup_cwd = std::path::Path::new(path)
                .parent()
                .map(|p| p.to_path_buf())
                .filter(|p| !p.as_os_str().is_empty());
        }
        *self.file_frecency.entry(path.to_string()).or_insert(0) += 1;
        Ok(id)
    }

    /// Seed the project index directly (selfcheck only — bypasses the fs walk).
    pub fn set_project_index_for_test(&mut self, root: std::path::PathBuf, files: Vec<String>) {
        self.project_index = Some(project::Index { root, files });
    }

    /// Build the project index on first use (lazy); returns its root + files.
    fn ensure_project_index(&mut self) -> &project::Index {
        if self.project_index.is_none() {
            let root = self
                .startup_cwd
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let root = project::project_root(&root);
            let idx = project::Index::build(
                root,
                self.tuning.project_index_max,
                &self.tuning.project_ignore,
            );
            self.project_index = Some(idx);
        }
        self.project_index.as_ref().unwrap()
    }

    // ── Pane management ──────────────────────────────────────────────────────

    fn alloc_pane(&mut self, buffer_id: BufferId) -> PaneId {
        let id = self.alloc_pane_id();
        self.panes.insert(id, Pane::new(id, buffer_id));
        id
    }

    // ── Focus helpers ────────────────────────────────────────────────────────

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }
    pub fn focused_pane_id(&self) -> PaneId {
        self.tabs[self.active_tab].focused_pane
    }
    pub fn focused_pane(&self) -> &Pane {
        let id = self.focused_pane_id();
        self.panes.get(&id).unwrap()
    }
    pub fn focused_pane_mut(&mut self) -> &mut Pane {
        let id = self.focused_pane_id();
        self.panes.get_mut(&id).unwrap()
    }
    pub fn focused_buf_id(&self) -> BufferId {
        match self.focused_pane().content {
            PaneContent::Editor(buf_id) => buf_id,
            PaneContent::Terminal(_) => {
                // Return first available buffer id for terminal panes
                *self.buffers.keys().next().unwrap_or(&0)
            }
        }
    }
    pub fn focused_buf(&self) -> &Buffer {
        let id = self.focused_buf_id();
        self.buffers.get(&id).unwrap()
    }
    pub fn focused_buf_mut(&mut self) -> &mut Buffer {
        let id = self.focused_buf_id();
        self.buffers.get_mut(&id).unwrap()
    }

    // ── Cursor movement ──────────────────────────────────────────────────────

    pub fn move_up(&mut self) {
        let pane = self.focused_pane();
        if let PaneContent::Terminal(_) = pane.content { return; }
        let (row, affinity, buf_id) = (pane.cursor_row, pane.col_affinity, match pane.content { PaneContent::Editor(id) => id, _ => return });
        if row == 0 {
            return;
        }
        let new_row = row - 1;
        let len = self.buffers[&buf_id].line_len(new_row);
        let p = self.focused_pane_mut();
        p.cursor_row = new_row;
        p.cursor_col = affinity.min(len);
    }

    pub fn move_down(&mut self) {
        let pane = self.focused_pane();
        if let PaneContent::Terminal(_) = pane.content { return; }
        let (row, affinity, buf_id) = (pane.cursor_row, pane.col_affinity, match pane.content { PaneContent::Editor(id) => id, _ => return });
        let line_count = self.buffers[&buf_id].line_count();
        if row + 1 >= line_count {
            return;
        }
        let new_row = row + 1;
        let len = self.buffers[&buf_id].line_len(new_row);
        let p = self.focused_pane_mut();
        p.cursor_row = new_row;
        p.cursor_col = affinity.min(len);
    }

    pub fn move_left(&mut self) {
        let col = self.focused_pane().cursor_col;
        if col > 0 {
            let p = self.focused_pane_mut();
            p.cursor_col = col - 1;
            p.col_affinity = p.cursor_col;
        }
    }

    pub fn move_right(&mut self) {
        let pane = self.focused_pane();
        if let PaneContent::Terminal(_) = pane.content { return; }
        let (row, col, buf_id) = (pane.cursor_row, pane.cursor_col, match pane.content { PaneContent::Editor(id) => id, _ => return });
        let len = self.buffers[&buf_id].line_len(row);
        if col < len {
            let p = self.focused_pane_mut();
            p.cursor_col = col + 1;
            p.col_affinity = p.cursor_col;
        }
    }

    pub fn move_line_start(&mut self) {
        let p = self.focused_pane_mut();
        p.cursor_col = 0;
        p.col_affinity = 0;
    }

    pub fn move_line_end(&mut self) {
        let pane = self.focused_pane();
        if let PaneContent::Terminal(_) = pane.content { return; }
        let (row, buf_id) = (pane.cursor_row, match pane.content { PaneContent::Editor(id) => id, _ => return });
        let len = self.buffers[&buf_id].line_len(row);
        let p = self.focused_pane_mut();
        p.cursor_col = len;
        p.col_affinity = len;
    }

    pub fn move_file_start(&mut self) {
        let p = self.focused_pane_mut();
        p.cursor_row = 0;
        p.cursor_col = 0;
        p.col_affinity = 0;
    }

    pub fn move_file_end(&mut self) {
        let pane = self.focused_pane();
        if let PaneContent::Terminal(_) = pane.content { return; }
        let buf_id = match pane.content { PaneContent::Editor(id) => id, _ => return };
        let line_count = self.buffers[&buf_id].line_count();
        let last = line_count.saturating_sub(1);
        let len = self.buffers[&buf_id].line_len(last);
        let p = self.focused_pane_mut();
        p.cursor_row = last;
        p.cursor_col = len;
        p.col_affinity = len;
    }

    // ── Text editing ─────────────────────────────────────────────────────────

    fn insert_char_at_cursor(&mut self, c: char) {
        let pane = self.focused_pane();
        let buf_id = match pane.content { PaneContent::Editor(id) => id, _ => return };
        let (row, col) = (pane.cursor_row, pane.cursor_col);
        let char_idx = self.buffers[&buf_id].char_at(row, col);
        {
            let buf = self.buffers.get_mut(&buf_id).unwrap();
            buf.rope.insert_char(char_idx, c);
            buf.modified = true;
        }
        let p = self.focused_pane_mut();
        if c == '\n' {
            p.cursor_row += 1;
            p.cursor_col = 0;
        } else {
            p.cursor_col += 1;
        }
        p.col_affinity = p.cursor_col;
    }

    fn delete_before_cursor(&mut self) {
        let pane = self.focused_pane();
        let buf_id = match pane.content { PaneContent::Editor(id) => id, _ => return };
        let (row, col) = (pane.cursor_row, pane.cursor_col);
        if col == 0 && row == 0 {
            return;
        }
        let char_idx = self.buffers[&buf_id].char_at(row, col);
        if char_idx == 0 {
            return;
        }
        let new_pos = if col > 0 {
            (row, col - 1)
        } else {
            let prev_len = self.buffers[&buf_id].line_len(row - 1);
            (row - 1, prev_len)
        };
        {
            let buf = self.buffers.get_mut(&buf_id).unwrap();
            buf.rope.remove(char_idx - 1..char_idx);
            buf.modified = true;
        }
        let p = self.focused_pane_mut();
        p.cursor_row = new_pos.0;
        p.cursor_col = new_pos.1;
        p.col_affinity = new_pos.1;
    }

    // ── Position helpers ─────────────────────────────────────────────────────

    /// (row, col, buffer) for the focused pane, or None if it hosts a terminal.
    fn editor_pos(&self) -> Option<(usize, usize, BufferId)> {
        let p = self.focused_pane();
        match p.content {
            PaneContent::Editor(id) => Some((p.cursor_row, p.cursor_col, id)),
            PaneContent::Terminal(_) => None,
        }
    }

    fn rowcol_of(&self, buf_id: BufferId, idx: usize) -> (usize, usize) {
        let rope = &self.buffers[&buf_id].rope;
        let idx = idx.min(rope.len_chars());
        let line = rope.char_to_line(idx);
        (line, idx - rope.line_to_char(line))
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        let p = self.focused_pane_mut();
        p.cursor_row = row;
        p.cursor_col = col;
        p.col_affinity = col;
    }

    // ── Selection ────────────────────────────────────────────────────────────

    fn clear_selection(&mut self) {
        let id = self.focused_pane_id();
        if let Some(p) = self.panes.get_mut(&id) { p.selection_anchor = None; }
    }

    fn begin_or_keep_selection(&mut self) {
        let (r, c) = { let p = self.focused_pane(); (p.cursor_row, p.cursor_col) };
        let p = self.focused_pane_mut();
        if p.selection_anchor.is_none() { p.selection_anchor = Some((r, c)); }
    }

    pub fn selection_range(&self) -> Option<(BufferId, usize, usize)> {
        let p = self.focused_pane();
        let anchor = p.selection_anchor?;
        let buf_id = match p.content { PaneContent::Editor(id) => id, _ => return None };
        let buf = &self.buffers[&buf_id];
        let a = buf.char_at(anchor.0, anchor.1);
        let b = buf.char_at(p.cursor_row, p.cursor_col);
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        if s == e { None } else { Some((buf_id, s, e)) }
    }

    // Selection-aware movement wrappers.
    fn move_left_sel(&mut self, extend: bool)  { if extend { self.begin_or_keep_selection(); } else { self.clear_selection(); } self.move_left(); }
    fn move_right_sel(&mut self, extend: bool) { if extend { self.begin_or_keep_selection(); } else { self.clear_selection(); } self.move_right(); }
    fn move_up_sel(&mut self, extend: bool)    { if extend { self.begin_or_keep_selection(); } else { self.clear_selection(); } self.move_up(); }
    fn move_down_sel(&mut self, extend: bool)  { if extend { self.begin_or_keep_selection(); } else { self.clear_selection(); } self.move_down(); }
    fn move_line_start_sel(&mut self, extend: bool) { if extend { self.begin_or_keep_selection(); } else { self.clear_selection(); } self.move_line_start(); }
    fn move_line_end_sel(&mut self, extend: bool)   { if extend { self.begin_or_keep_selection(); } else { self.clear_selection(); } self.move_line_end(); }

    /// One page ≈ viewport height minus overlap (fallback before first render).
    fn page_len(&self) -> usize {
        let h = self.focused_pane().view_h;
        if h == 0 { 18 } else { h.saturating_sub(self.tuning.page_overlap).max(1) }
    }
    fn page_up(&mut self) {
        self.clear_selection();
        for _ in 0..self.page_len() { self.move_up(); }
    }
    fn page_down(&mut self) {
        self.clear_selection();
        for _ in 0..self.page_len() { self.move_down(); }
    }

    // ── Word motion (M-f / M-b) ──────────────────────────────────────────────

    fn move_word_forward(&mut self) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let (len, mut idx) = {
            let b = &self.buffers[&buf_id];
            (b.rope.len_chars(), b.char_at(row, col))
        };
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        {
            let rope = &self.buffers[&buf_id].rope;
            while idx < len && !is_word(rope.char(idx)) { idx += 1; }
            while idx < len && is_word(rope.char(idx)) { idx += 1; }
        }
        let (r, c) = self.rowcol_of(buf_id, idx);
        self.set_cursor(r, c);
    }

    fn move_word_backward(&mut self) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let mut idx = self.buffers[&buf_id].char_at(row, col);
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        {
            let rope = &self.buffers[&buf_id].rope;
            while idx > 0 && !is_word(rope.char(idx - 1)) { idx -= 1; }
            while idx > 0 && is_word(rope.char(idx - 1)) { idx -= 1; }
        }
        let (r, c) = self.rowcol_of(buf_id, idx);
        self.set_cursor(r, c);
    }

    // ── Code-token motion (⌘←/→) ─────────────────────────────────────────────
    // A token is a maximal run of one class — word (alnum/`_`) or punctuation —
    // with whitespace skipped. So `foo.bar(baz)` stops at foo · . · bar · ( · baz
    // · ), which tracks how code reads (identifiers and operators as atoms).

    /// 0 = whitespace, 1 = word (alnum/underscore), 2 = punctuation.
    fn token_class(c: char) -> u8 {
        if c.is_whitespace() { 0 } else if c.is_alphanumeric() || c == '_' { 1 } else { 2 }
    }

    pub fn move_token_forward(&mut self) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let (len, mut idx) = {
            let b = &self.buffers[&buf_id];
            (b.rope.len_chars(), b.char_at(row, col))
        };
        {
            let rope = &self.buffers[&buf_id].rope;
            // Consume the current token's run, then any whitespace, landing on the
            // start of the next token.
            if idx < len {
                let c0 = Self::token_class(rope.char(idx));
                if c0 != 0 {
                    while idx < len && Self::token_class(rope.char(idx)) == c0 { idx += 1; }
                }
            }
            while idx < len && Self::token_class(rope.char(idx)) == 0 { idx += 1; }
        }
        let (r, c) = self.rowcol_of(buf_id, idx);
        self.set_cursor(r, c);
    }

    pub fn move_token_backward(&mut self) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let mut idx = self.buffers[&buf_id].char_at(row, col);
        {
            let rope = &self.buffers[&buf_id].rope;
            while idx > 0 && Self::token_class(rope.char(idx - 1)) == 0 { idx -= 1; }
            if idx > 0 {
                let c0 = Self::token_class(rope.char(idx - 1));
                while idx > 0 && Self::token_class(rope.char(idx - 1)) == c0 { idx -= 1; }
            }
        }
        let (r, c) = self.rowcol_of(buf_id, idx);
        self.set_cursor(r, c);
    }

    fn move_token_sel(&mut self, forward: bool, extend: bool) {
        if extend { self.begin_or_keep_selection(); } else { self.clear_selection(); }
        if forward { self.move_token_forward(); } else { self.move_token_backward(); }
    }

    // ── Structural jumps (C-x [ ] { } m) ─────────────────────────────────────

    fn line_is_blank(b: &Buffer, r: usize) -> bool {
        b.rope.line(r).chars().all(|c| c.is_whitespace())
    }

    /// Jump to the next/prev blank line — fly between code blocks.
    pub fn jump_block(&mut self, forward: bool) {
        let (row, _c, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let n = self.buffers[&buf_id].line_count();
        let target = {
            let b = &self.buffers[&buf_id];
            if forward {
                let mut r = row + 1;
                while r < n && Self::line_is_blank(b, r) { r += 1; }
                while r < n && !Self::line_is_blank(b, r) { r += 1; }
                r.min(n.saturating_sub(1))
            } else {
                let mut r = row.saturating_sub(1);
                while r > 0 && Self::line_is_blank(b, r) { r -= 1; }
                while r > 0 && !Self::line_is_blank(b, r) { r -= 1; }
                r
            }
        };
        self.clear_selection();
        self.set_cursor(target, 0);
    }

    /// Jump to the next/prev top-level definition (column-0 keyword heuristic).
    pub fn jump_symbol(&mut self, forward: bool) {
        let (row, _c, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let n = self.buffers[&buf_id].line_count();
        const KWS: &[&str] = &[
            "fn ", "pub fn", "pub(", "pub struct", "pub enum", "def ", "class ", "impl",
            "struct ", "enum ", "trait ", "mod ", "type ", "func ", "function ",
            "interface ", "async fn", "export ", "const fn",
        ];
        let is_def = |b: &Buffer, r: usize| -> bool {
            let line: String = b.rope.line(r).chars().collect();
            let t = line.trim_start();
            KWS.iter().any(|k| t.starts_with(k))
        };
        let target = {
            let b = &self.buffers[&buf_id];
            if forward {
                let mut r = row + 1;
                while r < n && !is_def(b, r) { r += 1; }
                (r < n).then_some(r)
            } else if row == 0 {
                None
            } else {
                let mut r = row - 1;
                loop {
                    if is_def(b, r) { break Some(r); }
                    if r == 0 { break None; }
                    r -= 1;
                }
            }
        };
        if let Some(r) = target {
            self.clear_selection();
            self.set_cursor(r, 0);
        }
    }

    /// Jump to the bracket matching the one at (or just before) the cursor.
    pub fn match_bracket(&mut self) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        const OPENS: [char; 3] = ['(', '[', '{'];
        const CLOSES: [char; 3] = [')', ']', '}'];
        let target = {
            let rope = &self.buffers[&buf_id].rope;
            let len = rope.len_chars();
            let cur = self.buffers[&buf_id].char_at(row, col);
            // Find a bracket: the char under the cursor, scanning to end of line;
            // else the char just before the cursor.
            let mut found = None;
            let mut j = cur;
            while j < len {
                let c = rope.char(j);
                if c == '\n' { break; }
                if OPENS.contains(&c) || CLOSES.contains(&c) { found = Some((j, c)); break; }
                j += 1;
            }
            if found.is_none() && cur > 0 {
                let c = rope.char(cur - 1);
                if OPENS.contains(&c) || CLOSES.contains(&c) { found = Some((cur - 1, c)); }
            }
            found.and_then(|(pos, c)| {
                if let Some(oi) = OPENS.iter().position(|&o| o == c) {
                    let (open, close) = (c, CLOSES[oi]);
                    let mut depth = 1i32;
                    let mut k = pos + 1;
                    while k < len {
                        let ch = rope.char(k);
                        if ch == open { depth += 1; }
                        else if ch == close { depth -= 1; if depth == 0 { return Some(k); } }
                        k += 1;
                    }
                    None
                } else if let Some(ci) = CLOSES.iter().position(|&cc| cc == c) {
                    let (open, close) = (OPENS[ci], c);
                    let mut depth = 1i32;
                    let mut k = pos;
                    while k > 0 {
                        k -= 1;
                        let ch = rope.char(k);
                        if ch == close { depth += 1; }
                        else if ch == open { depth -= 1; if depth == 0 { return Some(k); } }
                    }
                    None
                } else {
                    None
                }
            })
        };
        if let Some(idx) = target {
            let (r, c) = self.rowcol_of(buf_id, idx);
            self.clear_selection();
            self.set_cursor(r, c);
        }
    }

    // ── Kill-ring editing (C-d / C-k / C-w / M-w / C-y) ──────────────────────

    /// Every kill/copy lands in the kill-ring AND the system clipboard —
    /// copy in Ares, paste in the browser.
    fn push_kill(&mut self, text: String) {
        if let Some(cb) = self.clipboard.as_mut() {
            let _ = cb.set_text(text.clone());
        }
        self.kill_ring.push(text);
    }

    /// Insert a block of text at the cursor (one undo chunk, replaces selection).
    fn insert_text(&mut self, text: &str) {
        if self.editor_pos().is_none() {
            return;
        }
        self.focused_buf_mut().checkpoint();
        self.delete_selection();
        for ch in text.chars() {
            self.insert_char_at_cursor(ch);
        }
    }

    /// C-v — paste from the system clipboard (kill-ring head as fallback).
    fn paste_clipboard(&mut self) {
        let text = self
            .clipboard
            .as_mut()
            .and_then(|cb| cb.get_text().ok())
            .filter(|t| !t.is_empty())
            .or_else(|| self.kill_ring.last().cloned());
        match text {
            Some(t) => self.insert_text(&t),
            None => self.status_msg = Some("Clipboard empty".into()),
        }
    }

    /// Bracketed paste from the host terminal (Cmd+V etc.) — routed by mode.
    pub fn paste_text(&mut self, s: &str) {
        match self.mode {
            Mode::Terminal => {
                if let PaneContent::Terminal(tid) = self.focused_pane().content {
                    if let Some(t) = self.terms.get_mut(&tid) {
                        // Re-wrap if the inner app requested bracketed paste.
                        let wrap = t.screen().bracketed_paste();
                        if wrap { t.send_bytes(b"\x1b[200~"); }
                        t.send_bytes(s.as_bytes());
                        if wrap { t.send_bytes(b"\x1b[201~"); }
                    }
                }
            }
            Mode::Bar => {
                let clean: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
                if let Some(p) = self.palette.as_mut() {
                    p.query.push_str(&clean);
                }
            }
            Mode::Prompt => {
                let clean: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
                let is_search = if let Some(p) = self.prompt.as_mut() {
                    p.input.push_str(&clean);
                    p.kind == PromptKind::Search
                } else {
                    false
                };
                if is_search {
                    let q = self.prompt.as_ref().map(|p| p.input.clone()).unwrap_or_default();
                    self.update_isearch(&q);
                }
            }
            _ => self.insert_text(s),
        }
    }

    fn delete_char_forward(&mut self) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let buf = self.buffers.get_mut(&buf_id).unwrap();
        let idx = buf.char_at(row, col);
        if idx < buf.rope.len_chars() {
            buf.checkpoint();
            buf.rope.remove(idx..idx + 1);
            buf.modified = true;
        }
    }

    fn kill_line(&mut self) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let killed = {
            let buf = self.buffers.get_mut(&buf_id).unwrap();
            buf.checkpoint();
            let start = buf.char_at(row, col);
            let eol = buf.line_len(row);
            let end = if col >= eol { (start + 1).min(buf.rope.len_chars()) } else { buf.char_at(row, eol) };
            if end > start {
                let k = buf.rope.slice(start..end).to_string();
                buf.rope.remove(start..end);
                buf.modified = true;
                k
            } else {
                String::new()
            }
        };
        if !killed.is_empty() { self.push_kill(killed); }
    }

    fn kill_region(&mut self) {
        if let Some((buf_id, s, e)) = self.selection_range() {
            let killed = {
                let buf = self.buffers.get_mut(&buf_id).unwrap();
                buf.checkpoint();
                let k = buf.rope.slice(s..e).to_string();
                buf.rope.remove(s..e);
                buf.modified = true;
                k
            };
            self.push_kill(killed);
            let (r, c) = self.rowcol_of(buf_id, s);
            self.set_cursor(r, c);
            self.clear_selection();
        }
    }

    fn copy_region(&mut self) {
        if let Some((buf_id, s, e)) = self.selection_range() {
            let text = self.buffers[&buf_id].rope.slice(s..e).to_string();
            self.push_kill(text);
            self.status_msg = Some("Copied".into());
        } else if let Some((row, _, buf_id)) = self.editor_pos() {
            // No selection → copy the whole current line (VS Code behavior).
            let line = self.buffers[&buf_id].line_str(row);
            if !line.is_empty() {
                self.push_kill(line);
                self.status_msg = Some("Copied line".into());
            }
        }
        self.clear_selection();
    }

    fn yank(&mut self) {
        if let Some(text) = self.kill_ring.last().cloned() {
            let start = match self.editor_pos() {
                Some((r, c, id)) => (id, self.buffers[&id].char_at(r, c)),
                None => return,
            };
            self.focused_buf_mut().checkpoint();
            for ch in text.chars() { self.insert_char_at_cursor(ch); }
            self.last_yank =
                Some((start.0, start.1, text.chars().count(), self.kill_ring.len() - 1));
        }
    }

    /// M-y — replace the text just yanked with the previous kill (rotating).
    fn yank_pop(&mut self) {
        let (buf_id, start, len, ridx) = match self.last_yank {
            Some(x) => x,
            None => {
                self.status_msg = Some("Previous command was not a yank".into());
                return;
            }
        };
        // Only valid while the cursor still sits at the end of the yanked text.
        let at_end = self
            .editor_pos()
            .map(|(r, c, id)| id == buf_id && self.buffers[&id].char_at(r, c) == start + len)
            .unwrap_or(false);
        if !at_end || self.kill_ring.len() < 2 {
            self.status_msg = Some("Previous command was not a yank".into());
            return;
        }
        let new_ridx = if ridx == 0 { self.kill_ring.len() - 1 } else { ridx - 1 };
        let text = self.kill_ring[new_ridx].clone();
        {
            let buf = self.buffers.get_mut(&buf_id).unwrap();
            buf.rope.remove(start..start + len);
            buf.modified = true;
        }
        let (r, c) = self.rowcol_of(buf_id, start);
        self.set_cursor(r, c);
        for ch in text.chars() { self.insert_char_at_cursor(ch); }
        self.last_yank = Some((buf_id, start, text.chars().count(), new_ridx));
    }

    /// M-d / M-Backspace — kill from the cursor to a word boundary.
    fn kill_word(&mut self, forward: bool) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let from = self.buffers[&buf_id].char_at(row, col);
        if forward { self.move_word_forward(); } else { self.move_word_backward(); }
        let (row2, col2, _) = match self.editor_pos() { Some(x) => x, None => return };
        let to = self.buffers[&buf_id].char_at(row2, col2);
        let (s, e) = if from <= to { (from, to) } else { (to, from) };
        if s == e { return; }
        let killed = {
            let buf = self.buffers.get_mut(&buf_id).unwrap();
            buf.checkpoint();
            let k = buf.rope.slice(s..e).to_string();
            buf.rope.remove(s..e);
            buf.modified = true;
            k
        };
        let (r, c) = self.rowcol_of(buf_id, s);
        self.set_cursor(r, c);
        self.push_kill(killed);
    }

    /// C-l — center the viewport on the cursor line.
    fn recenter(&mut self) {
        let p = self.focused_pane_mut();
        let half = (p.view_h / 2).max(1);
        p.scroll_row = p.cursor_row.saturating_sub(half);
    }

    /// C-x h — select the whole buffer (anchor at start, cursor at end).
    fn select_all(&mut self) {
        if self.editor_pos().is_none() { return; }
        self.focused_pane_mut().selection_anchor = Some((0, 0));
        self.move_file_end();
    }

    // ── Buffers & windows ────────────────────────────────────────────────────

    fn kill_buffer(&mut self) {
        let buf_id = match self.editor_pos() { Some((_, _, id)) => id, None => return };
        if self.buffers.len() <= 1 {
            self.status_msg = Some("Only buffer".into());
            return;
        }
        let other = self.buffers.keys().copied().find(|&id| id != buf_id);
        self.buffers.remove(&buf_id);
        if let Some(o) = other {
            // Retarget EVERY pane showing the killed buffer, not just the
            // focused one — a stale BufferId would panic on next focus.
            for pane in self.panes.values_mut() {
                if matches!(pane.content, PaneContent::Editor(id) if id == buf_id) {
                    pane.content = PaneContent::Editor(o);
                    pane.buffer_id = o;
                    pane.cursor_row = 0; pane.cursor_col = 0; pane.scroll_row = 0;
                    pane.selection_anchor = None;
                }
            }
        }
    }

    fn delete_other_windows(&mut self) {
        let focused = self.focused_pane_id();
        for id in self.tab().layout.pane_ids() {
            if id != focused {
                self.panes.remove(&id);
                // any terminal owned by that pane is dropped with it
            }
        }
        let tab = self.tab_mut();
        tab.layout = PaneLayout::Single(focused);
        tab.focused_pane = focused;
    }

    // ── Incremental search ───────────────────────────────────────────────────

    /// All char indices where `needle` occurs in the focused buffer.
    fn find_matches(&self, buf_id: BufferId, needle: &str) -> Vec<usize> {
        let text: Vec<char> = self.buffers[&buf_id].rope.chars().collect();
        let pat: Vec<char> = needle.chars().collect();
        if pat.is_empty() || pat.len() > text.len() {
            return Vec::new();
        }
        (0..=text.len() - pat.len())
            .filter(|&i| text[i..i + pat.len()] == pat[..])
            .collect()
    }

    /// Refresh match highlights for the live query and jump to the first match
    /// at or after the search origin (wrapping).
    fn update_isearch(&mut self, needle: &str) {
        let buf_id = match self.editor_pos() { Some((_, _, id)) => id, None => return };
        let matches = self.find_matches(buf_id, needle);
        self.search_hl = matches
            .iter()
            .map(|&i| {
                let (r, c) = self.rowcol_of(buf_id, i);
                (r, c, needle.chars().count())
            })
            .collect();
        if needle.is_empty() {
            if let Some((r, c)) = self.search_origin {
                self.set_cursor(r, c);
            }
            return;
        }
        let origin_idx = self
            .search_origin
            .map(|(r, c)| self.buffers[&buf_id].char_at(r, c))
            .unwrap_or(0);
        match matches.iter().find(|&&i| i >= origin_idx).or(matches.first()) {
            Some(&idx) => {
                let (r, c) = self.rowcol_of(buf_id, idx);
                self.set_cursor(r, c);
            }
            None => self.status_msg = Some(format!("Failing I-search: {}", needle)),
        }
    }

    /// C-s / C-r inside isearch — jump to the next/previous match from the cursor.
    fn isearch_step(&mut self, needle: &str, forward: bool) {
        let (row, col, buf_id) = match self.editor_pos() { Some(x) => x, None => return };
        let matches = self.find_matches(buf_id, needle);
        if matches.is_empty() {
            self.status_msg = Some(format!("Failing I-search: {}", needle));
            return;
        }
        let cur = self.buffers[&buf_id].char_at(row, col);
        let idx = if forward {
            *matches.iter().find(|&&i| i > cur).unwrap_or(&matches[0]) // wrap
        } else {
            *matches.iter().rev().find(|&&i| i < cur).unwrap_or(matches.last().unwrap())
        };
        let (r, c) = self.rowcol_of(buf_id, idx);
        self.set_cursor(r, c);
    }

    fn start_isearch(&mut self) {
        let (row, col, _) = match self.editor_pos() {
            Some(x) => x,
            None => {
                self.status_msg = Some("No editor pane here".into());
                return;
            }
        };
        self.search_origin = Some((row, col));
        self.search_hl.clear();
        self.start_prompt(PromptKind::Search, "I-search: ");
    }

    fn end_isearch(&mut self, restore_origin: bool) {
        if restore_origin {
            if let Some((r, c)) = self.search_origin {
                self.set_cursor(r, c);
            }
        }
        self.search_origin = None;
        self.search_hl.clear();
        self.search_labels.clear();
        self.search_pick = false;
    }

    // ── Undo / redo ──────────────────────────────────────────────────────────

    fn do_undo(&mut self) {
        let did = self.focused_buf_mut().undo();
        if did {
            self.status_msg = Some("Undo".into());
        } else {
            self.status_msg = Some("Nothing to undo".into());
        }
        self.clamp_cursor_after_edit();
    }

    fn do_redo(&mut self) {
        let did = self.focused_buf_mut().redo();
        if did {
            self.status_msg = Some("Redo".into());
        } else {
            self.status_msg = Some("Nothing to redo".into());
        }
        self.clamp_cursor_after_edit();
    }

    fn clamp_cursor_after_edit(&mut self) {
        let pane = self.focused_pane();
        let buf_id = match pane.content { PaneContent::Editor(id) => id, _ => return };
        let line_count = self.buffers[&buf_id].line_count();
        let (row, col) = (pane.cursor_row, pane.cursor_col);
        let new_row = row.min(line_count.saturating_sub(1));
        let new_col = col.min(self.buffers[&buf_id].line_len(new_row));
        let p = self.focused_pane_mut();
        p.cursor_row = new_row;
        p.cursor_col = new_col;
        p.col_affinity = new_col;
    }

    // ── Save ─────────────────────────────────────────────────────────────────

    fn do_save(&mut self) {
        if self.focused_buf().path.is_none() {
            self.start_prompt(PromptKind::SaveAs, "Save as: ");
            return;
        }
        let name = self.focused_buf().name.clone();
        match self.focused_buf_mut().save() {
            Ok(_) => self.status_msg = Some(format!("Saved  {}", name)),
            Err(e) => self.status_msg = Some(format!("Save error: {}", e)),
        }
    }

    /// Quit, but never silently discard unsaved work.
    fn request_quit(&mut self) {
        let dirty = self.buffers.values().filter(|b| b.modified).count();
        if dirty == 0 {
            self.should_quit = true;
        } else {
            self.start_prompt(
                PromptKind::ConfirmQuit,
                &format!("{} modified buffer(s):  s save all & quit · q quit anyway · C-g cancel ", dirty),
            );
        }
    }

    /// Crash-safety: quietly save every modified buffer that has a real path.
    /// Scratch buffers are never touched. Called on a timer and on detach.
    pub fn autosave(&mut self) {
        for buf in self.buffers.values_mut() {
            if buf.modified && buf.path.is_some() {
                let _ = buf.save();
            }
        }
    }

    /// Save every modified buffer that has a path. Returns names left unsaved.
    fn save_all(&mut self) -> Vec<String> {
        let mut unsaved = Vec::new();
        for buf in self.buffers.values_mut() {
            if buf.modified {
                if buf.path.is_some() {
                    if buf.save().is_err() {
                        unsaved.push(buf.name.clone());
                    }
                } else {
                    unsaved.push(buf.name.clone());
                }
            }
        }
        unsaved
    }

    // ── Split panes ──────────────────────────────────────────────────────────

    pub fn split_horizontal(&mut self) {
        if self.tab().layout.count() >= self.tuning.max_panes {
            self.status_msg = Some(format!("Max {} panes", self.tuning.max_panes));
            return;
        }
        let focused = self.focused_pane_id();
        let buf_id = match self.focused_pane().content { PaneContent::Editor(id) => id, _ => self.new_scratch() };
        let new_id = self.alloc_pane(buf_id);
        let (r, c, s) = {
            let p = &self.panes[&focused];
            (p.cursor_row, p.cursor_col, p.scroll_row)
        };
        {
            let p = self.panes.get_mut(&new_id).unwrap();
            p.cursor_row = r;
            p.cursor_col = c;
            p.scroll_row = s;
        }
        let tab = self.tab_mut();
        tab.layout.hsplit(focused, new_id);
        tab.focused_pane = new_id;
        self.status_msg = Some("Split ─".into());
    }

    pub fn split_vertical(&mut self) {
        if self.tab().layout.count() >= self.tuning.max_panes {
            self.status_msg = Some(format!("Max {} panes", self.tuning.max_panes));
            return;
        }
        let focused = self.focused_pane_id();
        let buf_id = match self.focused_pane().content { PaneContent::Editor(id) => id, _ => self.new_scratch() };
        let new_id = self.alloc_pane(buf_id);
        let (r, c, s) = {
            let p = &self.panes[&focused];
            (p.cursor_row, p.cursor_col, p.scroll_row)
        };
        {
            let p = self.panes.get_mut(&new_id).unwrap();
            p.cursor_row = r;
            p.cursor_col = c;
            p.scroll_row = s;
        }
        let tab = self.tab_mut();
        tab.layout.vsplit(focused, new_id);
        tab.focused_pane = new_id;
        self.status_msg = Some("Split │".into());
    }

    pub fn close_pane(&mut self) {
        if self.tab().layout.count() <= 1 {
            return;
        }
        let focused = self.focused_pane_id();
        let next = self.tab().layout.next_pane(focused);
        let tab = self.tab_mut();
        tab.layout.remove(focused);
        tab.focused_pane = next;
        self.panes.remove(&focused);
    }

    pub fn focus_next_pane(&mut self) {
        let focused = self.focused_pane_id();
        let next = self.tab().layout.next_pane(focused);
        self.tab_mut().focused_pane = next;
    }

    /// M-arrows — focus the nearest pane in a screen direction, using the
    /// real geometry from the last render.
    fn focus_direction(&mut self, dx: i32, dy: i32) {
        let cur = self.focused_pane_id();
        let cur_rect = match self.pane_rects.iter().find(|(id, _)| *id == cur) {
            Some((_, r)) => *r,
            None => { self.focus_next_pane(); return; } // no geometry yet
        };
        let (cx, cy) = (
            cur_rect.x as i32 + cur_rect.width as i32 / 2,
            cur_rect.y as i32 + cur_rect.height as i32 / 2,
        );
        let mut best: Option<(i32, PaneId)> = None;
        for (id, r) in &self.pane_rects {
            if *id == cur { continue; }
            let px = r.x as i32 + r.width as i32 / 2;
            let py = r.y as i32 + r.height as i32 / 2;
            let (ddx, ddy) = (px - cx, py - cy);
            let aligned = if dx != 0 {
                ddx.signum() == dx && ddx.abs() >= ddy.abs()
            } else {
                ddy.signum() == dy && ddy.abs() >= ddx.abs()
            };
            if aligned {
                let dist = ddx.abs() + ddy.abs();
                if best.map(|(d, _)| dist < d).unwrap_or(true) {
                    best = Some((dist, *id));
                }
            }
        }
        if let Some((_, id)) = best {
            self.tab_mut().focused_pane = id;
        }
    }

    /// Grow/shrink the boundary nearest the focused pane (travel +/-).
    fn resize_pane(&mut self, delta: i16) {
        let focused = self.focused_pane_id();
        self.tab_mut().layout.resize(focused, delta);
    }

    /// Toggle zoom on the focused pane (travel z / tmux prefix-z).
    fn toggle_zoom(&mut self) {
        let focused = self.focused_pane_id();
        let tab = self.tab_mut();
        tab.zoomed = if tab.zoomed == Some(focused) { None } else { Some(focused) };
    }

    /// C-x x — move this pane's content into the next pane slot (swap).
    fn swap_pane(&mut self) {
        let a = self.focused_pane_id();
        let b = self.tab().layout.next_pane(a);
        if a == b { return; }
        let snap_a = self.panes.get(&a).unwrap().clone();
        let snap_b = self.panes.get(&b).unwrap().clone();
        for (dst, src) in [(a, &snap_b), (b, &snap_a)] {
            let p = self.panes.get_mut(&dst).unwrap();
            p.content = src.content.clone();
            p.buffer_id = src.buffer_id;
            p.cursor_row = src.cursor_row;
            p.cursor_col = src.cursor_col;
            p.col_affinity = src.col_affinity;
            p.scroll_row = src.scroll_row;
            p.selection_anchor = src.selection_anchor;
        }
        // Focus follows the moved content.
        self.tab_mut().focused_pane = b;
        self.status_msg = Some("Pane moved".into());
    }

    pub fn focus_prev_pane(&mut self) {
        let focused = self.focused_pane_id();
        let prev = self.tab().layout.prev_pane(focused);
        self.tab_mut().focused_pane = prev;
    }

    // ── Tab management ───────────────────────────────────────────────────────

    pub fn new_tab(&mut self) {
        let buf_id = self.new_scratch();
        let pane_id = self.alloc_pane(buf_id);
        let n = self.tabs.len() + 1;
        let id = self.alloc_tab_id();
        let tab = Tab::new(id, n.to_string(), pane_id);
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self) {
        if self.tabs.len() == 1 {
            self.request_quit();
            return;
        }
        let pane_ids = self.tab().layout.pane_ids();
        for pid in pane_ids {
            self.panes.remove(&pid);
        }
        self.tabs.remove(self.active_tab);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = if self.active_tab == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab - 1
            };
        }
    }

    /// Reorder: move the active tab one slot left/right (no wrap).
    pub fn move_tab(&mut self, delta: i32) {
        let i = self.active_tab as i32;
        let j = i + delta;
        if j < 0 || j >= self.tabs.len() as i32 {
            return;
        }
        self.tabs.swap(i as usize, j as usize);
        self.active_tab = j as usize;
    }

    /// M-1..M-9 — jump straight to tab N.
    fn goto_tab(&mut self, n: usize) {
        if n >= 1 && n <= self.tabs.len() {
            self.active_tab = n - 1;
        }
    }


    // ── Key handlers ─────────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        self.show_splash = false; // any keypress dismisses the banner
        match self.mode.clone() {
            Mode::Edit     => self.handle_edit(key),
            Mode::Bar      => self.handle_bar(key),
            Mode::Prompt   => self.handle_prompt(key),
            Mode::Tab      => self.handle_tab(key),
            Mode::Terminal => self.handle_terminal(key),
            Mode::Tree     => self.handle_tree(key),
        }
        Ok(())
    }

    // ── Non-modal editing (Emacs/Mac/Claude-Code feel) ───────────────────────

    fn handle_edit(&mut self, key: KeyEvent) {
        self.status_msg = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let chord = chord_of(&key);

        // Ctrl+Space (or NUL, which many terminals send for it) / M-x → bar.
        if self.pending_prefix.is_empty()
            && (self.keys.bar_open.contains(&chord) || matches!(key.code, KeyCode::Null))
        {
            self.open_bar(BarMode::Command);
            return;
        }

        // Terminal pane: Enter (no prefix) re-attaches — or dismisses a dead shell.
        if self.pending_prefix.is_empty() {
            if let PaneContent::Terminal(tid) = self.focused_pane().content {
                if matches!(key.code, KeyCode::Enter) && key.modifiers == KeyModifiers::NONE {
                    if self.terms.get(&tid).map(|t| t.exited).unwrap_or(true) {
                        self.close_terminal_pane(tid);
                    } else {
                        self.mode = Mode::Terminal;
                    }
                    return;
                }
            }
        }

        // C-g / Esc cancel a pending prefix / selection (Emacs quit, modern cancel).
        if (ctrl && matches!(key.code, KeyCode::Char('g')))
            || (matches!(key.code, KeyCode::Esc) && key.modifiers == KeyModifiers::NONE)
        {
            // Esc dismisses a proactive notice first (nothing else pending).
            if self.pending_prefix.is_empty()
                && self.focused_pane().selection_anchor.is_none()
                && self.dismiss_notice()
            {
                return;
            }
            let had_state = !self.pending_prefix.is_empty()
                || self.focused_pane().selection_anchor.is_some();
            self.pending_prefix.clear();
            self.clear_selection();
            if had_state {
                self.status_msg = Some("Quit".into());
            }
            return;
        }

        // Prefix-key state machine (C-x …).
        let mut seq = self.pending_prefix.clone();
        seq.push(chord.clone());
        if let Some(action) = self.keys.lookup(&seq) {
            self.pending_prefix.clear();
            self.run_action(action);
            return;
        }
        let extends = self.keys.edit.keys().any(|k| k.len() > seq.len() && k.starts_with(&seq));
        if extends {
            self.pending_prefix = seq;
            self.prefix_tick = self.frame_tick;
            return;
        }
        if !self.pending_prefix.is_empty() {
            let shown = crate::config::render_chords(&seq);
            self.pending_prefix.clear();
            self.status_msg = Some(format!("{} is undefined", shown));
            return;
        }

        // No binding matched → editing primitives.
        self.handle_edit_primitive(key);
    }

    fn handle_edit_primitive(&mut self, key: KeyEvent) {
        let ctrl  = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt   = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let cmd   = key.modifiers.contains(KeyModifiers::SUPER);

        self.last_yank = None; // any primitive key breaks a C-y / M-y chain

        match key.code {
            // Emacs cursor chords
            KeyCode::Char('f') if ctrl => self.move_right_sel(false),
            KeyCode::Char('b') if ctrl => self.move_left_sel(false),
            KeyCode::Char('n') if ctrl => self.move_down_sel(false),
            KeyCode::Char('p') if ctrl => self.move_up_sel(false),
            KeyCode::Char('a') if ctrl => self.move_line_start_sel(false),
            KeyCode::Char('e') if ctrl => self.move_line_end_sel(false),
            KeyCode::Char('d') if ctrl => self.delete_char_forward(),
            KeyCode::Char('f') if alt  => self.move_word_forward(),
            KeyCode::Char('b') if alt  => self.move_word_backward(),
            KeyCode::Char('v') if alt  => self.page_up(),

            // M-1..M-9 — jump to tab N (browser standard).
            KeyCode::Char(c) if alt && c.is_ascii_digit() => {
                self.goto_tab((c as u8 - b'0') as usize);
            }

            // Fast motion — ⌘ (kitty terminals) OR Option/Alt (the universal
            // fallback where the OS eats ⌘): ⌥←/→ = code-token, ⌥↑/↓ = page;
            // Shift extends the selection.
            KeyCode::Left  if cmd || alt => self.move_token_sel(false, shift),
            KeyCode::Right if cmd || alt => self.move_token_sel(true, shift),
            KeyCode::Up    if cmd || alt => self.page_up(),
            KeyCode::Down  if cmd || alt => self.page_down(),

            // Ctrl+arrows — directional pane focus (C-o and C-t travel also work).
            KeyCode::Left  if ctrl => self.focus_direction(-1, 0),
            KeyCode::Right if ctrl => self.focus_direction(1, 0),
            KeyCode::Up    if ctrl => self.focus_direction(0, -1),
            KeyCode::Down  if ctrl => self.focus_direction(0, 1),

            // Arrows / nav (Shift extends the selection, Mac-style)
            KeyCode::Left  => self.move_left_sel(shift),
            KeyCode::Right => self.move_right_sel(shift),
            KeyCode::Up    => self.move_up_sel(shift),
            KeyCode::Down  => self.move_down_sel(shift),
            KeyCode::Home  => self.move_line_start_sel(shift),
            KeyCode::End   => self.move_line_end_sel(shift),
            KeyCode::PageUp   => self.page_up(),
            KeyCode::PageDown => self.page_down(),

            // Editing — an active selection is replaced/deleted (Mac contract).
            KeyCode::Backspace => {
                if !self.delete_selection() { self.delete_before_cursor(); }
            }
            KeyCode::Delete => {
                if !self.delete_selection() { self.delete_char_forward(); }
            }
            KeyCode::Enter => {
                self.focused_buf_mut().checkpoint();
                self.delete_selection();
                self.insert_char_at_cursor('\n');
            }
            KeyCode::Tab   => { for _ in 0..4 { self.insert_char_at_cursor(' '); } }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.delete_selection();
                self.insert_char_at_cursor(c);
            }
            _ => {}
        }
    }

    /// Delete the active selection (no kill-ring). Returns true if one existed.
    fn delete_selection(&mut self) -> bool {
        if let Some((buf_id, s, e)) = self.selection_range() {
            {
                let buf = self.buffers.get_mut(&buf_id).unwrap();
                buf.checkpoint();
                buf.rope.remove(s..e);
                buf.modified = true;
            }
            let (r, c) = self.rowcol_of(buf_id, s);
            self.set_cursor(r, c);
            self.clear_selection();
            true
        } else {
            self.clear_selection();
            false
        }
    }

    // ── Minibuffer prompt (find-file, switch-buffer, search) ──────────────────

    fn open_bar(&mut self, bar_mode: BarMode) {
        // Remember where to return: a terminal keeps its focus (seamless switch).
        self.bar_return = if self.mode == Mode::Terminal { Mode::Terminal } else { Mode::Edit };
        let mut p = Palette::root();
        p.bar_mode = bar_mode;
        self.palette = Some(p);
        self.mode = Mode::Bar;
        self.shell_ready = false;
    }

    fn start_prompt(&mut self, kind: PromptKind, label: &str) {
        self.start_prompt_with(kind, label, "");
    }

    /// Prompt pre-filled with the current value (rename flows).
    fn start_prompt_with(&mut self, kind: PromptKind, label: &str, initial: &str) {
        self.prompt = Some(Prompt {
            label: label.to_string(),
            input: initial.to_string(),
            kind,
        });
        self.mode = Mode::Prompt;
    }

    fn handle_prompt(&mut self, key: KeyEvent) {
        let kind = match self.prompt.as_ref() {
            Some(p) => p.kind.clone(),
            None => { self.mode = Mode::Edit; return; }
        };
        match kind {
            PromptKind::Search => self.handle_isearch_key(key),
            PromptKind::ConfirmQuit => self.handle_confirm_quit_key(key),
            PromptKind::ConfirmAction(action) => self.handle_confirm_action_key(key, action),
            _ => self.handle_line_prompt_key(key),
        }
    }

    fn close_prompt(&mut self) {
        self.prompt = None;
        self.mode = Mode::Edit;
    }

    fn handle_line_prompt_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('g')) {
            self.close_prompt();
            return;
        }
        match key.code {
            KeyCode::Esc => self.close_prompt(),
            KeyCode::Enter => {
                if let Some(p) = self.prompt.take() {
                    self.mode = Mode::Edit;
                    self.finish_prompt(p);
                }
            }
            KeyCode::Backspace => { if let Some(p) = self.prompt.as_mut() { p.input.pop(); } }
            KeyCode::Char(c) if !ctrl => { if let Some(p) = self.prompt.as_mut() { p.input.push(c); } }
            _ => {}
        }
    }

    /// Live isearch: typing filters immediately; C-s/C-r step; Enter accepts;
    /// C-g/Esc restores the origin.
    fn handle_isearch_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let query = self.prompt.as_ref().map(|p| p.input.clone()).unwrap_or_default();

        // Label-pick mode (after Tab): the next key jumps to a labeled match.
        if self.search_pick {
            self.search_pick = false;
            if let KeyCode::Char(c) = key.code {
                if let Some(&(r, col, _)) = self.search_labels.iter().find(|(_, _, l)| *l == c) {
                    self.set_cursor(r, col);
                    self.end_isearch(false);
                    self.close_prompt();
                    return;
                }
            }
            self.search_labels.clear(); // not a label → drop labels, handle normally
        }

        if ctrl && matches!(key.code, KeyCode::Char('g')) {
            self.end_isearch(true);
            self.close_prompt();
            return;
        }
        match key.code {
            KeyCode::Esc => { self.end_isearch(true); self.close_prompt(); }
            KeyCode::Enter => { self.end_isearch(false); self.close_prompt(); }
            KeyCode::Char('s') if ctrl => self.isearch_step(&query, true),
            KeyCode::Char('r') if ctrl => self.isearch_step(&query, false),
            // Tab → teleport: label the matches; the next key jumps to one.
            KeyCode::Tab => {
                if self.search_hl.len() >= 2 {
                    self.build_search_labels();
                    self.search_pick = true;
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = self.prompt.as_mut() { p.input.pop(); }
                let q = self.prompt.as_ref().map(|p| p.input.clone()).unwrap_or_default();
                self.update_isearch(&q);
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(p) = self.prompt.as_mut() { p.input.push(c); }
                let q = self.prompt.as_ref().map(|p| p.input.clone()).unwrap_or_default();
                self.update_isearch(&q);
            }
            // Land-on-any-key: any other key accepts at the current match, then is
            // applied in edit mode — so search flows straight into editing.
            _ => {
                self.end_isearch(false);
                self.close_prompt();
                let _ = self.handle_key(key);
            }
        }
    }

    /// Assign home-row labels to the first matches (document order) for Tab-pick.
    fn build_search_labels(&mut self) {
        const ALPHA: &[u8] = b"asdfghjklqwertyuiopvbnm";
        self.search_labels = self
            .search_hl
            .iter()
            .take(ALPHA.len())
            .enumerate()
            .map(|(i, &(r, c, _))| (r, c, ALPHA[i] as char))
            .collect();
    }

    /// (1-based current, total) match index at the cursor, for the `n/m` counter.
    pub fn isearch_status(&self) -> Option<(usize, usize)> {
        let total = self.search_hl.len();
        if total == 0 {
            return None;
        }
        let pane = self.focused_pane();
        let (cr, cc) = (pane.cursor_row, pane.cursor_col);
        let cur = self
            .search_hl
            .iter()
            .position(|&(r, c, _)| r == cr && c == cc)
            .map(|i| i + 1)
            .unwrap_or(0);
        Some((cur, total))
    }

    fn handle_confirm_quit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('g')) {
            self.close_prompt();
            return;
        }
        match key.code {
            KeyCode::Char('s') => {
                let unsaved = self.save_all();
                self.close_prompt();
                if unsaved.is_empty() {
                    self.should_quit = true;
                } else {
                    self.status_msg = Some(format!(
                        "No file for: {} — save it first (C-x C-s)",
                        unsaved.join(", ")
                    ));
                }
            }
            KeyCode::Char('q') | KeyCode::Char('!') => {
                self.close_prompt();
                self.should_quit = true;
            }
            KeyCode::Esc | KeyCode::Char('n') => self.close_prompt(),
            _ => {}
        }
    }

    fn handle_confirm_action_key(&mut self, key: KeyEvent, action: Action) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.close_prompt();
                self.run_action(action);
            }
            _ if ctrl || matches!(key.code, KeyCode::Esc | KeyCode::Char('n')) => {
                self.close_prompt();
                self.status_msg = Some("Cancelled".into());
            }
            _ => {}
        }
    }

    fn finish_prompt(&mut self, p: Prompt) {
        match p.kind {
            PromptKind::GotoLine => {
                match p.input.trim().parse::<usize>() {
                    Ok(n) if n >= 1 => {
                        if let Some((_, _, buf_id)) = self.editor_pos() {
                            let last = self.buffers[&buf_id].line_count().saturating_sub(1);
                            let row = (n - 1).min(last);
                            self.set_cursor(row, 0);
                            self.recenter();
                        }
                    }
                    _ => self.status_msg = Some("Not a line number".into()),
                }
            }
            PromptKind::SaveAs => {
                let path = p.input.trim().to_string();
                if path.is_empty() { return; }
                let result = self
                    .focused_buf_mut()
                    .save_as(std::path::PathBuf::from(&path));
                self.status_msg = Some(match result {
                    Ok(_) => format!("Saved  {}", path),
                    Err(e) => format!("Save error: {}", e),
                });
            }
            PromptKind::RenameTab => {
                let name = p.input.trim().to_string();
                if !name.is_empty() {
                    let id = self.tab().id;
                    self.auto_name_attempted.insert(id); // manual name opts out
                    self.tab_mut().name = name;
                }
            }
            PromptKind::RenamePane => {
                let title = p.input.trim().to_string();
                let pid = self.focused_pane_id();
                if let Some(pane) = self.panes.get_mut(&pid) {
                    pane.title = if title.is_empty() { None } else { Some(title) };
                }
            }
            PromptKind::RenameSession => {
                let name = p.input.trim().to_string();
                if !name.is_empty() && !name.contains('/') {
                    self.rename_session_to = Some(name);
                } else if !name.is_empty() {
                    self.status_msg = Some("Session names cannot contain '/'".into());
                }
            }
            // Search / confirms are handled key-by-key, never via finish.
            PromptKind::Search | PromptKind::ConfirmQuit | PromptKind::ConfirmAction(_) => {}
        }
    }

    /// C-t travel mode — one-char verbs for tabs and panes, with an on-screen
    /// cheat panel. Rule: creation exits the mode, navigation stays.
    fn handle_tab(&mut self, key: KeyEvent) {
        let ctrl  = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers == KeyModifiers::SHIFT;

        // Leave: Esc / Enter / C-g / C-t (back to whatever the pane hosts).
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter)
            || (ctrl && matches!(key.code, KeyCode::Char('g') | KeyCode::Char('t')))
        {
            self.mode = self.mode_for_focused_pane();
            return;
        }

        match key.code {
            // ── Tabs ──
            KeyCode::Char('t') | KeyCode::Char('n') => {
                self.new_tab();
                self.mode = Mode::Edit; // creation exits — you'll want to type
            }
            KeyCode::Char('d') => self.close_tab(),
            KeyCode::Char('r') => self.run_action(Action::RenameTab), // → prompt, exits mode
            KeyCode::Char('?') => self.run_action(Action::ExplainFailure), // triage → Ask
            KeyCode::Char('w') => self.toggle_watch_pane(), // W6: watch this pane
            KeyCode::Char('h') | KeyCode::Left if !shift => self.prev_tab(),
            KeyCode::Char('l') | KeyCode::Right if !shift => self.next_tab(),
            KeyCode::Char('H') => self.move_tab(-1),
            KeyCode::Char('L') => self.move_tab(1),
            KeyCode::Left  if shift => self.move_tab(-1),
            KeyCode::Right if shift => self.move_tab(1),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                self.goto_tab((c as u8 - b'0') as usize);
                self.mode = self.mode_for_focused_pane(); // land ready to use
            }
            // ── Panes ──
            KeyCode::Char('o') | KeyCode::Tab => self.focus_next_pane(),
            KeyCode::Char('x') => self.swap_pane(),
            KeyCode::Char('z') => self.toggle_zoom(),
            KeyCode::Char('>') | KeyCode::Char('+') | KeyCode::Char('=') => self.resize_pane(6),
            KeyCode::Char('<') => self.resize_pane(-6),
            KeyCode::Char('|') | KeyCode::Char('\\') | KeyCode::Char('v') => {
                self.split_vertical();
                self.mode = Mode::Edit;
            }
            KeyCode::Char('-') | KeyCode::Char('s') => {
                self.split_horizontal();
                self.mode = Mode::Edit;
            }
            KeyCode::Char('q') | KeyCode::Char('0') => self.close_pane(),
            // ── Session ──
            KeyCode::Char('D') => {
                self.run_action(Action::Detach);
                if !self.detach_requested {
                    self.mode = self.mode_for_focused_pane(); // standalone: just exit mode
                }
            }
            _ => {}
        }
    }

    // ── Command bar (was handle_palette) ─────────────────────────────────────

    fn handle_bar(&mut self, key: KeyEvent) {
        let ctrl  = key.modifiers.contains(KeyModifiers::CONTROL);
        let none  = key.modifiers == KeyModifiers::NONE;
        let shift = key.modifiers == KeyModifiers::SHIFT;

        // Ctrl+Space inside a sub-mode (shell / file) → the full command bar.
        let chord = chord_of(&key);
        if self.keys.bar_open.contains(&chord) || matches!(key.code, KeyCode::Null) {
            if let Some(p) = self.palette.as_mut() {
                if p.bar_mode == BarMode::Shell {
                    p.bar_mode = BarMode::Command;
                    p.query.clear();
                    p.selected = 0;
                    self.shell_ready = false;
                }
            }
            return;
        }

        // Tab: in shell mode it TRANSLATES the English query into a command;
        // elsewhere it toggles CMD ↔ ASK.
        if let KeyCode::Tab = key.code {
            let mode = self.palette.as_ref().map(|p| p.bar_mode.clone());
            match mode {
                Some(BarMode::Shell) => { self.translate_shell_query(); return; }
                _ => {
                    if let Some(p) = self.palette.as_mut() {
                        p.bar_mode = match p.bar_mode {
                            BarMode::Command => BarMode::Ask,
                            _ => BarMode::Command,
                        };
                    }
                    return;
                }
            }
        }

        // Leading '!' / '?' / '@' on an empty query switches mode instead of typing:
        // `!` shell, `?` ask, `@` file picker (VS Code Ctrl+P style).
        let empty_query = self.palette.as_ref().map(|p| p.query.is_empty()).unwrap_or(false);
        if (none || shift) && empty_query {
            match key.code {
                KeyCode::Char('!') => {
                    if let Some(p) = self.palette.as_mut() { p.bar_mode = BarMode::Shell; }
                    return;
                }
                KeyCode::Char('?') => {
                    if let Some(p) = self.palette.as_mut() { p.bar_mode = BarMode::Ask; }
                    return;
                }
                KeyCode::Char('@') => {
                    self.close_bar();
                    self.toggle_file_tree(); // `@` opens the left file tree
                    return;
                }
                _ => {}
            }
        }

        let bar_mode = self
            .palette
            .as_ref()
            .map(|p| p.bar_mode.clone())
            .unwrap_or(BarMode::Command);
        match bar_mode {
            BarMode::Command => self.handle_bar_command(key, ctrl, none, shift),
            BarMode::Ask     => self.handle_bar_ask(key, none, shift),
            BarMode::Shell   => self.handle_bar_shell(key, none, shift),
        }
    }

    /// Clear the bar and any pending agent state, returning to the mode the
    /// bar was opened from (Edit, or Terminal for seamless switching).
    /// The unified terminal composer's shell fallback: the query didn't match a
    /// command, so translate it (LLM) into a shell command for confirmation —
    /// or, with no agent key, run it directly.
    fn submit_terminal_shell(&mut self) {
        let cmd = self.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default();
        if cmd.trim().is_empty() {
            return;
        }
        if self.shell_ready || !agent::AgentConfig::from_env().is_configured() {
            self.close_bar();
            self.run_shell_command(&cmd);
        } else {
            // Flip to the inline shell composer so the translated command shows,
            // anchored at the cursor, for a confirming second Enter.
            if let Some(p) = self.palette.as_mut() { p.bar_mode = BarMode::Shell; }
            self.translate_shell_query();
        }
    }

    fn close_bar(&mut self) {
        self.palette = None;
        self.mode = self.bar_return.clone();
        self.agent_answer = None;
        self.agent_directive = None;
        self.refactor_target = None;
        self.refactor_replacement = None;
        self.ask_scroll = 0;
        // agent_pending/agent_history survive — an in-flight answer lands in
        // the transcript and is there when the bar reopens.
    }

    fn handle_bar_command(&mut self, key: KeyEvent, ctrl: bool, none: bool, shift: bool) {
        let items_len = {
            let frec = &self.frecency;
            self.palette.as_ref().map(|p| p.visible_items(frec).len()).unwrap_or(0)
        };

        match key.code {
            KeyCode::Esc => {
                let close = if let Some(p) = self.palette.as_mut() { !p.pop() } else { true };
                if close { self.close_bar(); }
            }
            KeyCode::Up | KeyCode::BackTab => {
                if let Some(p) = self.palette.as_mut() { p.select_up(items_len); }
            }
            KeyCode::Down => {
                if let Some(p) = self.palette.as_mut() { p.select_down(items_len); }
            }
            KeyCode::Char('p') if ctrl => {
                if let Some(p) = self.palette.as_mut() { p.select_up(items_len); }
            }
            KeyCode::Char('n') if ctrl => {
                if let Some(p) = self.palette.as_mut() { p.select_down(items_len); }
            }
            KeyCode::Enter => {
                let frec = &self.frecency;
                let kind = self.palette.as_ref().and_then(|p| {
                    p.visible_items(frec).into_iter().nth(p.selected).map(|r| r.kind)
                });
                // Terminal composer: a query that matches no Mars command is a
                // shell command → LLM-translate + confirm (never a silent no-op).
                let has_query = self.palette.as_ref().map(|p| !p.query.trim().is_empty()).unwrap_or(false);
                if items_len == 0 && has_query && self.bar_return == Mode::Terminal {
                    self.submit_terminal_shell();
                } else {
                    self.activate_kind(kind);
                }
            }
            KeyCode::Backspace => {
                let close = if let Some(p) = self.palette.as_mut() {
                    if p.query.is_empty() {
                        !p.pop()
                    } else {
                        p.query.pop();
                        p.selected = 0;
                        false
                    }
                } else { false };
                if close { self.close_bar(); }
            }
            // Search-first (Claude-Code feel): typing always filters. Submenus are
            // reached with Enter, and fuzzy search flattens across them.
            KeyCode::Char(c) if none || shift => {
                if let Some(p) = self.palette.as_mut() {
                    p.query.push(c);
                    p.selected = 0;
                }
            }
            _ => {}
        }
    }

    /// Ask-the-AI submode: text is a natural-language question; Enter sends it,
    /// and a second Enter fires any directive (RUN/TYPE) the model proposed.
    fn handle_bar_ask(&mut self, key: KeyEvent, none: bool, shift: bool) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // C-l starts a fresh conversation.
        if ctrl && matches!(key.code, KeyCode::Char('l')) {
            self.agent_history.clear();
            self.agent_answer = None;
            self.agent_directive = None;
            self.refactor_target = None;
            self.refactor_replacement = None;
            self.ask_scroll = 0;
            return;
        }

        match key.code {
            KeyCode::Esc => self.close_bar(),
            // Scroll the transcript.
            KeyCode::Up => self.ask_scroll = self.ask_scroll.saturating_add(1),
            KeyCode::Down => self.ask_scroll = self.ask_scroll.saturating_sub(1),
            KeyCode::Enter => {
                // A pending refactor is confirmed with Enter (unless you're typing
                // a follow-up question), applied as one reversible edit.
                let has_query = self.palette.as_ref().map(|p| !p.query.trim().is_empty()).unwrap_or(false);
                if self.refactor_replacement.is_some() && !has_query {
                    self.apply_refactor();
                    return;
                }
                match self.agent_directive.clone() {
                    Some(agent::AgentDirective::Run(name)) => {
                        self.agent_directive = None;
                        let Some(action) = Action::from_name(&name) else {
                            self.agent_answer = Some(format!("⚠ unknown action: {name}"));
                            return;
                        };
                        self.close_bar();
                        if action.is_destructive() {
                            // Never let the model fire a destructive action unconfirmed.
                            self.start_prompt(
                                PromptKind::ConfirmAction(action.clone()),
                                &format!("Agent wants to run “{}” — y run · n cancel ", action.label()),
                            );
                        } else {
                            self.run_action(action);
                        }
                    }
                    Some(agent::AgentDirective::Type(cmd)) => {
                        self.agent_directive = None;
                        self.close_bar();
                        self.run_shell_command(&cmd);
                    }
                    Some(agent::AgentDirective::Open(loc)) => {
                        self.agent_directive = None;
                        self.close_bar();
                        self.open_at(&loc);
                    }
                    // NEED is auto-satisfied in tick and never surfaced here.
                    Some(agent::AgentDirective::Need(_)) => { self.agent_directive = None; }
                    None => self.submit_agent_query(),
                }
            }
            KeyCode::Backspace => {
                let close = if let Some(p) = self.palette.as_mut() {
                    if p.query.is_empty() {
                        true
                    } else {
                        p.query.pop();
                        false
                    }
                } else { false };
                if close {
                    self.close_bar();
                } else {
                    self.agent_directive = None; // a new edit invalidates the suggestion
                }
            }
            KeyCode::Char(c) if none || shift => {
                if let Some(p) = self.palette.as_mut() { p.query.push(c); }
                self.agent_directive = None;
            }
            _ => {}
        }
    }

    /// Inline natural-language shell composer. Enter translates the English
    /// request into a shell command via the agent (shown for confirmation),
    /// then a second Enter runs it. With no API key it runs the text literally.
    fn handle_bar_shell(&mut self, key: KeyEvent, none: bool, shift: bool) {
        match key.code {
            KeyCode::Esc => self.close_bar(),
            KeyCode::Enter => {
                let cmd = self.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default();
                if cmd.trim().is_empty() {
                    return;
                }
                if self.shell_ready || !agent::AgentConfig::from_env().is_configured() {
                    // Command is ready (translated), or there's no key to
                    // translate with → run what's shown.
                    self.close_bar();
                    self.run_shell_command(&cmd);
                } else {
                    // Translate the English request; the command lands in the
                    // pill (shell_ready) and the next Enter runs it.
                    self.translate_shell_query();
                }
            }
            KeyCode::Backspace => {
                self.shell_ready = false; // an edit invalidates the translation
                self.agent_answer = None; // and clears any stale error
                if let Some(p) = self.palette.as_mut() {
                    if p.query.is_empty() {
                        p.bar_mode = BarMode::Command;
                    } else {
                        p.query.pop();
                    }
                }
            }
            KeyCode::Char(c) if none || shift => {
                self.shell_ready = false;
                self.agent_answer = None;
                if let Some(p) = self.palette.as_mut() { p.query.push(c); }
            }
            _ => {}
        }
    }

    /// `OPEN: path:line` — open a file at a line (from a cited stack trace).
    /// If the focused pane is a terminal, split first so it stays visible.
    fn open_at(&mut self, loc: &str) {
        // Parse "path:line" — line optional, trailing ":col" tolerated.
        let (path, line) = match loc.rsplit_once(':') {
            Some((p, n)) if n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => {
                (p.to_string(), n.parse::<usize>().unwrap_or(1))
            }
            _ => (loc.to_string(), 1),
        };
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        // Keep a visible terminal by opening the file beside it.
        if matches!(self.focused_pane().content, PaneContent::Terminal(_))
            && self.tab().layout.count() < self.tuning.max_panes
        {
            self.split_vertical();
        }
        match self.open_file(path) {
            Ok(buf_id) => {
                let pid = self.focused_pane_id();
                if let Some(pane) = self.panes.get_mut(&pid) {
                    pane.content = PaneContent::Editor(buf_id);
                }
                let last = self.buffers[&buf_id].line_count().saturating_sub(1);
                let row = line.saturating_sub(1).min(last);
                self.set_cursor(row, 0);
                self.recenter();
                self.mode = Mode::Edit;
                self.status_msg = Some(format!("Opened {}:{}", path, line));
            }
            Err(e) => self.status_msg = Some(format!("Can't open {}: {}", path, e)),
        }
    }

    // ── Left file-tree sidebar (@ / C-x d) ───────────────────────────────────

    /// Open/focus/hide the tree (tri-state): closed → open+focus; open+focused →
    /// hide; open+unfocused → focus. Keeps the sidebar persistent across opens.
    pub fn toggle_file_tree(&mut self) {
        if !self.tree_open {
            self.ensure_project_index();
            if self.file_tree.is_none() {
                let root = self
                    .project_index
                    .as_ref()
                    .map(|i| i.root.clone())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                // Absolute path so `../` (parent) navigation works — a relative
                // "." has an empty parent and would blank the tree.
                let root = std::fs::canonicalize(&root).unwrap_or(root);
                self.file_tree = Some(FileTree {
                    root,
                    expanded: std::collections::HashSet::new(),
                    selected: 0,
                    filter: String::new(),
                });
            }
            self.tree_open = true;
            self.mode = Mode::Tree;
            self.refresh_tree_rows();
        } else if self.mode == Mode::Tree {
            self.close_tree();
        } else {
            self.mode = Mode::Tree;
            self.refresh_tree_rows();
        }
    }

    /// Hide the sidebar and forget its navigation state, so the next open starts
    /// fresh at the project root (not wherever `../` last wandered to).
    fn close_tree(&mut self) {
        self.tree_open = false;
        self.mode = Mode::Edit;
        self.file_tree = None;
        self.tree_rows.clear();
    }

    /// Recompute the flattened visible rows after any tree mutation.
    fn refresh_tree_rows(&mut self) {
        let rows = self.compute_tree_rows();
        let n = rows.len();
        self.tree_rows = rows;
        if let Some(t) = self.file_tree.as_mut() {
            if t.selected >= n {
                t.selected = n.saturating_sub(1);
            }
        }
    }

    /// The rows shown in the sidebar. Empty filter → the browse tree (folders
    /// expand in place); a filter → a flat fuzzy shortlist over the index.
    fn compute_tree_rows(&self) -> Vec<TreeRow> {
        let Some(tree) = self.file_tree.as_ref() else { return Vec::new() };
        if !tree.filter.is_empty() {
            // Shortlist: fuzzy over the project index (relative paths).
            let mut scored: Vec<(i64, u32, String)> = self
                .project_index
                .as_ref()
                .map(|i| {
                    i.files
                        .iter()
                        .filter_map(|f| {
                            palette::fuzzy_score(&tree.filter, f)
                                .map(|s| (s, *self.file_frecency.get(f).unwrap_or(&0), f.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            return scored
                .into_iter()
                .take(300)
                .map(|(_, _, rel)| TreeRow {
                    path: tree.root.join(&rel),
                    label: rel,
                    depth: 0,
                    is_dir: false,
                    expanded: false,
                    updir: false,
                })
                .collect();
        }
        // Browse: `../` (if the root has a parent), then the expanded tree.
        let mut rows = Vec::new();
        if tree.root.parent().is_some() {
            rows.push(TreeRow {
                path: tree.root.clone(),
                label: "../".into(),
                depth: 0,
                is_dir: true,
                expanded: false,
                updir: true,
            });
        }
        self.push_dir_rows(&tree.root, 0, &tree.expanded, &mut rows);
        rows
    }

    /// Append a directory's entries (dirs first, alpha), recursing into expanded
    /// folders — the expand-in-place tree.
    fn push_dir_rows(
        &self,
        dir: &std::path::Path,
        depth: usize,
        expanded: &std::collections::HashSet<std::path::PathBuf>,
        rows: &mut Vec<TreeRow>,
    ) {
        for (name, is_dir) in self.read_dir_entries(dir) {
            let path = dir.join(&name);
            let is_expanded = is_dir && expanded.contains(&path);
            rows.push(TreeRow {
                path: path.clone(),
                label: name,
                depth,
                is_dir,
                expanded: is_expanded,
                updir: false,
            });
            if is_expanded {
                self.push_dir_rows(&path, depth + 1, expanded, rows);
            }
        }
    }

    /// One directory's entries (dotdirs + the ignore-list skipped), dirs first.
    fn read_dir_entries(&self, dir: &std::path::Path) -> Vec<(String, bool)> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut entries: Vec<(String, bool)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || self.tuning.project_ignore.iter().any(|i| i == &name) {
                    return None;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                Some((name, is_dir))
            })
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        entries
    }

    fn handle_tree(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let none = key.modifiers.is_empty();
        let shift = key.modifiers == KeyModifiers::SHIFT;
        let len = self.tree_rows.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('g') if key.code == KeyCode::Esc || ctrl => {
                // Esc / C-g: clear an active filter, else close the sidebar.
                let cleared = self.file_tree.as_mut().map(|t| {
                    if t.filter.is_empty() { false } else { t.filter.clear(); t.selected = 0; true }
                }).unwrap_or(false);
                if cleared { self.refresh_tree_rows(); }
                else { self.close_tree(); }
            }
            KeyCode::Up | KeyCode::BackTab => {
                if let Some(t) = self.file_tree.as_mut() { t.selected = t.selected.saturating_sub(1); }
            }
            KeyCode::Down => {
                if let Some(t) = self.file_tree.as_mut() {
                    if t.selected + 1 < len { t.selected += 1; }
                }
            }
            KeyCode::Char('p') if ctrl => {
                if let Some(t) = self.file_tree.as_mut() { t.selected = t.selected.saturating_sub(1); }
            }
            KeyCode::Char('n') if ctrl => {
                if let Some(t) = self.file_tree.as_mut() {
                    if t.selected + 1 < len { t.selected += 1; }
                }
            }
            KeyCode::Enter => self.tree_activate(true),  // open + focus editor
            KeyCode::Right => self.tree_activate(false), // expand / preview (stay in tree)
            KeyCode::Left => self.tree_collapse(),
            KeyCode::Backspace => {
                let changed = self.file_tree.as_mut().map(|t| {
                    if t.filter.is_empty() { false } else { t.filter.pop(); t.selected = 0; true }
                }).unwrap_or(false);
                if changed { self.refresh_tree_rows(); }
            }
            KeyCode::Char(c) if none || shift => {
                if let Some(t) = self.file_tree.as_mut() { t.filter.push(c); t.selected = 0; }
                self.refresh_tree_rows();
            }
            _ => {}
        }
    }

    /// Enter/→ on a row. Folders expand and `../` re-roots for both. For a file,
    /// `commit` (Enter) opens it and focuses the editor; a preview (→) shows it
    /// but keeps you in the tree, reversibly — arrow to another file to re-preview.
    fn tree_activate(&mut self, commit: bool) {
        let Some(row) = self.tree_rows.get(self.file_tree.as_ref().map(|t| t.selected).unwrap_or(0)) else { return };
        let (path, is_dir, updir) = (row.path.clone(), row.is_dir, row.updir);
        if updir {
            if let Some(parent) = path.parent().map(|p| p.to_path_buf()) {
                if let Some(t) = self.file_tree.as_mut() { t.root = parent; t.selected = 0; }
            }
            self.refresh_tree_rows();
        } else if is_dir {
            if let Some(t) = self.file_tree.as_mut() {
                if !t.expanded.remove(&path) { t.expanded.insert(path); }
            }
            self.refresh_tree_rows();
        } else {
            self.show_file_in_pane(&path, commit);
        }
    }

    /// ←: collapse an expanded folder, else jump selection to the parent row.
    fn tree_collapse(&mut self) {
        let sel = self.file_tree.as_ref().map(|t| t.selected).unwrap_or(0);
        let Some(row) = self.tree_rows.get(sel) else { return };
        if row.is_dir && row.expanded {
            let path = row.path.clone();
            if let Some(t) = self.file_tree.as_mut() { t.expanded.remove(&path); }
            self.refresh_tree_rows();
        } else if row.depth > 0 {
            let target_depth = row.depth - 1;
            let parent = self.tree_rows[..sel].iter().rposition(|r| r.depth == target_depth);
            if let (Some(idx), Some(t)) = (parent, self.file_tree.as_mut()) { t.selected = idx; }
        }
    }

    /// Show a tree file in the focused pane. `commit` (Enter) focuses the editor;
    /// otherwise (→) it's a preview and focus stays in the tree. Reuses an already
    /// open buffer so repeated previews don't pile up duplicates.
    fn show_file_in_pane(&mut self, path: &std::path::Path, commit: bool) {
        let existing = self
            .buffers
            .values()
            .find(|b| b.path.as_deref() == Some(path))
            .map(|b| b.id);
        // Keep a visible terminal by opening the file beside it.
        if matches!(self.focused_pane().content, PaneContent::Terminal(_))
            && self.tab().layout.count() < self.tuning.max_panes
        {
            self.split_vertical();
        }
        let buf = match existing {
            Some(id) => Ok(id),
            None => self.open_file(&path.to_string_lossy()),
        };
        match buf {
            Ok(buf_id) => {
                let pid = self.focused_pane_id();
                if let Some(pane) = self.panes.get_mut(&pid) {
                    pane.content = PaneContent::Editor(buf_id);
                    pane.cursor_row = 0; pane.cursor_col = 0; pane.scroll_row = 0;
                    pane.selection_anchor = None;
                }
                if commit {
                    self.mode = Mode::Edit; // focus the editor; sidebar stays open
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    self.status_msg = Some(format!("Opened {name}"));
                }
            }
            Err(e) => self.status_msg = Some(format!("Can't open {}: {e}", path.display())),
        }
    }

    /// W3: send the shell-bar's English text to be turned into one command,
    /// which replaces the query when it returns (`ShellTranslation` event).
    fn translate_shell_query(&mut self) {
        let text = self.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default();
        if text.trim().is_empty() {
            return;
        }
        let cfg = agent::AgentConfig::from_env();
        if !cfg.is_configured() {
            self.status_msg = Some("No API key — set GEMINI_API_KEY to translate".into());
            return;
        }
        self.agent_pending = true;
        agent::translate_shell(cfg, text, self.screen_context(), self.agent_tx.clone());
    }

    /// Run `cmd` in a terminal pane: reuse one in this tab, else open one here.
    fn run_shell_command(&mut self, cmd: &str) {
        // Prefer an existing terminal pane in the current tab.
        let term_pane = self
            .tab()
            .layout
            .pane_ids()
            .into_iter()
            .find(|id| matches!(self.panes.get(id).map(|p| &p.content), Some(PaneContent::Terminal(_))));
        if let Some(pid) = term_pane {
            self.tab_mut().focused_pane = pid;
            self.mode = Mode::Terminal;
        } else {
            self.open_terminal();
        }
        if let PaneContent::Terminal(tid) = self.focused_pane().content {
            if let Some(t) = self.terms.get_mut(&tid) {
                t.send_bytes(cmd.as_bytes());
                t.send_bytes(b"\n");
            }
        }
    }

    /// W1/W2: open the Ask bar with a canned question and submit it at once —
    /// a zero-typing "explain / triage" gesture grounded in the live screen.
    fn ask_prefilled(&mut self, question: &str) {
        self.open_bar(BarMode::Ask);
        if let Some(p) = self.palette.as_mut() {
            p.query = question.to_string();
        }
        self.submit_agent_query();
    }

    /// Fire off the current Ask query to the LLM on a background thread —
    /// grounded in the live screen, with the conversation history attached.
    fn submit_agent_query(&mut self) {
        let question = self.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default();
        if question.trim().is_empty() {
            return;
        }
        let mut cfg = agent::AgentConfig::from_env();
        cfg.max_tokens = self.tuning.agent_max_tokens;
        cfg.temperature = self.tuning.agent_temperature;
        if !cfg.is_configured() {
            self.agent_answer = Some(
                "⚠ No API key. Export GROQ_API_KEY, GEMINI_API_KEY, or MARS_LLM_KEY and retry."
                    .into(),
            );
            self.agent_directive = None;
            return;
        }
        self.agent_pending = true;
        self.agent_answer = None;
        self.agent_directive = None;
        self.refactor_replacement = None;
        self.last_question = question.clone();
        self.need_depth = 0;
        self.ask_scroll = 0; // snap to the newest turn
        let history = self.agent_history.clone();
        self.agent_history.push(("user".into(), question.clone()));
        if let Some(p) = self.palette.as_mut() {
            p.query.clear();
        }
        // Selection-aware: a live selection becomes precise context, and marks the
        // range a proposed refactor would replace.
        let mut context = self.screen_context();
        self.refactor_target = self.selection_range();
        if let Some(sel) = self.selected_text() {
            context.push_str(&sel);
        }
        agent::ask(
            cfg,
            question,
            palette::registry_context(),
            context,
            history,
            self.agent_tx.clone(),
        );
    }

    /// Apply a confirm-gated refactor: replace the captured selection with the
    /// agent's code block, as ONE undo step (C-/ reverts the whole AI edit).
    pub fn apply_refactor(&mut self) {
        let (Some((buf_id, s, e)), Some(code)) =
            (self.refactor_target, self.refactor_replacement.take())
        else {
            return;
        };
        self.refactor_target = None;
        if let Some(buf) = self.buffers.get_mut(&buf_id) {
            buf.checkpoint(); // one reversible chunk
            buf.rope.remove(s..e);
            buf.rope.insert(s.min(buf.rope.len_chars()), &code);
            buf.modified = true;
        }
        let (r, c) = self.rowcol_of(buf_id, s + code.chars().count());
        self.close_bar();
        self.clear_selection();
        self.set_cursor(r, c);
        self.mode = Mode::Edit;
        self.status_msg = Some("Refactor applied — C-/ to undo".into());
    }

    /// W4/W5: replay the last question with an extra context source the model asked
    /// for via `NEED:`. One expansion per ask (capped in `tick`), never surfaced.
    fn reask_with_need(&mut self, kind: agent::NeedKind) {
        let mut cfg = agent::AgentConfig::from_env();
        cfg.max_tokens = self.tuning.agent_max_tokens;
        cfg.temperature = self.tuning.agent_temperature;
        if !cfg.is_configured() {
            self.agent_pending = false;
            return;
        }
        let extra = self.expand_context(&kind);
        let context = format!("{}\n\n### expanded ###\n{}", self.screen_context(), extra);
        let history = self.agent_history.clone();
        let q = self.last_question.clone();
        self.agent_pending = true; // keep the spinner; the re-ask is the same turn
        agent::ask(cfg, q, palette::registry_context(), context, history, self.agent_tx.clone());
    }

    /// Render the extra source a `NEED:` asked for (full scrollback, or another tab).
    fn expand_context(&self, kind: &agent::NeedKind) -> String {
        match kind {
            agent::NeedKind::Scrollback => {
                if let PaneContent::Terminal(id) = self.focused_pane().content {
                    if let Some(t) = self.terms.get(&id) {
                        let cap = self.tuning.terminal_scrollback_lines.min(2000);
                        return format!("FULL TERMINAL SCROLLBACK:\n{}", t.history_tail(cap));
                    }
                }
                String::new()
            }
            agent::NeedKind::Tab(name) => {
                let low = name.to_lowercase();
                let Some(tab) = self.tabs.iter().find(|t| t.name.to_lowercase().contains(&low)) else {
                    return format!("(no tab matching '{name}')");
                };
                let mut out = format!("TAB {}:\n", tab.name);
                for pid in tab.layout.pane_ids() {
                    let Some(p) = self.panes.get(&pid) else { continue };
                    match p.content {
                        PaneContent::Terminal(tid) => {
                            if let Some(t) = self.terms.get(&tid) {
                                out.push_str(t.screen().contents().trim_end());
                                out.push('\n');
                            }
                        }
                        PaneContent::Editor(bid) => {
                            if let Some(b) = self.buffers.get(&bid) {
                                out.push_str(&format!("[{}]\n", b.name));
                                for line in b.rope.to_string().lines().take(120) {
                                    out.push_str(line);
                                    out.push('\n');
                                }
                            }
                        }
                    }
                }
                out
            }
        }
    }

    /// The highlighted code as a labeled context block, telling the model that a
    /// refactor request should reply with ONLY the replacement in a ``` block.
    fn selected_text(&self) -> Option<String> {
        let (buf_id, s, e) = self.selection_range()?;
        let buf = self.buffers.get(&buf_id)?;
        let text = buf.rope.slice(s..e).to_string();
        let (sr, _) = self.rowcol_of(buf_id, s);
        let (er, _) = self.rowcol_of(buf_id, e);
        Some(format!(
            "\n\nSELECTED CODE — {} lines {}-{} (the user has this highlighted). If they ask \
             to refactor/rewrite/fix/simplify it, reply with ONLY the replacement inside one \
             ``` code block and no prose:\n```\n{}\n```\n",
            buf.name,
            sr + 1,
            er + 1,
            text
        ))
    }

    /// The context-bus slice: what the user is looking at, as text the model
    /// can ground its answers in. Capped so huge buffers can't blow the prompt.
    fn screen_context(&self) -> String {
        const CAP: usize = 6 * 1024;
        let mut out = String::new();
        if let Some(s) = &self.session_name {
            out.push_str(&format!("session: {s}\n"));
        }
        let tab_names: Vec<String> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == self.active_tab { format!("[{}]", t.name) } else { t.name.clone() }
            })
            .collect();
        out.push_str(&format!("tabs: {}\n", tab_names.join(" ")));

        let focused = self.focused_pane_id();
        for pid in self.tab().layout.pane_ids() {
            let Some(pane) = self.panes.get(&pid) else { continue };
            let marker = if pid == focused { " (focused)" } else { "" };
            match pane.content {
                PaneContent::Editor(buf_id) => {
                    if let Some(buf) = self.buffers.get(&buf_id) {
                        out.push_str(&format!(
                            "\n--- editor pane: {}{marker}, cursor at line {} ---\n",
                            buf.name,
                            pane.cursor_row + 1
                        ));
                        // The visible window plus a little margin.
                        let from = pane.scroll_row;
                        let to = (from + pane.view_h.max(20) + 10).min(buf.line_count());
                        for row in from..to {
                            out.push_str(&buf.line_str(row));
                            out.push('\n');
                        }
                    }
                }
                PaneContent::Terminal(tid) => {
                    if let Some(t) = self.terms.get(&tid) {
                        out.push_str(&format!("\n--- terminal pane{marker} ---\n"));
                        out.push_str(t.screen().contents().trim_end());
                        out.push('\n');
                    }
                }
            }
            if out.len() > CAP {
                break;
            }
        }
        if out.len() > CAP {
            // Keep the head (layout) and the tail (most recent output).
            let head: String = out.chars().take(CAP / 3).collect();
            let tail: String = out
                .chars()
                .rev()
                .take(2 * CAP / 3)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            out = format!("{head}\n…(truncated)…\n{tail}");
        }
        out
    }

    /// Dispatch a palette ItemKind.
    fn activate_kind(&mut self, kind: Option<ItemKind>) {
        match kind {
            Some(ItemKind::Submenu(name)) => {
                if let Some(p) = self.palette.as_mut() {
                    p.push(name);
                }
            }
            Some(ItemKind::Run(action)) => {
                self.palette = None;
                self.mode = self.bar_return.clone();
                self.run_action_from_bar(action);
            }
            None => {}
        }
    }

    /// Run an action chosen in the bar, and — once it's clearly a habit —
    /// nudge toward its direct keybinding (subtle, one status line, never blocks).
    fn run_action_from_bar(&mut self, action: Action) {
        let key = format!("{:?}", action);
        let uses = self.bar_uses.entry(key).or_insert(0);
        *uses += 1;
        let nudge = if *uses >= self.tuning.nudge_threshold {
            self.keys
                .binding_for(&action)
                .map(|b| format!("💡 next time: {}  ({})", b, action.label()))
        } else {
            None
        };
        self.run_action(action);
        if let Some(n) = nudge {
            self.status_msg = Some(n);
        }
    }

    /// Execute a palette action.
    pub fn run_action(&mut self, action: Action) {
        // Track frecency
        let key = format!("{:?}", action);
        *self.frecency.entry(key).or_insert(0) += 1;

        // Any action other than yank/yank-pop breaks the M-y chain.
        if !matches!(action, Action::Yank | Action::YankPop) {
            self.last_yank = None;
        }

        match action {
            Action::SplitHorizontal    => self.split_horizontal(),
            Action::SplitVertical      => self.split_vertical(),
            Action::ClosePane          => self.close_pane(),
            Action::DeleteOtherWindows => self.delete_other_windows(),
            Action::NextPane           => self.focus_next_pane(),
            Action::PrevPane           => self.focus_prev_pane(),
            Action::SwapPane           => self.swap_pane(),
            Action::NewTab             => self.new_tab(),
            Action::CloseTab           => self.close_tab(),
            Action::NextTab            => self.next_tab(),
            Action::PrevTab            => self.prev_tab(),
            Action::MoveTabLeft        => self.move_tab(-1),
            Action::MoveTabRight       => self.move_tab(1),
            Action::RenameTab          => {
                let current = self.tab().name.clone();
                self.start_prompt_with(PromptKind::RenameTab, "Rename tab: ", &current);
            }
            Action::RenamePane         => {
                let current = self.focused_pane().title.clone().unwrap_or_default();
                self.start_prompt_with(PromptKind::RenamePane, "Rename pane: ", &current);
            }
            Action::RenameSession      => {
                if self.session_name.is_some() {
                    let current = self.session_name.clone().unwrap_or_default();
                    self.start_prompt_with(PromptKind::RenameSession, "Rename session: ", &current);
                } else {
                    self.status_msg =
                        Some("Not in a session — start one with: mars new <name>".into());
                }
            }
            Action::TabMode            => self.mode = Mode::Tab,
            Action::Save               => self.do_save(),
            Action::FindFile | Action::QuickOpen | Action::ToggleFileTree
            | Action::SwitchBuffer => self.toggle_file_tree(),
            Action::RefreshIndex       => {
                self.project_index = None;
                self.ensure_project_index();
                if self.tree_open { self.refresh_tree_rows(); }
                self.status_msg = Some("File index refreshed".into());
            }
            Action::KillBuffer         => self.kill_buffer(),
            Action::Undo               => self.do_undo(),
            Action::Redo               => self.do_redo(),
            Action::KillLine           => self.kill_line(),
            Action::KillRegion         => self.kill_region(),
            Action::CopyRegion         => self.copy_region(),
            Action::Yank               => self.yank(),
            Action::YankPop            => self.yank_pop(),
            Action::Paste              => self.paste_clipboard(),
            Action::KillWordForward    => self.kill_word(true),
            Action::KillWordBackward   => self.kill_word(false),
            Action::SelectAll          => self.select_all(),
            Action::GoTop              => self.move_file_start(),
            Action::GoBottom           => self.move_file_end(),
            Action::GotoLine           => self.start_prompt(PromptKind::GotoLine, "Go to line: "),
            Action::JumpBlockPrev      => self.jump_block(false),
            Action::JumpBlockNext      => self.jump_block(true),
            Action::JumpSymbolPrev     => self.jump_symbol(false),
            Action::JumpSymbolNext     => self.jump_symbol(true),
            Action::MatchBracket       => self.match_bracket(),
            Action::Recenter           => self.recenter(),
            Action::Search             => self.start_isearch(),
            Action::SearchBackward     => self.start_isearch(),
            Action::OpenTerminal       => self.open_terminal(),
            Action::AskAgent           => self.open_bar(BarMode::Ask),
            Action::ExplainThis        => self.ask_prefilled(
                "Explain what's on screen at my cursor — what is this and what matters about it?",
            ),
            Action::ExplainFailure     => self.ask_prefilled(
                "Why did this fail? Name the cause, cite the exact file:line if there is one, \
                 and give the fix. Be terse.",
            ),
            Action::WatchPane          => self.toggle_watch_pane(),
            Action::Detach             => {
                if self.session_name.is_some() {
                    self.detach_requested = true;
                } else {
                    self.status_msg =
                        Some("Not in a session — start one with: mars --session <name>".into());
                }
            }
            Action::Quit               => self.request_quit(),
        }
    }

    // ── Terminal pane ────────────────────────────────────────────────────────

    pub fn open_terminal(&mut self) {
        // If this pane is already a terminal, just re-attach.
        if let PaneContent::Terminal(_) = self.focused_pane().content {
            self.mode = Mode::Terminal;
            return;
        }
        let id = self.next_term_id;
        self.next_term_id += 1;
        let (rows, cols) = (self.tuning.terminal_default_rows, self.tuning.terminal_default_cols);
        let scrollback = self.tuning.terminal_scrollback_lines;
        // The first opened file's dir if any, else where `mars` was launched —
        // never portable-pty's default (which lands the shell at /).
        let cwd = self.startup_cwd.clone().or_else(|| self.run_cwd.clone());
        match terminal::spawn(id, rows, cols, scrollback, cwd, self.term_tx.clone()) {
            Ok(term) => {
                self.terms.insert(id, term);
                let pid = self.focused_pane_id();
                if let Some(p) = self.panes.get_mut(&pid) {
                    p.content = PaneContent::Terminal(id);
                }
                self.mode = Mode::Terminal;
                self.status_msg = Some("Terminal — Ctrl+g to detach".into());
            }
            Err(e) => {
                self.status_msg = Some(format!("Terminal failed: {}", e));
            }
        }
    }

    /// The mode a pane's content wants when it has focus.
    fn mode_for_focused_pane(&self) -> Mode {
        match self.focused_pane().content {
            PaneContent::Terminal(_) => Mode::Terminal,
            PaneContent::Editor(_) => Mode::Edit,
        }
    }

    /// Chrome layer: navigation chords are global — they mean the same thing
    /// inside a terminal pane as in the editor. Editing chords (C-k, C-c,
    /// C-x…) are NOT intercepted; they keep their shell meanings.
    fn is_chrome_action(a: &Action) -> bool {
        matches!(
            a,
            Action::NextPane | Action::PrevPane | Action::SwapPane
                | Action::NextTab | Action::PrevTab | Action::MoveTabLeft
                | Action::MoveTabRight | Action::NewTab | Action::TabMode
                | Action::SplitHorizontal | Action::SplitVertical
        )
    }

    fn handle_terminal(&mut self, key: KeyEvent) {
        // Ctrl+Space from a terminal opens ONE composer: type to fuzzy-match Mars
        // commands; if the text matches no command, Enter treats it as a shell
        // command (LLM-translated + confirmed). No double-press.
        let chord = chord_of(&key);
        if self.keys.bar_open.contains(&chord) || matches!(key.code, KeyCode::Null) {
            self.open_bar(BarMode::Command);
            return;
        }
        // Ctrl+g detaches back to the editor.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if let KeyCode::Char('g') = key.code {
                self.mode = Mode::Edit;
                return;
            }
        }
        // Global chrome chords (single-chord only — prefixes belong to the shell).
        if let Some(action) = self.keys.lookup(std::slice::from_ref(&chord)) {
            if Self::is_chrome_action(&action) {
                self.run_action(action);
                // Follow the (possibly new) focused pane — unless the action
                // opened a transient mode of its own (travel mode).
                if !matches!(self.mode, Mode::Tab | Mode::Bar) {
                    self.mode = self.mode_for_focused_pane();
                }
                return;
            }
        }
        // Chrome primitives: M-1..9 tab jump, M-/Ctrl+arrows pane focus.
        let alt  = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char(c) if alt && c.is_ascii_digit() => {
                self.goto_tab((c as u8 - b'0') as usize);
                self.mode = self.mode_for_focused_pane();
                return;
            }
            KeyCode::Left if alt || ctrl => {
                self.focus_direction(-1, 0);
                self.mode = self.mode_for_focused_pane();
                return;
            }
            KeyCode::Right if alt || ctrl => {
                self.focus_direction(1, 0);
                self.mode = self.mode_for_focused_pane();
                return;
            }
            KeyCode::Up if alt || ctrl => {
                self.focus_direction(0, -1);
                self.mode = self.mode_for_focused_pane();
                return;
            }
            KeyCode::Down if alt || ctrl => {
                self.focus_direction(0, 1);
                self.mode = self.mode_for_focused_pane();
                return;
            }
            _ => {}
        }
        let term_id = match self.focused_pane().content {
            PaneContent::Terminal(id) => id,
            _ => {
                self.mode = Mode::Edit;
                return;
            }
        };

        // Dead shell: the pane only waits to be dismissed.
        if self.terms.get(&term_id).map(|t| t.exited).unwrap_or(false) {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char('q')) {
                self.close_terminal_pane(term_id);
            }
            return;
        }

        // Scrollback view: Shift+PageUp/PageDown page through history.
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if shift && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            let page = self.focused_pane().view_h.max(2) as i64 - 1;
            let delta = if key.code == KeyCode::PageUp { page } else { -page };
            if let Some(t) = self.terms.get_mut(&term_id) {
                t.scroll_view(delta);
            }
            return;
        }

        let bytes = key_to_bytes(&key);
        if !bytes.is_empty() {
            if let Some(t) = self.terms.get_mut(&term_id) {
                t.scroll_to_live(); // typing snaps out of scrollback
                t.send_bytes(&bytes);
            }
        }
    }

    /// Dismiss an exited terminal: close the pane, or recycle the last pane
    /// back into an editor showing a scratch/existing buffer.
    fn close_terminal_pane(&mut self, term_id: TermId) {
        self.terms.remove(&term_id);
        if self.tab().layout.count() > 1 {
            self.close_pane();
        } else {
            let buf = match self.buffers.keys().next().copied() {
                Some(b) => b,
                None => self.new_scratch(),
            };
            let pid = self.focused_pane_id();
            if let Some(p) = self.panes.get_mut(&pid) {
                p.content = PaneContent::Editor(buf);
                p.cursor_row = 0;
                p.cursor_col = 0;
                p.scroll_row = 0;
            }
        }
        self.mode = self.mode_for_focused_pane();
    }

    // ── Main loop ────────────────────────────────────────────────────────────

    /// One housekeeping tick: animation counter + PTY/agent event drains.
    /// Called every loop iteration whether or not a client is attached.
    pub fn tick(&mut self) {
        self.frame_tick = self.frame_tick.wrapping_add(1);

        // Drain terminal signals (repaint next frame); mark dead shells and feed
        // the watch clock (W6: output resets quiet, exit queues a verdict).
        let now = self.frame_tick;
        while let Ok(ev) = self.term_rx.try_recv() {
            match ev {
                TermEvent::Output(id) => {
                    if let Some(w) = self.watches.get_mut(&id) {
                        w.last_output_tick = now;
                        w.triggered = false;
                    }
                }
                TermEvent::Exited(id) => {
                    if let Some(t) = self.terms.get_mut(&id) {
                        t.exited = true;
                    }
                    if let Some(w) = self.watches.get_mut(&id) {
                        if w.watched && !w.triggered {
                            self.pending_watch = Some((id, WatchReason::Exit));
                        }
                    }
                }
            }
        }

        // Silent autosave of modified, path-backed buffers.
        let secs = self.tuning.autosave_secs;
        if secs > 0 {
            let ticks_per_save = (secs * 1000 / self.tuning.poll_interval_ms.max(1)).max(1);
            if self.frame_tick % ticks_per_save == 0 {
                self.autosave();
            }
        }

        // Drain background LLM-agent events.
        let mut events = Vec::new();
        while let Ok(ev) = self.agent_rx.try_recv() {
            events.push(ev);
        }
        for ev in events {
            match ev {
                AgentEvent::Answer { text, directive } => {
                    // W4/W5: a NEED: request re-asks once with the extra source and
                    // is never surfaced (no history push, spinner keeps spinning).
                    if let Some(agent::AgentDirective::Need(kind)) = &directive {
                        if self.need_depth < 1 {
                            self.need_depth += 1;
                            self.reask_with_need(kind.clone());
                            continue;
                        }
                    }
                    self.agent_pending = false;
                    // If the query targeted a selection and the reply carries a code
                    // block, offer it as a confirm-gated replacement (a refactor).
                    if self.refactor_target.is_some() {
                        self.refactor_replacement = extract_code_block(&text);
                    }
                    self.agent_history.push(("assistant".into(), text));
                    self.agent_directive = directive;
                    self.ask_scroll = 0; // show the new turn
                }
                AgentEvent::AutoName { tab_id, name } => {
                    self.bg_busy = false;
                    // Apply only if the tab still wears its default numeric
                    // name — a user rename always wins the race.
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        if tab.name.chars().all(|c| c.is_ascii_digit()) {
                            tab.name = name;
                        }
                    }
                }
                AgentEvent::SessionName { name } => {
                    self.bg_busy = false;
                    // Rename only if still numeric (user/explicit names win).
                    let numeric = self
                        .session_name
                        .as_ref()
                        .map(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false);
                    if numeric {
                        self.rename_session_to = Some(name);
                    }
                }
                AgentEvent::ShellTranslation { command } => {
                    self.agent_pending = false;
                    // Only meaningful if still composing a shell command.
                    let is_shell = self
                        .palette
                        .as_ref()
                        .map(|p| p.bar_mode == BarMode::Shell)
                        .unwrap_or(false);
                    if is_shell {
                        if let Some(p) = self.palette.as_mut() {
                            p.query = command;
                        }
                        self.shell_ready = true; // Enter now runs the translated command
                        self.agent_answer = None; // clear any prior error
                    }
                }
                AgentEvent::Error(e) => {
                    self.agent_pending = false;
                    self.bg_busy = false;
                    self.agent_answer = Some(format!("⚠ {}", e));
                    self.agent_directive = None;
                }
                AgentEvent::WatchSummary { term_id, verdict } => {
                    self.bg_busy = false;
                    let failed = verdict.to_lowercase().contains("fail")
                        || verdict.to_lowercase().contains("error");
                    let tab = self.tab_label_of_term(term_id);
                    if let Some(w) = self.watches.get_mut(&term_id) {
                        w.verdict = Some(verdict.clone());
                    }
                    self.notices.push(Notice {
                        text: format!("{verdict}{tab}"),
                        kind: if failed { NoticeKind::Failure } else { NoticeKind::Info },
                    });
                    // Failures surface first.
                    self.notices.sort_by(|a, b| a.kind.cmp(&b.kind));
                }
            }
        }

        self.maybe_auto_name();
        self.maybe_auto_name_session();
        self.maybe_fire_watches();
    }

    /// Toggle watching the focused terminal pane (W6).
    fn toggle_watch_pane(&mut self) {
        let PaneContent::Terminal(id) = self.focused_pane().content else {
            self.status_msg = Some("Watch works on a terminal pane".into());
            return;
        };
        let w = self.watches.entry(id).or_default();
        w.watched = !w.watched;
        w.last_output_tick = self.frame_tick;
        w.triggered = false;
        self.status_msg = Some(if w.watched {
            "Watching this pane — I'll summarize it when it quiets or exits".into()
        } else {
            "Stopped watching this pane".into()
        });
    }

    /// W7: capture a cheap snapshot when the last client detaches, so reattach can
    /// tell you what changed while you were gone.
    pub fn on_detach(&mut self) {
        self.detach_snapshot = Some(Snapshot {
            exited: self.terms.iter().filter(|(_, t)| t.exited).map(|(id, _)| *id).collect(),
            dirty: self.buffers.values().filter(|b| b.modified).map(|b| b.name.clone()).collect(),
            verdicts: self
                .watches
                .iter()
                .filter_map(|(id, w)| w.verdict.clone().map(|v| (*id, v)))
                .collect(),
        });
    }

    /// W7: diff the live state against the detach snapshot and, if anything changed,
    /// push one "while away — …" briefing notice (deterministic; absent when idle).
    pub fn on_attach(&mut self) {
        let Some(snap) = self.detach_snapshot.take() else { return };
        let mut items: Vec<String> = Vec::new();
        let mut failed = false;
        // Watched tasks that produced a NEW verdict while away (the top signal).
        for (id, w) in &self.watches {
            if let Some(v) = &w.verdict {
                if snap.verdicts.get(id) != Some(v) {
                    items.push(v.clone());
                    let lv = v.to_lowercase();
                    if lv.contains("fail") || lv.contains("error") {
                        failed = true;
                    }
                }
            }
        }
        let exited = self
            .terms
            .iter()
            .filter(|(id, t)| t.exited && !snap.exited.contains(id))
            .count();
        if exited > 0 {
            items.push(format!("{exited} shell{} exited", if exited == 1 { "" } else { "s" }));
        }
        let dirty = self
            .buffers
            .values()
            .filter(|b| b.modified && !snap.dirty.contains(&b.name))
            .count();
        if dirty > 0 {
            items.push(format!("{dirty} file{} modified", if dirty == 1 { "" } else { "s" }));
        }
        if items.is_empty() {
            return; // nothing changed — no briefing
        }
        self.notices.push(Notice {
            text: format!("while away — {}", items.join(" · ")),
            kind: if failed { NoticeKind::Failure } else { NoticeKind::Info },
        });
        self.notices.sort_by(|a, b| a.kind.cmp(&b.kind));
    }

    /// Dismiss the front (highest-priority) notice, if any. Returns true if one popped.
    pub fn dismiss_notice(&mut self) -> bool {
        if self.notices.is_empty() {
            false
        } else {
            self.notices.remove(0);
            true
        }
    }

    /// W6: summarize a watched terminal that just went quiet or exited. One global
    /// in-flight gate (`bg_busy`); a foreground ask always preempts. Runs inside the
    /// daemon's `tick`, so it fires even while detached.
    fn maybe_fire_watches(&mut self) {
        if self.bg_busy || self.agent_pending {
            return;
        }
        let quiet_ticks =
            self.tuning.watch_quiet_secs * 1000 / self.tuning.poll_interval_ms.max(1);
        let now = self.frame_tick;
        // An exit trigger queued from term_rx wins; else the first quiet watched pane.
        let fire = self.pending_watch.take().or_else(|| {
            self.watches
                .iter()
                .find(|(_, w)| {
                    w.watched && !w.triggered && now.saturating_sub(w.last_output_tick) > quiet_ticks
                })
                .map(|(id, _)| (*id, WatchReason::Quiet))
        });
        let Some((id, reason)) = fire else { return };
        if let Some(w) = self.watches.get_mut(&id) {
            w.triggered = true;
        }
        let cfg = agent::AgentConfig::from_env();
        if !cfg.is_configured() {
            return;
        }
        let tail = self.terminal_tail(id, self.tuning.agent_scrollback_context);
        self.bg_busy = true;
        agent::watch_summary(cfg, id, reason, tail, self.agent_tx.clone());
    }

    /// The last `lines` of a terminal pane's visible screen, for a watch summary.
    fn terminal_tail(&self, id: TermId, lines: usize) -> String {
        let Some(t) = self.terms.get(&id) else { return String::new() };
        let contents = t.screen().contents();
        let rows: Vec<&str> = contents.lines().collect();
        let start = rows.len().saturating_sub(lines);
        rows[start..].join("\n")
    }

    /// A " · <tab>/<n panes>" locator suffix for a watched terminal's notice.
    fn tab_label_of_term(&self, id: TermId) -> String {
        for tab in &self.tabs {
            for pid in tab.layout.pane_ids() {
                if let Some(p) = self.panes.get(&pid) {
                    if matches!(p.content, PaneContent::Terminal(tid) if tid == id) {
                        return format!("  · {}", tab.name);
                    }
                }
            }
        }
        String::new()
    }

    /// One-shot AI naming of a still-numeric session (numbered → AI → explicit).
    fn maybe_auto_name_session(&mut self) {
        if self.session_name_attempted || self.tuning.auto_name_secs == 0 {
            return;
        }
        let numeric = self
            .session_name
            .as_ref()
            .map(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false);
        if !numeric {
            self.session_name_attempted = true; // explicitly named already
            return;
        }
        // Give it a little longer than tab-naming so there's real activity.
        let ticks = (self.tuning.auto_name_secs * 2 * 1000 / self.tuning.poll_interval_ms.max(1)).max(1);
        if self.frame_tick % ticks != 0 || self.bg_busy {
            return;
        }
        let cfg = agent::AgentConfig::from_env();
        if !cfg.is_configured() {
            return;
        }
        self.session_name_attempted = true;
        self.bg_busy = true;
        agent::name_session(cfg, self.screen_context(), self.agent_tx.clone());
    }

    /// With an agent configured, quietly name the active tab once it has
    /// content and is still called "1"/"2"/…. Manual renames opt a tab out.
    fn maybe_auto_name(&mut self) {
        let secs = self.tuning.auto_name_secs;
        if secs == 0 || self.bg_busy {
            return;
        }
        let ticks = (secs * 1000 / self.tuning.poll_interval_ms.max(1)).max(1);
        if self.frame_tick % ticks != 0 {
            return;
        }
        let tab = self.tab();
        let (tab_id, default_named) =
            (tab.id, tab.name.chars().all(|c| c.is_ascii_digit()));
        if !default_named || self.auto_name_attempted.contains(&tab_id) {
            return;
        }
        // Only bother once there's something to name.
        let has_content = self.tab().layout.pane_ids().iter().any(|pid| {
            match self.panes.get(pid).map(|p| &p.content) {
                Some(PaneContent::Editor(b)) => {
                    self.buffers.get(b).map(|b| b.rope.len_chars() > 40).unwrap_or(false)
                }
                Some(PaneContent::Terminal(_)) => true,
                None => false,
            }
        });
        if !has_content {
            return;
        }
        let cfg = agent::AgentConfig::from_env();
        if !cfg.is_configured() {
            return;
        }
        self.auto_name_attempted.insert(tab_id);
        self.bg_busy = true;
        agent::auto_name(cfg, tab_id, self.screen_context(), self.agent_tx.clone());
    }

    /// Apply one source-agnostic input event.
    pub fn apply_input(&mut self, ev: InputEvent) -> Result<()> {
        match ev {
            InputEvent::Key(key) => self.handle_key(key)?,
            InputEvent::Mouse(m) => self.handle_mouse(m),
            InputEvent::Paste(s) => self.paste_text(&s),
            InputEvent::Resize(_, _) => {} // session server rebuilds its viewport
        }
        Ok(())
    }

    /// Standalone main loop: draw, tick, and consume events from `events`
    /// (fed by a TTY-reader thread) until quit.
    pub fn run<W: io::Write>(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<W>>,
        events: &mpsc::Receiver<InputEvent>,
    ) -> Result<()> {
        loop {
            terminal.draw(|f| ui::render(f, self))?;
            self.tick();

            match events.recv_timeout(Duration::from_millis(self.tuning.poll_interval_ms)) {
                Ok(first) => {
                    // Apply the first event, then drain whatever else queued.
                    self.apply_input(first)?;
                    while let Ok(ev) = events.try_recv() {
                        self.apply_input(ev)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break, // input source gone
            }

            if self.detach_requested {
                // Standalone has nothing to detach from; session servers use
                // their own loop and consume this flag before we get here.
                self.detach_requested = false;
                self.status_msg =
                    Some("Not in a session — start one with: mars --session <name>".into());
            }
            if self.should_quit {
                break;
            }
        }
        self.save_state();
        Ok(())
    }

    pub fn save_state_now(&self) {
        self.save_state();
    }

    // ── Mouse ────────────────────────────────────────────────────────────────

    /// Click focuses a pane (and positions the cursor); wheel scrolls.
    /// Only active in Edit/Terminal — the bar and prompts own the keyboard.
    pub fn handle_mouse(&mut self, m: MouseEvent) {
        if !matches!(self.mode, Mode::Edit | Mode::Terminal) {
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .pane_rects
                    .iter()
                    .find(|(_, r)| {
                        m.column >= r.x && m.column < r.x + r.width
                            && m.row >= r.y && m.row < r.y + r.height
                    })
                    .map(|(id, r)| (*id, *r));
                let (pane_id, rect) = match hit { Some(h) => h, None => return };
                self.tab_mut().focused_pane = pane_id;
                match self.panes.get(&pane_id).map(|p| p.content.clone()) {
                    Some(PaneContent::Terminal(_)) => self.mode = Mode::Terminal,
                    Some(PaneContent::Editor(buf_id)) => {
                        self.mode = Mode::Edit;
                        // Inner area = rect minus 1-cell border; text starts
                        // after the line-number gutter.
                        let inner_x = rect.x + 1 + crate::ui::gutter_width(&self.tuning);
                        let inner_y = rect.y + 1;
                        if m.row >= inner_y && m.column >= rect.x + 1 {
                            let scroll = self.panes[&pane_id].scroll_row;
                            let row = scroll + (m.row - inner_y) as usize;
                            let row = row.min(self.buffers[&buf_id].line_count().saturating_sub(1));
                            let col = (m.column.saturating_sub(inner_x)) as usize;
                            let col = col.min(self.buffers[&buf_id].line_len(row));
                            self.clear_selection();
                            let p = self.panes.get_mut(&pane_id).unwrap();
                            p.cursor_row = row;
                            p.cursor_col = col;
                            p.col_affinity = col;
                        }
                    }
                    None => {}
                }
            }
            MouseEventKind::ScrollUp => {
                let n = self.tuning.wheel_scroll_lines;
                match self.focused_pane().content {
                    PaneContent::Terminal(tid) => {
                        if let Some(t) = self.terms.get_mut(&tid) {
                            t.scroll_view(n as i64); // back through history
                        }
                    }
                    PaneContent::Editor(_) if self.mode == Mode::Edit => {
                        for _ in 0..n { self.move_up(); }
                    }
                    _ => {}
                }
            }
            MouseEventKind::ScrollDown => {
                let n = self.tuning.wheel_scroll_lines;
                match self.focused_pane().content {
                    PaneContent::Terminal(tid) => {
                        if let Some(t) = self.terms.get_mut(&tid) {
                            t.scroll_view(-(n as i64)); // toward live
                        }
                    }
                    PaneContent::Editor(_) if self.mode == Mode::Edit => {
                        for _ in 0..n { self.move_down(); }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // ── Persisted state (frecency + nudge counters) ──────────────────────────

    fn save_state(&self) {
        let state = PersistedState {
            frecency: self.frecency.clone(),
            bar_uses: self.bar_uses.clone(),
            file_frecency: self.file_frecency.clone(),
        };
        if let Some(path) = config::state_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(&state) {
                let _ = std::fs::write(path, json);
            }
        }
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct PersistedState {
    #[serde(default)]
    frecency: HashMap<String, u32>,
    #[serde(default)]
    bar_uses: HashMap<String, u32>,
    #[serde(default)]
    file_frecency: HashMap<String, u32>,
}

impl PersistedState {
    fn load() -> Self {
        config::state_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}

/// Extract the first fenced ``` code block's body (drops an optional language
/// tag on the opening fence). None if there's no complete block.
pub fn extract_code_block(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    // Skip the rest of the opening-fence line (e.g. ```rust).
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
    let body = &after[body_start..];
    let end = body.find("```")?;
    Some(body[..end].trim_end_matches('\n').to_string())
}

/// Translate a key event into the byte sequence a PTY expects.
fn key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(c) => {
            if ctrl && c.is_ascii_alphabetic() {
                // Ctrl-A..Ctrl-Z → 0x01..0x1a
                vec![(c.to_ascii_lowercase() as u8 - b'a') + 1]
            } else {
                let mut b = [0u8; 4];
                c.encode_utf8(&mut b).as_bytes().to_vec()
            }
        }
        KeyCode::Enter     => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab       => vec![b'\t'],
        KeyCode::BackTab   => vec![0x1b, b'[', b'Z'],
        KeyCode::Esc       => vec![0x1b],
        KeyCode::Left      => vec![0x1b, b'[', b'D'],
        KeyCode::Right     => vec![0x1b, b'[', b'C'],
        KeyCode::Up        => vec![0x1b, b'[', b'A'],
        KeyCode::Down      => vec![0x1b, b'[', b'B'],
        KeyCode::Home      => vec![0x1b, b'[', b'H'],
        KeyCode::End       => vec![0x1b, b'[', b'F'],
        KeyCode::PageUp    => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown  => vec![0x1b, b'[', b'6', b'~'],
        KeyCode::Delete    => vec![0x1b, b'[', b'3', b'~'],
        _ => vec![],
    }
}
