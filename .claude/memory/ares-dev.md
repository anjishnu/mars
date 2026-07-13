# Mars (formerly Ares) development notes

## SSH broker — agent works on keyless remote boxes (2026-07, shipped a55f108/e12d844)
- `src/broker.rs` (new): `mars keyd` = home broker holding the key, binds `$HOME/.mars/auth.sock`
  (0600 under 0700 dir), reuses `session::write_frame` + `read_line`. Protocol: `BrokerRequest::Chat
  {version, model:Option, messages, max_tokens, temperature}` → `BrokerResponse::Chat{text}|Error`.
  keyd `handle_conn` runs `agent::chat` (now `pub`) with a fresh `from_env()` per request.
- Remote side: `AgentConfig` gained `broker_sock: Option<String>`. `from_env()` highest-precedence
  branch: `detect_broker_sock()` (MARS_AUTH_SOCK, else well-known `/tmp/mars-auth-<uid>.sock` if it
  exists) → provider "broker" — UNLESS an explicit MARS_LLM_KEY/ARES_LLM_KEY is set (that wins).
  `chat()` forks to `broker::chat_via_broker` in broker mode (no Authorization header on the box).
  `is_configured()` in broker mode = `UnixStream::connect(sock).is_ok()` (honest when tunnel down).
- `mars ssh <host>`: wraps system ssh — `-R remote_sock:home_sock -o StreamLocalBindUnlink=yes
  -o ControlMaster=auto/Persist=60s -t host "MARS_AUTH_SOCK=… exec $SHELL -l"`. NOT SetEnv (no
  AcceptEnv dep). Records the host in fleet cache + nudges install if mars missing on remote.
- Deferred watch: `maybe_fire_watches` peeks for a candidate first, and if provider=="broker" &&
  !is_configured() (tunnel down) RETURNS WITHOUT consuming the trigger → verdict fires on reattach.
- Fleet: `~/.mars/fleet.json` (FleetEntry{host,cwd,session,last_status,as_of}); `fleet_record` on
  `mars ssh`; `mars ls` (now `list_main(prompt: bool)`) shows local sessions + numbered RECENT HOSTS
  + interactive `→ ssh (number/name)` follow-up via `resolve_target` (ordinal/exact/unique-prefix);
  `--no-prompt` or non-TTY skips. Live status-push from remote daemons = NOT built (next).
- VERIFIED LIVE: `mars keyd` (real GROQ key) + `mars ask` with only MARS_AUTH_SOCK (no key in env)
  returned the answer. 63 selfchecks (broker detect/precedence/availability/round-trip + fleet).
- DESIGN: `design_ideas/ssh_strategy.md` §1.5 (transport: mosh rejected, OpenSSH v1 / russh v2,
  Mode P shipped / Mode E next). DEFERRED: Mode E key-push, russh, Windows TCP fallback, keychain,
  remote→home Status-frame push (needs-you-from-remotes in `mars ls`), bare-`mars` attach-or-create
  (kept tmux-like "new" default; use `mars attach`).

## Rebrand (2026-07)
- Binary/crate = `mars` (repo dir still Ares/ on disk). Config ~/.config/mars/ with
  one-time auto-migration from ~/.config/ares/. Sockets $TMPDIR/mars-<uid>/. Env:
  MARS_LLM_KEY/URL/MODEL, MARS_NO_SYSTEM_CLIPBOARD, MARS_DEBUG_LOG — all fall back to
  the old ARES_* names. Tagline: "mission control for your terminal".
- Palette (theme_* knobs in tuning.json): accent #D97757 terracotta (Claude Code clay),
  bright #E9A178 sand (teaching surfaces), dark #B7410E rust (splash gradient),
  chip fg #1F1410; selection bg #4A2A1F; search bg #8A5414. Rule: brand in chrome,
  not meaning (terminal panes stay green; danger stays red-only-in-confirms).
- Splash: MARS block logo + tagline + starter hints in the empty scratch until first
  key (app.show_splash). Selfcheck asserts "mission control" appears then vanishes.

## Undo: coalesced runs + time-travel mode (2026-07)
- ROOT BUG (fixed): insert_char_at_cursor/delete_before_cursor never checkpoint()'d → typing
  was invisible to undo. Now coalesced via App.edit_run: EditRun{None,Insert,Delete}. In
  handle_edit_primitive: capture prev_run at top, reset edit_run=None; the Char arm checkpoints
  only if prev_run != Insert (a run of typed chars = ONE undo), Backspace arm similarly for
  Delete. run_action resets edit_run=None so any command breaks the run. Enter checkpoints +
  auto_indent (copies prev line's leading whitespace). Bindings: C-/, C-_, C-x u = undo; M-/,
  C-x C-u, cmd-Z = redo; cmd-z = undo (Mac muscle memory, kitty only).
- TIME-TRAVEL MODE (user request): Mode::Undo, entered via Action::UndoMode (M-u + menu row
  "Undo history…"). handle_undo_mode: ←/↑/u = do_undo, →/↓/r = do_redo, Home = undo-all
  (while focused_buf_mut().undo()), End = redo-all, any other key = exit to Edit. Status line
  (undo_status) shows "UNDO ◂ N back · M forward". buffer.undo_depth()→(undo_len,redo_len).
  Verified live: M-u → Home rewinds to file start, End restores, Esc exits.
- GOTCHA: testing undo via screen_text() is flaky (empty-buffer render); assert on
  buffers[id].rope.to_string() directly instead.

## GOTCHA: stale cargo fingerprint masked rebuilds (2026-07)
- After `cargo publish --dry-run` (Jul 3), `cargo build` reported "Finished 0.2s" with NO
  Compiling line even after source edits + touch — target/debug/mars stayed at the Jul 3
  binary and selfchecks silently ran STALE code. Fix: `cargo clean && cargo build`.
- LESSON: if `cargo build` shows no "Compiling mars-terminal" line after an edit, or a brand-new
  selfcheck line doesn't appear in output, suspect the fingerprint cache — check
  `stat -f "%Sm" target/debug/mars` against wall clock before trusting a PASS.
- RELIABLE FIX: `cargo clean -p mars-terminal && cargo build` (faster than full `cargo clean`;
  `touch src/main.rs` alone does NOT bust it). Recurred 2026-07 during the render-loop work.

## Render only when changed + terminal mouse-copy (2026-07, shipped 43150bc + next)
- SSH lag root cause: both render loops (`App::run`, `session::server_main`) drew+flushed EVERY
  tick (~61/s at poll=16) even idle → 61 no-op packets/s over SSH. Fix: `pub needs_redraw: bool`
  on App (init true); tick() sets it on term_rx events, agent_rx events, agent_pending (spinner),
  or non-empty pending_prefix (which-key). Loops reordered tick→draw-if-needs_redraw→recv; input
  arms set it. server_main uses `std::mem::take(&mut app.needs_redraw)`. Idle = zero flushes.
  Users should revert any poll_interval_ms mitigation back to 16 (now cheap).
- Terminal mouse-copy: `pub term_sel: Option<TermSel{tid,ox,oy,vw,vh,anchor,end}>`. handle_mouse:
  Down(Left) on a terminal pane starts a selection at the clicked screen cell; Drag extends end;
  Up copies via `selection_text_from_screen(&screen,a,b,last_col)` (pub(crate) free fn, linear
  text-flow, trailing-space-trimmed) → clipboard + kill_ring + "Copied N chars". ui.rs
  render_terminal_pane highlights selected cells (selection_bg). Wheel-scroll + Cmd+V paste
  ALREADY worked (ScrollUp/Down→scroll_view; paste_text→send_bytes w/ bracketed re-wrap).
  Selfcheck extracts a printf'd row via the free fn. Real drag = real-terminal-only per AGENTS.md.

## Away Digest (2026-07, shipped a1062f8)
- away_log: bounded (200) Vec<AwayEvent{tick, pane, kind: NeedsYou|Done|Context, text, dur_ticks}>
  on App; push_away() appends; ALSO the episodic Tier-1 substrate for the planned memory system.
- Sources: WatchSummary arm (verdict + duration from WatchState.run_started_tick — stamped when
  output resumes after triggered/first output), unwatched TermEvent::Exited ("shell exited"),
  dirty-file names folded as Context at on_attach.
- on_detach stamps detach_tick; on_attach builds ONE headline from events since detach_tick:
  "while away <dur> — ✗ fails · ✓ dones (+N more) · context · <binding> digest", dedupes W6
  notices it subsumes (retain on text equality), sets digest_from_tick. Quiet when empty.
- show_away_digest(): sectioned render (needs you/done/context, relative "Xs ago", "ran Xs")
  pushed into agent_history + open_bar(Ask) — deterministic, no key. Action::AwayDigest, C-x g.
- Broker-ready: only LLM part is verdict TEXT via existing watch_summary→chat seam.
- fmt_dur(ticks): secs = ticks*poll_interval_ms/1000 → "45s"/"4m12s"/"3h02m".

## GOTCHA: bg_busy leak wedged all background AI (2026-07, FIXED)
- Symptom: watch (W6) never produced a summary. Cause: agent::watch_summary/auto_name/
  name_session only sent their AgentEvent inside `if let Ok(chat)…` — on ANY LLM failure
  (rate limit/timeout/bad key) they sent nothing, so bg_busy (set true before the call in
  maybe_fire_watches/maybe_auto_name*) was NEVER cleared → maybe_fire_watches' `if bg_busy
  { return }` gate blocked every future watch + auto-name permanently. One failed bg call
  wedged all background AI. FIX: AgentEvent::BgDone sent unconditionally at the end of every
  bg thread (tick: BgDone→bg_busy=false); watch_summary now also sends an error verdict on
  Err so failures are visible ("⚠ watch couldn't summarize — …"); toggle_watch_pane warns
  if no key. Refresh cadence: tick every poll_interval_ms(16ms); watch fires on TermEvent::
  Exited (shell exit) OR quiet = frame_tick-last_output_tick > watch_quiet_secs(20s)*1000/
  poll_ms. Verified live with GROQ + watch_quiet_secs=3.

## AI workflows W6/W7/W5/W4 shipped (workflows_eng.md, 2026-07)
- Trigger/Watch framework (daemon-resident, in app.rs:tick). W6 (commit 3183471): WatchState
  per TermId fed by term_rx drain (Output resets last_output_tick+triggered; Exit queues
  pending_watch); maybe_fire_watches fires quiet/exit → agent::watch_summary (auto_name clone,
  new AgentEvent::WatchSummary) under one bg_busy gate (renamed from auto_name_inflight; user
  asks preempt via agent_pending). notices: Vec<Notice{text,kind:Failure|Info}> pull-rendered
  by render_notice (bottom line, failures first, Esc=dismiss_notice). Action::WatchPane (C-t w).
  knobs watch_quiet_secs=20, agent_scrollback_context=200.
- W7 (commit 483e8c3): Snapshot{exited,dirty,verdicts} via on_detach/on_attach hooked into
  session.rs server_main ClientGone/Attach arms. on_attach diffs → one "while away — …"
  notice (deterministic, no key; absent if nothing changed). Pairs with W6 (detached verdicts).
- W5/W4 (commit 74d1130): AgentDirective::Need(NeedKind{Scrollback,Tab(String)}) — read-side,
  parsed by match_directive, taught in system_prompt. tick Answer arm: if Need && need_depth<1
  → reask_with_need (rebuilds context via expand_context: Term::history_tail(paged vt100
  scrollback, restores live view) OR named tab's panes) + continue (never surfaced); capped
  at 1. last_question/need_depth set in submit_agent_query. Single-tab cross-pane already in
  screen_context. GOTCHA: adding Need variant needs match arms in handle_bar_ask Enter, ui.rs
  directive label, main.rs ask_cli. 54 selfchecks. DEFERRED: Context Bus registry +
  parameterized actions (RunWith) — no W1-7 consumer, need transaction journal for plans.

## Speed features shipped (speed_design.md steps 1-4, 2026-07)
- STEP 1 motion: KeyModifiers::SUPER detected in handle_edit_primitive; ⌘←/→=move_token_sel
  (code-token: class-run of word/punct, whitespace skipped — token_class helper), ⌘↑/↓=page,
  ⌘⇧=extend. Structural jumps: jump_block (blank line), jump_symbol (col-0 kw heuristic),
  match_bracket — Actions JumpBlockPrev/Next, JumpSymbolPrev/Next, MatchBracket bound
  C-x [ ] { } m. ⌘ only on kitty terminals; M-f/M-b + PageUp/Down are the fallback.
- STEP 2 search-as-teleport: search_labels + search_pick fields. handle_isearch_key: Tab →
  build_search_labels (home-row asdfghjkl over search_hl in doc order) + search_pick=true;
  next key picks a label → jump+accept. Land-on-any-key: the `_` arm ends isearch + re-
  dispatches the key to handle_key. isearch_status()→(cur,total) for the n/m counter (shown
  in the Prompt label; cursor anchored to label+input len, not incl. the counter). Labels
  render as hl kind 3 (label_style chip) in the per-char highlight map.
- STEP 3 unified terminal composer: handle_terminal Ctrl+Space → open_bar(Command) (was
  Shell). handle_bar_command Enter: if items_len==0 && has_query && bar_return==Terminal →
  submit_terminal_shell() (flips to BarMode::Shell + translate_shell_query, or runs directly
  with no key). "if not a command → shell-translate" per user. No double-press.
- STEP 4 selection-aware agent + reversible refactor: refactor_target/refactor_replacement
  fields. submit_agent_query captures selection_range + appends selected_text() block to
  context (tells model: refactor→reply ONLY a ``` block). tick Answer: if refactor_target,
  extract_code_block(text)→refactor_replacement. Ask-panel Enter (empty query) → apply_refactor:
  ONE checkpoint() + rope.remove+insert → reversible via C-/. Panel shows "▶ Enter to replace
  the selection (N lines)". Cleared in close_bar + C-l.
- GOTCHA fixed: selfcheck now hermetic — clears GEMINI/GOOGLE/GROQ/MARS_LLM/ARES_LLM env at
  the top of selfcheck() (an inherited key flipped the shell composer to translate-not-run,
  failing the terminal check). 51 checks pass. Logo: render_splash is now a top-level overlay
  (Clear + centered) gated on show_splash — was editor-pane-only, so terminal-default startup
  hid it.

## GOTCHA: tree selection highlight must be full-width + high-contrast (2026-07)
- Bug report "can't move up/down in the tree, can't type, right opens the file": the tree
  was WORKING (Mode::Tree correct, keys routed) — the SELECTION HIGHLIGHT was just invisible.
  Cause: bg=Color::DarkGray applied only to the short label span (not full row width), and
  the `../` row was DarkGray-fg-on-DarkGray-bg = invisible. Fix in render_file_tree: selected
  row uses bg=theme_accent (terracotta) with fg=theme_chip_fg, and pads a trailing spaces span
  to inner.width so the band spans the WHOLE row (like render_bar_dropdown's selected row).
- DEBUG METHOD that cracked it: python pty.fork + pyte emulator to drive the REAL binary
  (headless TestBackend couldn't show it). Two pitfalls: (1) must answer the DA1/kitty query
  (`\x1b[c`/`\x1b[?u`) with `\x1b[?62;c` or crossterm's supports_keyboard_enhancement blocks
  startup forever; (2) must set pty winsize via ioctl TIOCSWINSZ (struct winsize) or ratatui
  renders to a 0x0 area (blank). pyte's screen.buffer[y][x].bg exposes cell bg to verify
  highlights. Script pattern saved mentally: fork → set winsize → drain+answer-DA → write
  keys → snapshot screen.buffer. Raw-byte grep for typed text FAILS (ratatui interleaves
  cursor moves — the AGENTS.md gotcha); use pyte/vt100.

## speed_design.md (2026-07, PROPOSAL — not built, under review)
- Laser-fast editor+terminal movement & the anchored query. KEY BLOCKER: Ctrl/Alt+arrow
  are currently PANE nav (focus_direction, app.rs ~1347), word-jump only on M-f/M-b — so
  the intuitive "hold key + skip token" gesture is blocked; proposal reclaims Option+arrow
  for token movement, panes → C-o + C-t. Three granularities: word / code-token / subword
  (CamelHumps). Adds half-page (C-d/C-u), block/blank-line jump, symbol jump (col-0 fn/def
  heuristic), matching-bracket, and TELEPORT (avy/easymotion labels) = highest ROI. Part B:
  editor Ctrl+Space = anchored query over the SELECTION (explain/generate/tests/refactor/fix
  /review) — ship read-only+insert now, refactor-replace gated on undo checkpoint/journal.
  Part C: terminal ONE Ctrl+Space composer, shell-first with command suggestions (no more
  double-press), + prompt-jump in scrollback + copy-last-command/output + select-output→query.
  Unifying: Ctrl+Space = "do something here now" in both editor & terminal. 5 decisions to
  confirm before building (see doc). Current editor motions live in handle_edit (app.rs
  ~1333-1360), NOT the keymap; word=move_word_forward/backward, page_up/page_down exist.

## Tree reset-on-close + terminal cwd (2026-07)
- Closing the tree (close_tree(): used by the toggle-hide branch AND handle_tree Esc) now
  sets file_tree=None + clears tree_rows, so reopening rebuilds fresh at the project root
  (forgets any `../` wandering). Opening a FILE keeps the tree open (not a close) so it
  doesn't reset then.
- Terminal cwd: portable-pty's CommandBuilder with NO cwd lands the shell at `/` (not the
  process cwd — confirmed the daemon cwd was correct but the shell still went to root).
  Fix: App.run_cwd = std::env::current_dir() at App::new; open_terminal passes
  startup_cwd.or(run_cwd) so a no-file session's terminal opens where `mars` was launched.
  (startup_cwd = first opened file's dir still wins when a file was opened.)

## GOTCHA: tree root MUST be absolute (2026-07)
- Bug "blank sidebar after Enter on ../": the tree root was the relative "." (from
  startup_cwd=None → project_index root "."), and "." .parent() is an empty PathBuf →
  read_dir("") fails → blank tree + header shows "/". FIX: canonicalize the root to
  absolute in toggle_file_tree when creating the FileTree
  (std::fs::canonicalize(&root).unwrap_or(root)). Now `../` (parent) navigation works.
- Also `../` was invisible: fg was Color::DarkGray. Now theme_accent_bright + a "↑ " glyph.

## Left file-tree sidebar (2026-07, REPLACED the `@` bottom picker)
- User pivoted the `@` bottom fuzzy dropdown → a LEFT sidebar file tree (Mode::Tree).
  BarMode::File + render_file_dropdown + file_matches + longest_common_prefix ALL REMOVED.
- FileTree{root, expanded:HashSet<PathBuf>, selected, filter} + App.tree_open + App.tree_rows
  (Vec<TreeRow>{path,label,depth,is_dir,expanded,updir}). compute_tree_rows(&self):
  filter empty → browse (../ row if root has parent, then push_dir_rows recursing into
  expanded dirs, read_dir_entries reads fs live + skips dotfiles/project_ignore, dirs-first);
  filter non-empty → flat fuzzy shortlist over project_index. refresh_tree_rows() recomputes
  after every mutation + clamps selected. Browse reads fs live (only expanded dirs, cheap) —
  no dir cache.
- Entry: `@` (in bar → close_bar + toggle_file_tree) OR C-x d / C-x C-f / C-x p / C-x b
  (all → Action::ToggleFileTree/FindFile/etc → toggle_file_tree). toggle is tri-state:
  closed→open+focus(Mode::Tree); open+Tree→close; open+Edit→focus. GOTCHA: C-x is an
  Edit-only prefix so you CANNOT press C-x d from inside the focused tree — close via Esc
  (handle_tree Esc: clear filter else close). Opening a file (Enter on file row) → Mode::Edit
  but tree STAYS open (persistent sidebar); re-focus with C-x d from Edit.
- Nav (handle_tree): ↑↓/C-p/C-n move; Right = tree_activate(false), Enter = tree_activate(true)
  — folders/`../` behave the same for both (expand / re-root); for a FILE, Right PREVIEWS
  (show_file_in_pane(commit=false): shows it in the pane, stays Mode::Tree, reversible) while
  Enter COMMITS (commit=true: Mode::Edit). show_file_in_pane reuses an already-open buffer
  (find by path) so repeated previews don't pile up duplicate buffers. ← collapse-or-parent;
  typing filters; Backspace pops filter. Layout: render() carves a left Constraint::Length(tree_width)
  column (knob, default 30, capped at width-20) when tree_open; render_file_tree draws a
  bordered box, folders bold+accent with ▾/▸ carets, `../` dim, indent by depth, selection
  bg only when focused. tree_width knob in tuning.rs.
- Groq/qwen setup (still current): see below block.

## `@` Groq/qwen agent (2026-07)
- Agent providers: Groq default model is now qwen/qwen3-32b (was llama-3.1-8b-instant);
  Gemini = gemini-3.1-flash-lite. Reasoning models (qwen3/R1) emit <think>…</think> —
  chat() strips them via strip_reasoning() before display+parse. RUN: parsing now takes
  only the FIRST token (qwen appends prose to the directive line); TYPE/OPEN keep the
  full rest. agent_max_tokens default bumped 512→1024 (reasoning needs headroom).
  Validated live with the user's Groq key: RUN/OPEN directives clean, triage answers well.

## Sessions-by-default + launch (user rev, 2026-07)
- `mars [file]` is now a SESSION by default (auto-numbered, next_auto_name = lowest free
  int). No file → server/standalone opens a terminal pane (not scratch). `mars -s/
  --standalone [file]` = old no-daemon path (also opens terminal if no file). Terminal
  open in the daemon is gated by env MARS_OPEN_TERMINAL (set by session_main when
  file.is_none()) so selfcheck's server_main(None) stays scratch.
- Session naming: numbered → AI (agent::name_session → AgentEvent::SessionName →
  rename_session_to only if still numeric) → explicit (mars rename / RenameSession wins).
  maybe_auto_name_session in tick, fires once (session_name_attempted), 2× the
  auto_name_secs cadence. Reuses the socket-rename infra.
- terminal::spawn gained a cwd param; App.startup_cwd = parent of first opened file
  (set in App::new from launch file, or open_file first-file-wins); open_terminal uses it.
- Status bar line/col fix: the position readout ("<buf>  Ln N, Col N  ⚡session" for
  editors, "terminal ⚡session" for terminals) is now a SEPARATE right-aligned Paragraph
  drawn over the status area in theme_accent_bright bold — never truncated by left hints
  or hidden by a status_msg (which now trails the hints on the left). Was: single
  right_info string that got hidden by status_msg / truncated when narrow.

## Grounded agent + renames (2026-07)
- Agent is conversational (agent_history, last 12 turns sent; C-l = new chat) and
  screen-grounded: app.screen_context() (~6KB cap) = session/tabs/pane contents
  (editor visible lines + terminal vt100 contents) — the first context-bus slice.
- Directives: RUN: <ActionName> + TYPE: <shell cmd> (agent::AgentDirective, parsed
  by pub agent::parse_directive, unit-tested). TYPE → run_shell_command on explicit
  Enter. Ask panel renders the transcript (you ›/mars ›), adaptive to 60% height,
  Up/Down scroll (ask_scroll = lines up from bottom).
- Renames: RenameTab (travel r), RenamePane (Pane.title override), RenameSession
  (live socket fs::rename — bound listener follows the inode, verified; CLI
  `mars rename <old> <new>` via ClientFrame::Rename). Attached clients survive.
- Auto-naming: tabs with default numeric names only; agent::auto_name kebab-labels
  from screen context every auto_name_secs (45; 0=off); manual rename opts out
  permanently (auto_name_attempted set); user wins races (numeric-name check on
  apply). Gutter now opt-in (line_numbers knob, default false — ui::gutter_width);
  terminal chrome is theme_terminal dark teal #0D7377.

## Phase 1 agentic workflows SHIPPED (2026-07, per workflows_design.md)
- W1 ExplainThis (C-x e) + W2 ExplainFailure (C-x ?, travel ?) → ask_prefilled()
  opens Ask, seeds a canned question, auto-submits (grounded in screen_context).
- OPEN: directive added to AgentDirective (Run/Type/Open); parse_directive now
  lenient (scans last 4 non-empty lines, strips backticks/list markers). app.open_at()
  parses path:line, splits if a terminal is focused, opens + goto line + recenter.
  System prompt gained OPEN + a no-essays rule. Live-verified: triage → OPEN: app.py:87.
- W3 shell Tab-translate: in bar shell mode (Ctrl+Space !), Tab → agent::translate_shell
  → ShellTranslation event replaces the query. Rendered as render_shell_overlay
  anchored at app.cursor_screen (captured in render_panes) — cursor-anchored, no
  eye-jump. Tab is special-cased in handle_bar BEFORE the CMD/ASK toggle.
- Pane resize + zoom: layout.rs HSplit/VSplit carry a `ratio` (15-85, clamped);
  PaneLayout::resize(focused, delta) nudges the innermost split. Tab.zoomed:
  Option<PaneId>; ui render zooms to one pane and auto-clears when focus moves away.
  Travel keys: z zoom, < > resize (- is split-below, can't reuse).
- Banner: src/banner.rs = raw truecolor-ANSI BANNER_LINES (user-supplied planet art)
  + print_banner() for `mars version`. TUI splash (ui::render_splash) parses them via
  ui::ansi_to_line (handles \x1b[38;2;r;g;bm + \x1b[0m only), uniform left-pad to keep
  art aligned, fallback "M A R S" when narrow. GOTCHA: ratatui shows styled Spans not
  raw ANSI — must parse escapes to Spans or you get literal escape codes on screen.
  Splash selfcheck matches "control for your terminal" (banner is capital "Mission").
- 44 selfchecks pass. Phase 2 (W4/W5 context selectors + NEED: expansion) and Phase 3
  (W6/W7 triggers/notices/reattach-brief) still per workflows_design.md, not built.
- Shell composer activation (user rev 2): Ctrl+Space in Mode::Terminal opens the
  INLINE shell composer (BarMode::Shell) directly — no `!` needed; second Ctrl+Space
  (in the bar) → full command bar (BarMode::Command). Editor Ctrl+Space still → command
  bar. `!` from the command bar still enters shell mode (editor path). Translation is
  now Enter-driven: Enter with a key translates NL→command via agent::translate_shell
  (shell_ready flag; command lands in the pill, 2nd Enter runs); Enter with NO key runs
  the text literally; typing/backspace clears shell_ready. Tab still translates (alias).
  "does nothing" was: no GEMINI_API_KEY, or user pressed Enter (ran literal English) —
  fixed by Enter-translates-when-key-present.
- Translate STUCK bug fixed: translate_shell now ALWAYS sends one event (Error if the
  command comes back empty — Gemini thinking models can return ""); chat() got a 30s
  ureq timeout so stalls surface instead of hanging the spinner. chat() also extracts
  real API error messages — GOTCHA: Gemini's OpenAI-compat error body is a JSON ARRAY
  [{"error":{"message"}}], not an object; handle j.is_array() → j[0]. Shell overlay
  shows the error in its hint line (cleared on edit / on successful translation).
- Gutter (user feedback rev): default is now a 1-glyph POINTER gutter (▸ on cursor
  line, POINTER_W=2) not line numbers; line_numbers knob still gives the 6-col number
  column. Status bar shows "Ln N, Col N" (sole position readout). Shell overlay
  repositioned: input row sits ON the cursor row (was cy+1), no [SH !] prefix (text
  starts where the cursor was), accent-pill styling, hint line shows
  "needs GEMINI_API_KEY" when unconfigured. Tab-translate does nothing without a key —
  that's expected; user must export GEMINI_API_KEY and press Tab (not Enter) in shell
  mode (Ctrl+Space then !).

## strategy.md (2026-07, strategy doc — review artifact)
- AI product strategy: sight×persistence thesis; 8 scenarios ranked by ownability×freq
  (1 triage = wedge, 2 remote/SSH, 3 watch-detached, 4 reattach-brief, 5 cross-pane…);
  before/after workflows with time-saved (~45-75 min/day for terminal-heavy dev); 6
  primitives w/ engineering designs (Context Bus, Trigger framework, Parameterized
  actions, Session-as-artifact, Transaction journal, Project index); anti-scenarios
  (no ghost-text, no context-free chat, no head-on Cursor competition — invert: be the
  substrate code-agents run IN); recommendation = own triage, build Trigger framework
  next (turns sight into vigilance, the moat-widener). Companion to agentic_inline.md
  (brief) / workflows_design.md (build spec) / delighters_design.md (nav+polish).

## delighters_design.md (2026-07, APPROVED-PENDING spec, NOT built)
- Navigation + polish delighters. Two substrates: (A) reusable fuzzy Picker (generalize
  the minibuffer Prompt + render_bar_dropdown, reuse fuzzy_score, Tab=longest-common-prefix
  NOT a trie), (B) Project index (lazy, session-cached, skip-list not .gitignore v1, git-root
  or startup_cwd, cap project_index_max=20k). Tier1: file finder (C-x C-f), quick-open
  (C-x p, file_frecency in state.json), buffer switcher (C-x b). Tier2: git gutter (shell
  `git diff -U0` async, marker in the 2nd gutter col, git_gutter knob), autosave ✓ pulse.
  Deferred: cmd-bar starter set (fixed-order ruling), smart paste, dashboard splash
  (terminal-default makes splash rare). User reviewing before implementation.

## Roadmap docs (2026-07, superseded for Phase 1 by the section above)
- `agentic_inline.md` = product brief (10 non-commoditized AI workflows, personas,
  wedge, retention loop). `workflows_design.md` = build spec for the first 7 (W1-W7)
  with enables/disables per choice. Both are DESIGN, no code written yet — user
  reviewing offline before implementation. Phase 1 = W1/W2/W3 (OPEN: directive,
  ExplainThis/ExplainFailure actions, shell Tab-translate + cursor-anchored overlay,
  no-essays prompt) + pane resize/zoom. Phase 2 = W4/W5 (context selectors, NEED:
  expansion). Phase 3 = W6/W7 (trigger framework, notices queue, detach/attach diff).
- Key design decisions to preserve when building: directive vocabulary stays
  trailing-line text (portability + readable confirm gate; parse_directive is the
  seam); one global agent_busy in-flight gate; proactive output is pull-rendered
  (notices queue, never pushed — enforces interruption budget structurally); OPEN:
  is line-only. Reuse the (cx,cy) that render_editor_pane/render_terminal_pane
  already return for the cursor-anchored overlay.

## CLI surface (2026-07)
- Subcommands (with long-flag aliases): mars new/session <name> [file], attach/a/
  resume [name], ls/list, kill <name>, ask "<q>", help/-h/--help, version/-V.
  Unknown -/-- args exit 2 with help (previously they were treated as FILENAMES —
  `mars --help` opened a buffer named --help). README.md = the user instructions.
- `mars ls` shows attached/detached via ClientFrame::Status → ServerFrame::Status
  (connection thread answers from an Arc<AtomicBool> the server loop maintains — no
  server-loop round trip, so it can't hang on a busy session). `mars kill` sends
  ClientFrame::Kill → SrvEvent::Kill → autosave + forced quit (skips dirty guard).

## TTY hygiene (2026-07, user hit this in Warp)
- Killed clients (SIGKILL) can't restore termios → the shell's tty stays raw →
  staircase output (`\n` without `\r`) for everything after, incl. `mars help`.
  Fix: session::sanitize_tty() runs first thing in main() — repairs OPOST/ONLCR/
  ICANON/ECHO on stdout if it's a tty. Doubly important BEFORE enable_raw_mode:
  otherwise crossterm snapshots the broken state as "original" and faithfully
  restores brokenness on exit. session::install_panic_restore() wraps the panic
  hook for both TUI paths (standalone + client) so panics leave a readable message
  and a working shell. Verified in a real pty: `stty -opost` → run mars → `opost`.
  Never verify this with mars stdout redirected (isatty=false → sanitize skips).

## P0 tmux-parity features (2026-07)
- Terminal scrollback: vt100::Parser now created with tuning.terminal_scrollback_lines
  (10k default). Term.scroll_view(delta)/scroll_to_live()/view_offset(); wheel scrolls
  terminal panes, Shift+PgUp/PgDn page, any keystroke snaps to live; title shows
  " terminal ^N " while scrolled. GOTCHA: vt100 0.15 grid.rs has a DEBUG-ONLY integer
  underflow when scroll offset > screen rows (release wraps to correct behavior) —
  worked around with [profile.dev.package.vt100] overflow-checks=false in Cargo.toml.
  vt100 0.16 fixes it but conflicts with ratatui 0.29's pinned unicode-width =0.2.0.
- Dead-shell lifecycle: reader EOF sends TermEvent::Exited -> Term.exited flag (set in
  App::tick) -> pane border rust + "process exited — Enter closes" overlay; Enter/q
  closes pane (close_terminal_pane recycles the last pane into an editor).
- Crash safety: App::autosave() silently saves modified path-backed buffers every
  tuning.autosave_secs (0=off, ticked in App::tick) AND on session detach/disconnect
  (session.rs). Daemon stdout/stderr -> ~/.local/state/mars/<name>.log with
  RUST_BACKTRACE=1 (startup/end/crash lines from main.rs --server arm).

## Build & verify
- Cargo is not on the default PATH: `source ~/.cargo/env && cargo build`.
- Headless verification: `./target/debug/ares --selfcheck` (ratatui TestBackend, 15
  checks incl. a live PTY). Extend it whenever key handling changes — synthesized
  KeyEvents can't catch raw terminal byte encodings, so eyeball real-terminal passes
  for new chords.
- Not a git repository (cloud handoffs that need git will fail).

## Terminal key-encoding gotchas (cost real debugging)
- `C-/` arrives as `C-_` (0x1f) in many terminals → Undo is bound to both.
- `C-@` IS NUL — the same byte legacy terminals send for `Ctrl+Space` → a C-@ set-mark
  alias is physically impossible; selection is Shift+arrows/mouse.
- `M-<` arrives as ALT|SHIFT+'<' → `config::chord_of` strips SHIFT from non-alphabetic
  chars so bindings parse-match; keep that invariant when touching chord code.
- Ctrl+Space may arrive as `KeyCode::Null`; both `handle_edit` and `handle_terminal`
  check for it.

## Doc roles
- `key_design.md` is a VISION document (what should exist + evolution horizons), not a
  status report — user ruling 2026-07-01. Don't rewrite it to describe current code.

## Design invariants (from the approved v2 review plan)
- Every hint surface derives from `KeyBindings::binding_for` — never hardcode a
  keybinding string in menus/hints (remaps would make the UI lie).
- Empty-query bar menu is fixed-order; frecency is a search-result tiebreaker only.
- Destructive actions (quit/close/kill) always confirm; quit passes the dirty-buffer
  guard. Agent `RUN:` of a destructive action requires y/n.
- Frecency + nudge counters persist in `~/.config/ares/state.json`; user keybindings in
  `~/.config/ares/keys.json` are layered OVER defaults (new defaults appear in old
  configs; user entries win).
- Movement rulings (2026-07, rev 2): C-t = TRAVEL MODE (one-char verbs + cheat panel;
  new tab = C-t t; creation exits, navigation stays); C-c = copy (line if no
  selection), C-v = system paste (Emacs C-c prefix and page-down gone by ruling);
  M-o/M-arrows = panes; C-{ C-} (kitty-protocol) + M-{ M-} M-1..9 C-PgUp/PgDn = tabs;
  C-| / C-- splits (kitty) with C-\ / M-- universal twins; M-g = goto-line;
  C-x x = swap pane. Shifted punctuation can't be a chord on legacy terminals —
  kitty keyboard protocol (crossterm PushKeyboardEnhancementFlags, gated on
  supports_keyboard_enhancement) unlocks it; every modern chord has an Alt twin.
- Clipboard: arboard crate; kills/copies also set OS clipboard; crossterm needs the
  "bracketed-paste" feature for Event::Paste. ARES_NO_SYSTEM_CLIPBOARD=1 disables
  clipboard init (selfcheck sets it — keeps tests off the user's real clipboard).
- Round-3 rulings (2026-07): C-o + Ctrl+arrows = pane nav (Alt isn't Meta on stock mac
  terminals — that's why M-o felt broken); chrome layer = navigation chords work inside
  terminal panes (is_chrome_action set in app.rs), editing chords never intercepted;
  cmd-/super- parse to SUPER (cmd-c/v/s/a bound; only super-reporting terminals
  deliver them); tuning.json = all behavioral knobs as {value, description}
  (src/tuning.rs, layered like keys.json); agent env precedence ARES_LLM_KEY >
  GROQ_API_KEY > GEMINI_API_KEY (Gemini via its OpenAI-compatible endpoint
  generativelanguage.googleapis.com/v1beta/openai, model gemini-flash-latest —
  use the alias: pinned versions age out of the free tier, 2.0-flash hit quota 0).
  Verified live 2026-07-01 with the user's key. `ares --ask "<question>"` = headless
  end-to-end agent test (prints provider/model/answer/RUN directive). Newer Gemini
  flash models think by default — keep max_tokens ≥512 or answers come back empty
  (finish_reason: length, all budget spent on reasoning).
- Selfcheck isolates config via XDG_CONFIG_HOME=temp dir (immune to user remaps and
  proves default-file writing for keys.json + tuning.json).
- Session daemon (2026-07, src/session.rs): thin client, server renders — daemon runs
  the same App the selfcheck already proved works headless; input = deserialized
  crossterm events over a unix socket (`$TMPDIR/ares-<uid>/<name>.sock`, mode 0700);
  output = ratatui's own ANSI bytes captured by pointing CrosstermBackend at a
  socket-backed Write sink (FrameWriter) instead of stdout. `ares --session <name>`
  spawns `ares --server <name>` detached (setsid via libc::setsid in pre_exec) and
  attaches; `--resume [name]` reattaches (most-recent socket mtime if unnamed);
  `--list` pings each socket, prunes dead ones. One client per session; new attach
  sends the old one an Exit frame (takeover). Detach (C-t D / bar row) leaves the
  session running; C-x C-c ends it (dirty-guard still applies) and removes the socket.
  `App::run` was refactored to take an `InputEvent` receiver instead of reading
  crossterm directly (`app.rs` step/tick split) — standalone mode spawns a TTY-reader
  thread feeding the same channel type the server consumes from sockets.
  GOTCHA (cost ~1hr): don't write ad-hoc test helpers that re-`try_clone()`+drop a
  socket per call — works fine, was a red herring. The REAL bug: ratatui's incremental
  cell-diffing interleaves cursor-repositioning escape codes BETWEEN individual
  changed characters (one draw per keystroke), so typed text never appears as a
  contiguous substring in the raw ANSI byte stream. Test/verify session output through
  a real ANSI parser (vt100 — already a project dep) and check the INTERPRETED screen
  contents, never raw-byte-contains() on accumulated Output frames.
  Verified manually end-to-end via `script -q /dev/null ares --session/--resume` +
  `ps`/`--list` (headless client_main can't be exercised without a real/pty TTY).
  `ARES_DEBUG_LOG=<path>` env var (session.rs `debug_log`) writes timestamped
  diagnostics for hello/parse/read errors — zero-cost when unset, useful for future
  daemon debugging since a detached daemon has no visible stderr.

- Replacing the installed binary (`~/.cargo/bin/mars`) with `cp` over the existing
  file gets the new binary SIGKILLed on launch (exit 137) — macOS AMFI caches the
  code signature by inode. `rm` the old binary first, then `cp` (or use `mv`).
- Two build SKUs since the memory feature gate: `cargo build` and
  `cargo build --no-default-features` (retrieval_stub.rs) — BOTH must pass
  --selfcheck when touching retrieval.rs or its facade callers.
- `ssh -o StreamLocalBindUnlink=yes` on the CLIENT is a no-op for `-R` (remote)
  unix-socket forwards — only the server's sshd_config honors it there. A stale
  /tmp/mars-auth-<uid>.sock on the remote makes the -R bind fail (and the mux
  forward failure cascades into a second password prompt). mars sweeps it in the
  ssh prelude (`remote_prelude_cmd`) and remote-side via `probe_and_sweep`.
- `command -v mars` in an ssh remote-command runs under sshd's bare non-login
  PATH (no ~/.cargo/bin) — probe install dirs explicitly, don't trust PATH.
- Local Cargo.toml version can lag crates.io: 0.3.0 was published out-of-band
  (2026-07-11) while the repo said 0.2.0 — check `crates.io/api/v1/crates/
  mars-terminal` max_version before bumping for publish.
