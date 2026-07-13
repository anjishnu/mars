# Changelog

## 0.3.1

Hardening release: `mars ssh` now recovers from the leftovers of a dead session
instead of failing on them.

### Fixed
- **Stale auth-socket sweep**: a previous session's leftover
  `/tmp/mars-auth-<uid>.sock` on the remote made the reverse tunnel fail to bind
  (with a confusing double password prompt). The ssh prelude now removes it before
  the forward is requested, and the remote side unlinks a dead socket when it finds
  one — no `sshd_config` changes needed.
- **Honest install detection**: the "[mars] not installed here" nudge checked
  `command -v` under sshd's bare non-login PATH, so a cargo-installed mars was
  reported missing on every connect. The check now probes `~/.cargo/bin` and
  `~/.local/bin` directly.
- **No dead-tunnel pinning**: a remote mars that finds a dead auth socket now falls
  back to its normal provider chain instead of sending every agent call into an
  unreachable broker.
- **Cross-uid socket discovery**: the forwarded socket is named with the home
  machine's uid (a Mac's 501), which rarely matches the remote's (Linux's 1000) —
  the remote now scans for any live `/tmp/mars-auth-*.sock` instead of guessing by
  its own uid, so the agent works in shells without `MARS_AUTH_SOCK` exported
  (cron, plain ssh, nested sessions).
- **Honest tunnel status**: `mars ssh` opens the remote shell with
  `[mars] agent tunnel ready` (or a warning if the forward failed) — a working
  connection is no longer indistinguishable from plain ssh.
- **ControlMaster keepalives** (`ServerAliveInterval=30`): a master whose TCP died
  (laptop sleep, network change) exits on its own instead of answering `-O check`
  and then breaking the next connection with "Broken pipe" + a surprise password
  prompt.

## 0.3.0

The agent grows a memory and a spine: provider cascade with tiered routing,
always-on prompt redaction, streaming replies, a work journal that infers what
you're working on — and quality-of-life across the board: quit now detaches,
the command bar fires the top match as you type, and Claude Code scrolls
correctly inside a pane.

> **Beta:** the AI/agent features and the SSH/remote path remain beta. The core
> editor, multiplexer, and sessions are stable.

### Added
- **Provider cascade**: on a 429 the call rotates across every configured
  provider (paid-first) and escalates one model tier when a self-check fails —
  pinned models (`MARS_LLM_MODEL`/`MARS_LLM_URL`) opt out. A **model-tier ring**
  routes each task (naming, missions, translate, ask) to the right size model.
- **Streaming replies**: `?` answers render token-by-token in a live `mars ›`
  turn (all direct providers; reasoning `<think>` blocks are held back, never
  retracted).
- **Memory hygiene**: an always-on redaction pass strips credentials
  (`sk-…`, `ghp_…`, `AKIA…`, JWTs, `Bearer` values, URL passwords) from every
  prompt; `~/.mars/denylist` adds your own strings; retrieval ranking now
  weights recency and working directory. New command-bar rows: *Open command
  memory*, *Forget all commands* (confirm-gated), *Open redaction denylist* —
  the stores are plain files you can read and edit.
- **Memory-free build**: `cargo build --no-default-features` produces a
  terminal with the whole retrieval subsystem compiled out (same selfcheck).
- **Work journal + missions**: watch verdicts persist to `~/.mars/worklog.jsonl`;
  a background inference distills them into a one-line mission shown by
  `mars ls` in a dedicated, wrapped SUMMARY column. Reattaching greets you with
  a deterministic "Where you left off" briefing; *Show all notices* expands the
  pending notice queue into one digest.
- **Unified `mars ls`**: local sessions and remote fleet hosts in one numbered
  table — shared resolver (ordinal/name/prefix), live status pushed home by
  remote agents.
- **Command bar**: in-bar quick keys (`!` `?` `@`) taught as chips and a legend;
  typing pre-selects the top match so Enter fires it immediately; no match
  falls through to natural language (shell-translate in terminals, a grounded
  ask elsewhere). With a selection, "translate this to french" proposes a
  replacement; with just a cursor, "write a limerick about potatoes" inserts at
  point — both confirm-gated, both one undo step.
- **Ask panel**: capped to the bottom ~30% of the workspace
  (`ask_panel_max_pct`), scrollable through past turns with Up/Down or the
  mouse wheel.
- **Quit = detach**: `C-x C-c` leaves the session running; *Kill session*
  (confirm-gated) and `mars kill` end it. **`mars killall`** ends every session
  and starts fresh (refuses to run from inside a session).
- **Prompts as Markdown**: every model instruction ships as an editable
  `src/prompts/*.md`, embedded at compile time.

### Changed / fixed
- **Terminal wheel dispatch** (tmux parity): scrolling works inside Claude
  Code, `less`, and vim — alternate-screen apps get arrow keys, mouse-protocol
  apps get encoded wheel events, everything else scrolls Mars scrollback.
- The cursor-anchored composer overlay yields to the command-bar dropdown when
  the two would overlap.
- `mars llm-stats` profiles the LLM debug log per task×model to right-size
  models per call.
- Latent task-tag mismatch fixed: auto-name/session-name calls now actually
  route through their intended (cheaper) model tier.

## 0.2.0

The first substantial release since 0.1.0 — remote agents, a unified terminal
composer, reattach briefings, and a top-to-bottom ergonomics pass.

> **Beta:** the AI/agent features and the SSH/remote path are new and still being
> hardened. The core editor, multiplexer, and sessions are stable.

### Added
- **SSH broker** (`mars ssh <host>`, `mars keyd`) — **beta**, still being hardened:
  your LLM key stays home and is
  served to remote boxes over the reverse-tunneled socket, so the agent works on a
  host that has no key on it. `mars ssh` auto-starts the home broker and drops a
  self-contained `install.sh` on the remote (rustup + `cargo install`, honest Windows
  error) so a fresh box is one command from running Mars.
- **Fleet view**: `mars ls` lists recent hosts with an interactive `→ ssh:` prompt
  (ordinal / name / unique-prefix resolution).
- **Away Digest** (`C-x g`): a duration-anchored briefing of what happened while you
  were detached — runs finished, shells that exited, files that changed.
- **Unified terminal composer** (`Ctrl+Space` in a terminal): one shell-first surface
  with the red inline overlay AND a ↑/↓ menu of Mars commands. Enter runs your typed
  command; arrow into the menu to run an action instead. `!` forces pure shell, `?`
  asks the agent.
- **Terminal mouse copy**: drag-select to the system clipboard.
- **Watch a pane** (`C-x w` / `C-t w`): summarize a terminal when it goes quiet or
  exits, even while you're detached.
- Nested `mars <file>` inside a session opens a new tab instead of a nested Mars.
- **Space warp** (`C-t`): renamed travel mode, with a `T` verb to open a terminal tab.
- **Mission control** — the command bar (`Ctrl+Space` / `M-x`) is now named mission
  control on every teaching surface (start screen, help, menus).
- **Navigator** — the file sidebar (`C-x C-f`, or `@` in mission control) is renamed Navigator
  and now surfaced on the start screen and as a searchable menu row with its shortcut.

### Changed / fixed (ergonomics)
- **No orphaned shells**: closing a pane/tab now reaps its PTY (kills the child) and a
  live terminal inside prompts for confirmation first.
- **Motor-slip guards**: space-warp `d`/`q`/`0` (destructive keys next to navigation)
  confirm before closing.
- **`C-g` cancels the command bar** from every submode (was silently swallowed).
- **Honest hints**: `binding_for` teaches only chords the terminal can actually send
  (universal over kitty-only ⌘/`C-|`; canonical over aliases) — Save shows `C-x C-s`,
  Search `C-s`, Split `C-x 3`. Reattach/notice hints are mode-aware.
- **Durable failures**: autosave errors go to the persistent notice queue, not the
  status line the next keystroke wipes.
- `Ctrl+Space` opens the command bar from every mode (space warp, time-travel, tree).
- A plain terminal click no longer clobbers the clipboard; scrollback offset reflects
  real history depth.
- Idle SSH sessions no longer flush no-op redraws (latency fix).

## 0.1.0

Initial release: non-modal Emacs-compatible terminal editor, command bar, built-in
LLM agent, tmux-style persistent sessions.
