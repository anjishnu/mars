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

## 0.3.0

Agent quality-of-life batch: streaming, a work journal, and a memory subsystem you
can rip out.

### Added
- **Streaming replies**: agent answers render token-by-token in the ask panel
  (SSE for OpenAI-compatible and Anthropic providers), with reasoning-model
  `<think>` blocks stripped incrementally so they never flash on screen.
- **Work journal + mission**: watch-mode frame summaries are logged as work
  snapshots (`~/.mars/worklog.jsonl`); a low-tier model periodically infers the
  session's mission, which `mars ls` shows as the summary column and reattach
  opens with a "Where you left off" briefing.
- **Unified `mars ls`**: local sessions and fleet hosts in one numbered table with
  a shared open prompt; remote agent calls self-report host + session so status
  stays fresh.
- **Model cascade, completed**: rotation across keyed providers on rate limits and
  one-step escalation to a stronger tier on low-confidence answers.
- **Memory hygiene**: secret redaction (credential prefixes, `password=`-style
  values, URL credentials, a user-editable `~/.mars/denylist`) on every line
  bound for a prompt; recency/cwd-weighted retrieval; in-editor actions to open,
  inspect, and clear the command memory.
- **Deletion-proof memory seam**: the whole retrieval subsystem sits behind a
  default-on `memory` cargo feature; `cargo build --no-default-features` yields a
  fully working memory-free terminal.
- **Prompts as Markdown**: every model-facing instruction lives in
  `src/prompts/*.md`, embedded at compile time — editable without touching code.
- **Command bar overhaul**; `quit` now detaches (with `killall` for a hard stop).

### Fixed
- Mouse-wheel scrollback now reaches full-screen terminal apps (Claude Code, less,
  vim): wheel events are forwarded in the app's own mouse protocol, or translated
  to DECCKM-aware arrow keys on the alternate screen.

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
