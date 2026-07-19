# Mars — Architecture Overview

*A file-by-file tour of the codebase: what lives where, how the pieces connect, and
the patterns that hold it together. Companion to [`DESIGN.md`](./DESIGN.md) (rationale
and tradeoffs) and [`key_design.md`](./key_design.md) (UX doctrine and vision) — this
document is the map; those are the argument.*

Mars is a single Rust binary (`src/main.rs` is the only `[[bin]]`), ~9,500 lines
across 16 modules, built on ratatui + crossterm, with `ropey` for text,
`portable-pty` + `vt100` for terminal panes, `ureq` for the LLM agent, and Unix
domain sockets for session persistence.

## 1. The big picture

Three ideas shape everything:

1. **One action registry, many retrieval paths.** Everything runnable is a variant
   of `Action` (`palette.rs`). Keybindings, the fuzzy command bar, travel mode, and
   the LLM agent's `RUN:` directives all resolve to an `Action` and funnel through a
   single dispatch point, `App::run_action`. Adding one `Action` variant makes a
   capability chord-bindable, bar-searchable, and agent-invokable at once.

2. **A source-agnostic core.** `App` (`app.rs`) never reads a TTY and never writes
   to one. Input arrives as `InputEvent` values (key / mouse / paste / resize);
   output happens by `ui::render` painting a ratatui backend. Because the core
   doesn't care whether that backend is a real terminal, a socket, or a test buffer,
   the same `App` runs in three configurations with zero forks: standalone mode
   (real TTY), the session daemon (socket-backed, headless), and `--selfcheck`
   (`TestBackend`, no TTY at all).

3. **Thin client, server renders.** Session persistence is not a save/restore
   layer — it's a process split. The daemon owns the `App` and renders frames;
   the client owns the TTY and pumps bytes. Terminal panes and agent threads never
   depended on anyone watching them, so detach is free.

```
                 ┌────────────────────────────────────────────┐
                 │              mars (one binary)             │
                 └────────────────────────────────────────────┘
   mars -s file          mars / mars new work          mars --selfcheck
   (standalone)        (sessions by default)             (headless CI)
        │                        │                            │
        │             ┌──────────┴──────────┐                 │
        │             │ client   ⇄  daemon  │ local control:  │
        │             │ (TTY)      (App)    │ ClientFrame /   │
        │             └──────────┬──────────┘ ServerFrame     │
        ▼                        ▼                            ▼
   ┌─────────────────────────────────────────────────────────────┐
   │  App (app.rs) — all state, all behavior                     │
   │    apply_input(InputEvent) → mode handlers → run_action()   │
   │    tick() — drains PTY + agent channels, autosave, watches  │
   ├─────────────────────────────────────────────────────────────┤
   │  ui.rs      renders &App each frame (stateless projection)  │
   │  palette.rs Action registry + command-bar menus/search      │
   │  config.rs  keymap        tuning.rs  behavior knobs         │
   │  buffer/pane/layout/tab/mode  — the data model              │
   │  terminal.rs PTY panes    agent.rs  LLM threads             │
   │  project.rs file index    banner.rs  splash art             │
   └─────────────────────────────────────────────────────────────┘
```

## 2. Module map by layer

| Layer | Files | Lines (approx) |
|---|---|---|
| Entry & test harness | `main.rs` | 1,440 |
| Application core | `app.rs` | 3,690 |
| Rendering | `ui.rs` | 1,370 |
| Data model | `buffer.rs`, `pane.rs`, `layout.rs`, `tab.rs`, `mode.rs` | 460 |
| Command surface | `palette.rs`, `config.rs`, `tuning.rs` | 1,060 |
| Subsystems | `terminal.rs`, `agent.rs`, `session.rs`, `sys/`, `project.rs`, `banner.rs` | 1,450 |
| Memory (feature `memory`, default-on) | `retrieval.rs` (`retrieval_stub.rs` when off) | 700 |

Dependencies point downward-ish: `main` → `session`/`app`; `session` → `app`;
`app` → everything; `ui` reads `app`; the data-model and subsystem files depend
only on each other in small, local ways. There are no circular ownership
relationships — `App` owns all state; `ui.rs` is functions over `&App`.

## 3. Entry point and orchestration — `main.rs`

The CLI dispatcher and the test suite.

- **Subcommands**: `help`/`version`; `new`/`session <name>` (create-or-attach);
  `attach`/`resume [name]`; `ls`/`list`; `kill <name>`; `rename <old> <new>`;
  `ask "<q>"` (headless one-shot agent query); `--selfcheck`; `--server <name>`
  (internal — the daemon body, spawned by `session_main`, never called directly);
  `-s`/`--standalone` (no daemon). A bare `mars [file]` is **sessions-by-default**:
  it computes the next free auto-numbered session name and delegates to
  `session::session_main`, tmux-style. Unknown `-`/`--` flags exit 2 with help;
  bare arguments are filenames.
- **Standalone event loop**: raw mode + alternate screen + mouse + bracketed paste
  + kitty keyboard flags (when supported), a TTY-reader thread mapping crossterm
  events to `InputEvent`s over an mpsc channel, then `App::run`. The *daemon's*
  event loop is not here — it lives in `session.rs`.
- **`sanitize_tty()` runs first thing** in `main`, before crossterm can snapshot
  terminal state: a SIGKILL'd previous client leaves the TTY raw, and every `mars`
  invocation repairs that on startup.
- **`selfcheck()`** is most of the file: ~54 checks (numbered 1–29, many with
  sub-checks) driving the real `App`
  against `ratatui::TestBackend` with no mocks. It hermetically clears inherited
  agent keys, redirects `XDG_CONFIG_HOME` to a temp dir, and disables the system
  clipboard, then exercises everything from kill-ring semantics to search-teleport
  labels, the terminal composer, watch notices (W6), the reattach briefing (W7),
  and NEED-directive depth-capping (W5/W4). Check 27 starts a **real session
  daemon on a thread** and drives it through a `TestClient` over a real Unix
  socket — including version-handshake refusal, client takeover, PTY survival
  across disconnect, and live rename. Screen assertions go through a `vt100`
  parser, never raw byte matching (see the gotcha in `AGENTS.md`).

## 4. The application core — `app.rs`

The largest file by design: `App` is the single owner of all state — buffers,
panes, tabs, terminals, the palette, prompts, search state, the agent
conversation, watch state, notices — and all behavior. No rendering lives here.

**Input path.** `apply_input(InputEvent)` fans out to `handle_key` /
`handle_mouse` / `paste_text`. `handle_key` routes on the current `Mode` to one
handler per mode: `handle_edit`, `handle_bar`, `handle_prompt`, `handle_tab`
(travel mode), `handle_terminal`, `handle_tree`. `handle_edit` runs the Emacs
prefix-key state machine (`pending_prefix` + `KeyBindings::lookup`), then a table
of modified-key editing primitives, and only then falls through to plain
insertion — so a bare keystroke can never run a command (the non-modal safety
argument in `DESIGN.md` §4).

**Command path.** `run_action(Action)` is the one dispatch point for every
command regardless of origin (chord, bar, travel mode, agent `RUN:`). It also
maintains frecency counters and breaks the `M-y` yank chain — cross-cutting
invariants live at the funnel, not at each call site.

**Main loop.** `run` is draw → `tick` → block on the input channel with a
timeout. `tick` does the per-frame housekeeping: drain `TermEvent`s from PTY
reader threads, drain `AgentEvent`s from LLM threads, autosave on a timer, fire
watch summaries, auto-name tabs/sessions. The session daemon calls exactly the
same `tick` — which is why watches keep firing while you're detached.

Functional areas worth knowing (all methods on `App`):

- **Editing primitives** — cursor motion, insert/delete, selection
  (anchor-based, `selection_range`), kill-ring + system clipboard (`push_kill`
  writes both), paste routing per mode.
- **Fast motion** — `move_token_forward/backward` (code-token hops for ⌘/⌥
  arrows), `jump_block` (blank-line blocks), `jump_symbol` (column-0
  `fn`/`def`/`class` heuristic), `match_bracket`.
- **Incremental search / teleport** — `update_isearch` jumps live while typing;
  `build_search_labels` assigns home-row labels to visible matches (Tab);
  land-on-any-key commits the search and applies the key. `search_origin` makes
  `C-g` restore where you started.
- **Panes/tabs** — splits bounded by `tuning.max_panes`, geometric
  focus-by-direction using last-render `pane_rects`, zoom, swap;
  `kill_buffer` retargets every pane showing the killed buffer (stale-`BufferId`
  safety invariant).
- **File tree** — `FileTree` state + `compute_tree_rows` (browse tree vs.
  fuzzy-filtered shortlist over `project::Index`), preview (`→`, reversible) vs.
  open (`Enter`).
- **Terminal panes** — `is_chrome_action` defines exactly which chords (pane/tab
  navigation) pierce a focused terminal; everything else is translated to PTY
  bytes by `key_to_bytes`. `Ctrl+Space` opens the unified composer.
- **Agent integration** — `submit_agent_query` ships question + `screen_context`
  (a size-capped slice of what you see, plus the exact selection if any) +
  action registry + conversation history to a background thread; `tick` consumes
  the reply. Directives are confirm-gated (see §8). `apply_refactor` replaces the
  captured selection with the model's code block as **one undo step**; with no
  selection the target is an empty range at the cursor, so the block inserts at
  point ("write a limerick about potatoes").
- **Watch & briefing (W6/W7)** — `WatchState` per terminal fed by the PTY drain;
  `maybe_fire_watches` summarizes on exit or quiet via a background thread;
  results land in a pull-model `notices` queue (the agent's *only* path to the
  screen). `on_detach` snapshots cheap facts; `on_attach` diffs them into a
  deterministic "while away —" notice — no LLM involved.
- **Persistence** — `PersistedState` (frecency, bar-usage nudge counters, file
  frecency) in `~/.config/mars/state.json`; `autosave` writes dirty file-backed
  buffers on a timer and on detach.

**Undo grouping**: `Buffer::checkpoint()` is called once per logical edit —
a paste, a typed run, an applied refactor — so each is one reversible unit.

## 5. Rendering — `ui.rs`

Free functions over `&App`; ratatui repaints the full frame each tick and diffs
cells itself. Rendering is a pure projection of App state with one deliberate
exception: it writes back render-derived geometry (`pane_rects`,
`cursor_screen`, per-pane `view_h`) that App's mouse hit-testing and overlay
anchoring need next frame.

- **Fixed chrome**: tab bar (1 row) · pane area · status bar (1) · control bar
  (1); the file-tree sidebar is carved from the pane area's left edge.
- **Panes**: `compute_rects` walks the `PaneLayout` tree into rectangles;
  editor panes render with a per-character highlight map (selection / search
  match / teleport label in a single styled pass); terminal panes render the
  `vt100` screen grid cell-by-cell, including a scrollback-offset indicator and
  an exit banner for dead shells.
- **Overlays**, drawn last and grown upward from the control bar: the command-bar
  dropdown, the ask panel (agent transcript with confirm lines for pending
  directives/refactors), the inline shell composer (anchored at the terminal
  cursor — no eye-jump; it yields to the dropdown when the two would collide),
  the which-key continuation panel (appears after a
  hesitation delay on a pending prefix), and the travel-mode cheat panel. The
  splash banner (`banner.rs` art, parsed from raw ANSI by `ansi_to_line`)
  overlays everything at startup until the first keypress.
- **The honesty invariant lives here**: every hint surface — status-bar hints,
  dropdown badges, which-key rows, the idle control-bar line — calls
  `KeyBindings::binding_for(&Action)` at render time. No binding string is ever
  stored in UI code, so a remap in `keys.json` updates every surface at once.

## 6. The data model — `buffer.rs`, `pane.rs`, `layout.rs`, `tab.rs`, `mode.rs`

Five small files, deliberately dumb:

- **`buffer.rs`** — `Buffer`: a `ropey::Rope` plus name/path/modified flag and a
  snapshot undo/redo stack (`checkpoint`/`undo`/`redo` clone the whole rope —
  simple and correct; the planned cross-buffer transaction journal is a known
  future substrate, per `DESIGN.md` §4).
- **`pane.rs`** — `Pane`: cursor, column affinity, scroll, selection anchor,
  optional title, and `PaneContent::Editor(BufferId) | Terminal(TermId)` — a
  pane is a *view*, pointing at content it doesn't own.
- **`layout.rs`** — `PaneLayout`: a binary tree of `Single`/`HSplit`/`VSplit`
  with clamped split ratios; supports split/remove (sibling promotion),
  next/prev traversal, and deepest-first resize around the focused pane.
- **`tab.rs`** — `Tab`: a `PaneLayout` + focused pane + name + optional zoomed
  pane. Eleven lines of state; all behavior lives in `App`.
- **`mode.rs`** — `Mode`: `Edit`, `Bar`, `Prompt`, `Tab` (travel), `Terminal`,
  `Tree` — the top of `handle_key`'s routing, plus each mode's status-bar chip
  and hint pairs (Edit's hints are intentionally empty here: they're derived
  live from the keymap in `ui.rs`).

## 7. The command surface — `palette.rs`, `config.rs`, `tuning.rs`

- **`palette.rs`** — the `Action` enum (the registry), each action's `label()`,
  the `is_destructive()` set (Quit/CloseTab/KillBuffer/ClosePane — these confirm
  before firing, whether a human or an agent asked), the curated menu tree
  behind the command bar, `fuzzy_score` (subsequence match with contiguity and
  word-boundary bonuses), and `Palette` state (menu stack, query, `BarMode:
  Command|Ask|Shell`). Two rules encoded here: an **empty query renders the menu
  in fixed order** (spatial memory; frecency is only ever a tiebreaker on
  searches), and `registry_context()` generates the live action catalog the LLM
  receives — so agent answers cite real commands, and `Action::from_name`
  round-trips its `RUN:` directives.
- **`config.rs`** — `KeyChord`/`KeyBindings`: parses Emacs notation (`C-x C-s`,
  `M-<`), long forms (`ctrl-x`), and `cmd-` (⌘, kitty-protocol terminals only)
  into chord *sequences*; computes the prefix set for the pending-prefix state
  machine; loads `~/.config/mars/keys.json`, layering defaults under user
  entries so new default bindings appear in old files. `binding_for(&Action)`
  (shortest sequence wins) is the single source of truth for every UI hint.
  `chord_of` normalizes real-terminal quirks (e.g. dropping SHIFT on
  non-alphabetic chars, since `M-<` arrives as ALT|SHIFT+`<`). Also owns the
  config-dir resolution, including one-time `~/.config/ares` → `mars` migration.
- **`tuning.rs`** — every behavioral magic number as a named knob in
  `~/.config/mars/tuning.json`, stored as `{"value": …, "description": "…"}`.
  The description makes the file safely editable by a human *or an agent* asked
  to change editor behavior. Covers timings (poll interval, which-key delay,
  watch quiet threshold), limits (max panes, scrollback, project index cap),
  the whole color theme, and agent sampling parameters. Same layering lifecycle
  as `keys.json`.

## 8. The subsystems

### `terminal.rs` — PTY panes

`spawn` runs the platform shell on a `portable-pty` PTY and pumps its output into
a `vt100::Parser` on a dedicated reader thread. A separate process watcher owns and
polls the child, handles pane-close kill requests, and emits `TermEvent::Exited`
after a bounded final-output drain; this is required because ConPTY can keep its
output pipe open after the child exits. **The parser and shell run whether or not
anyone is watching** — this property is what makes session detach free.
`Term` also owns scrollback view state (`scroll_view`, `scroll_to_live`) and
`history_tail(lines)` — the method that pages back through vt100 scrollback
(and restores the live view) to satisfy the agent's `NEED: scrollback` requests.
Every fresh terminal buffers input until a recognized prompt or retryable
shell-readiness marker proves that profile startup has completed.

### `agent.rs` — the LLM layer

Stateless functions over any OpenAI-compatible `/chat/completions` endpoint.
`AgentConfig::from_env()` resolves provider by precedence: `MARS_LLM_KEY`
(custom URL/model, e.g. local Ollama) → `GROQ_API_KEY` (default
`qwen/qwen3-32b`) → `GEMINI_API_KEY`/`GOOGLE_API_KEY` (Gemini's OpenAI-compat
endpoint); legacy `ARES_*` names still honored. Five fire-and-forget entry
points (`ask`, `translate_shell`, `watch_summary`, `auto_name`, `name_session`)
each spawn a thread, POST with a 30s cap, and deliver one `AgentEvent` over the
caller's channel — nothing here ever blocks the UI; `App::tick` drains results.

The system prompt is a contract: be terse, ground answers in the embedded live
screen and the `registry_context()` action catalog, and end with **exactly one
directive** on the final line:

| Directive | Meaning | Gating |
|---|---|---|
| `RUN: <ActionName>` | fire an editor action from the registry | confirm-gated; destructive actions get the full confirmation prompt |
| `TYPE: <command>` | type a shell command into the terminal pane | confirm-gated |
| `OPEN: path:line` | jump to a file/line (stack traces) | confirm-gated |
| `NEED: scrollback` / `NEED: tab <name>` | request more context (W5/W4) | never shown to the user; Mars re-asks **once** with the extra source (`need_depth` hard-capped at 1 — a loop is structurally impossible) |

`parse_directive` tolerates markdown noise and post-directive sign-offs;
`strip_reasoning` removes `<think>…</think>` blocks from reasoning models
(Qwen3, DeepSeek-R1) on every response.

### `broker.rs` + `ssh.rs` — key-never-leaves-home

`broker.rs` owns the portable JSON request protocol, the `mars keyd` service, and
the remote chat proxy. keyd listens through `sys::control`: a protected Unix
socket on Unix or token-authenticated loopback TCP on Windows.

`ssh.rs` owns system-OpenSSH lifecycle and remote POSIX command construction.
Unix retains connection multiplexing. A Windows home uses a per-invocation
capability relay and `-R remote-unix-socket:local-tcp`; the relay authenticates
the remote bytes, then opens the protected local keyd channel. The current socket
and capability travel in the session `Hello`, so reattaching a persistent remote
daemon replaces its dead prior tunnel route. SSH child environments explicitly
remove provider credentials before OpenSSH can apply user `SendEnv` rules. A
separate prelude stages the embedded installer and runs it only for a missing or
handoff-incompatible remote Mars; Windows may therefore authenticate twice.

### `session.rs` — persistence as a process split

The tmux-style client/server implementation. The wire protocol is newline-delimited
JSON over a platform-local control stream: a mode-0700 Unix-domain socket on Unix,
or nonce/HMAC mutually-authenticated loopback TCP with a rendezvous file on Windows. Addresses
normally live under the platform temp directory in `mars-<user-tag>`; the
`MARS_RUNTIME_DIR` base override makes selfcheck isolation explicit.
Control probes distinguish live, definitively dead, and indeterminate endpoints,
so an upgrade or authentication timeout never unlinks a live daemon's address.
Client→server is `ClientFrame` — `Hello{cols,rows,version,broker_*}` (strict
protocol-qualified version handshake plus optional live SSH broker handoff),
`Key`/`Mouse`/`Paste`/`Resize`, plus one-shot control frames
`Status`/`Kill`/`Rename` used by `mars ls`/`kill`/`rename`. Server→client is
`ServerFrame` — `Output{b64}` (one rendered frame's ANSI bytes), `Exit{message}`,
`Status`.

- **`server_main`** (the daemon): owns the `App`, accepts connections on a
  listener thread, and runs draw → `tick` → recv. The ratatui terminal writes
  into a `FrameWriter` — a `Write` impl that buffers a frame and ships it as one
  `Output` on flush, marking the client dead on IO error (2s write timeout) so a
  wedged client can never stall the session. With no client attached there is
  simply no terminal to draw to; `tick` keeps running, which is what keeps PTYs,
  autosave, and watch summaries alive while detached. Attach triggers
  `App::on_attach` (the W7 briefing diff); disconnect triggers `on_detach` +
  autosave. Generation counters on connections guard against a stale client's
  disconnect or input affecting its successor. The one-shot `BrokerRoute`
  control frame lets Mars subprocesses in persistent PTYs resolve the current
  attach's socket and capability rather than their inherited environment; an
  immutable instance ID keeps that lookup valid across session renames.
- **`client_main`**: owns the real TTY (raw mode, alt screen, mouse, bracketed
  paste, kitty flags), one thread pumping `Output` frames to stdout, one loop
  serializing input events to the socket. One client per session; a new attach
  sends the old client a clean takeover `Exit`.
- **`session_main`**: attach-if-alive, else spawn `mars --server <name>` fully
  detached (`setsid` on Unix, detached process flags on Windows; stdio goes to the
  per-session postmortem log), wait for the address, attach. Live rename moves the
  address file without disturbing the already-bound listener or attached clients.
- **TTY hygiene**: `sanitize_tty` (idempotent raw-mode repair) and a panic hook
  that restores the terminal before the panic message prints.

### `project.rs` — the file index

A bounded, lazily built, session-cached walk of the project root (nearest
ancestor with `.git`), skipping dotdirs and the `tuning.project_ignore` list,
capped at `project_index_max` files. Feeds the `@` file tree's fuzzy filter.
(v1 is a skip-list, not `.gitignore`-aware — the `ignore` crate is the
documented upgrade path.)

### `banner.rs` — the splash

Generated ANSI art (`BANNER_LINES`, truecolor SGR escapes) with `print_banner`
for `mars version`; the TUI splash parses the same lines into ratatui spans via
`ui::ansi_to_line`. Machine-generated — don't edit by hand.

## 9. Threading model

The main thread owns `App` exclusively — there are no locks around editor state.
Everything else is a producer on an mpsc channel:

| Thread | Created by | Sends |
|---|---|---|
| TTY reader (standalone) / socket connection threads (daemon) | `main.rs` / `session.rs` | `InputEvent` / `SrvEvent` |
| One PTY reader per terminal pane | `terminal::spawn` | `TermEvent::Output/Exited` |
| One thread per LLM call | `agent.rs` entry points | one `AgentEvent`, then exits |
| Client frame pump | `client_main` | decoded ANSI → stdout |

The only shared-state exceptions: each terminal's `vt100::Parser` sits behind an
`Arc<Mutex<…>>` (written by its reader thread, read by the renderer), and the
daemon shares two atomics (`attached`, a connection generation counter) with its
connection threads. Background agent work is additionally serialized by a single
`bg_busy` slot in `App`, and foreground asks preempt it.

## 10. Testing

`./target/debug/mars --selfcheck` is the primary suite (see §3 and `AGENTS.md`):
headless, hermetic, no mocks, real PTYs, a real daemon over a real socket. Extend
it for new behavior rather than adding a parallel harness. What it *cannot* verify
— real terminal byte encodings, kitty-protocol negotiation, and `setsid`
process-detachment — needs a manual real-terminal pass (`DESIGN.md` §9).

## 11. Deferred, by design

Two substrates are designed (`workflows_eng.md`) but deliberately unbuilt because
no shipped feature needs them yet: the **Context Bus registry** (formalizing
`screen_context` into consented `ContextSource` objects) and **parameterized
actions** (`RUN: FindFile("x")`), which gates multi-step agent plans and itself
waits on a cross-buffer **transaction journal** (reversibility before autonomy).
Subword motion (`⌘⌥←/→`) is a planned fast-follow.
