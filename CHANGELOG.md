# Changelog

## 0.2.0

The first substantial release since 0.1.0 — remote agents, a unified terminal
composer, reattach briefings, and a top-to-bottom ergonomics pass.

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
