# Changelog

## 0.4.0 (unreleased)

The mission-aware release: reattaching becomes a save-state restore, the
assistant gains a configurable voice, and the work journal starts carrying
outcomes, not just verdicts.

### Added
- **The shift report**: reattach to a session where things happened and get a
  full-screen briefing — the MARS wordmark, a plain-English situation report in
  the mission-control voice ("Trainer went down at epoch 3, CUDA OOM — needs a
  smaller batch before relaunch. Build's green. Quiet otherwise, captain."),
  then a terse per-workstream manifest (failures first, then blocked ⏸, done,
  running) with a "why" line (cwd · exit · error) under anything that failed.
  The prose streams in; any key resumes exactly where you left off. Shows only
  when something actually happened. Knob `shift_report`: 2 = full screen
  (default), 1 = the classic one-line notice, 0 = off.
- **Goal tracking**: when you detach, the agent captures what you were working
  toward (from the live panes + recent journal), so the reattach briefing
  reports progress against it — "you were trying to get the auth test green;
  it's still failing." Knob `goal_tracking` (default on).
- **Verdict triage ladder**: watch verdicts now escalate one way — free
  deterministic heuristics (exit codes, error/blocked/progress tail shapes),
  then ONE batched low-tier model call for ambiguous rows only. A mars with no
  API key at all now produces deterministic verdicts instead of silence, and
  the report renders instantly with model text streaming in afterwards
  ("telemetry coming in").
- **Auto-watch**: panes that stay busy past `watch_min_active_secs` (10s) are
  watched automatically — the fleet reaches verdicts without arming anything.
  The pane you're looking at is never summarized. Knob `auto_watch` to disable.
- **Blocked verdicts**: a pane waiting on your input is its own class (⏸),
  sorted right after failures in notices and the report.
- **Persona**: the assistant speaks in a configurable voice
  (`~/.mars/persona.md`, "Open persona" in the command bar) — default: mission
  control addressing the ship's captain. Style only: it structurally cannot
  change what the agent does. Empty file turns it off.
- **Outcome-carrying work journal**: watch records now include cwd, the
  command mars ran, the exit code, and a redacted error excerpt on failure —
  the substrate for failure→fix recall. Journal self-compacts
  (`worklog_max_lines`).
- **Open tuning knobs** joins the command bar.

- **AWS Bedrock + Azure OpenAI/Foundry**: MARS now speaks to enterprise model
  gateways. Set `AWS_BEARER_TOKEN_BEDROCK` (+ `AWS_REGION`) to use any Bedrock
  model through the Converse API, or `AZURE_OPENAI_API_KEY` +
  `AZURE_OPENAI_ENDPOINT` (+ `MARS_AZURE_DEPLOYMENT`) for Azure. Bearer/api-key
  auth only — no AWS SigV4, so the single static binary stays dependency-light.
  Both slot into the provider cascade (rotation + tiering) and work over the ssh
  broker with the key never leaving home. (Bedrock is non-streaming for now.)

### Fixed
- **The reattach shift report never appeared after a normal detach**: the
  intended `C-x C-c` quit-detaches path didn't snapshot session state, so the
  save-state restore (and the classic briefing) had nothing to diff against.
  Only an accidental disconnect armed it. Now both do.
- Two panes concluding while detached no longer lose one verdict (the pending
  trigger queue was a single slot).
- Translate calls now actually route through their intended model tier (the
  task tag said "shell", the tier map said "translate" — nobody won).

## 0.3.3

### Added
- **Copy that works over ssh (OSC 52)**: every copy — editor kills, `C-c`,
  terminal mouse selection — now also emits an OSC 52 escape to the real
  terminal, so text copied inside a remote mars session lands on the clipboard
  of the machine you're sitting at. (Previously the daemon wrote to the remote
  box's clipboard, which over ssh is the wrong machine — usually a headless one
  with no clipboard at all.) Requires a terminal that supports OSC 52: iTerm2
  (enable "Applications in terminal may access clipboard"), kitty, WezTerm,
  Alacritty, Ghostty. macOS Terminal.app does not support it.
- **`mars killall` is now the reset button**: gracefully ends every session
  (autosaving), force-kills unresponsive daemons and the key broker, shuts down
  lingering ssh ControlMasters, and sweeps every stale socket. Memory files
  (command memory, worklog, denylist) are untouched, and it no longer starts a
  new session afterwards.

### Fixed
- **Reconnecting no longer breaks the agent tunnel**: reattaching while the ssh
  ControlMaster was still warm deleted the live forwarded socket (the sweep ran
  unconditionally) and the re-requested forward was a mux no-op — leaving the
  remote agent with "no API key". The sweep and the forward request now only
  run on a fresh connection; a reused master keeps its working tunnel.

## 0.3.2

### Added
- **`mars ssh` lands in a mars session**: instead of a bare login shell, you
  arrive inside a remote mars session — the most recent live one, or a fresh
  `main` — with the auth tunnel exported to the session daemon and every shell
  it spawns. Detaching (`C-x C-c`) ends the ssh and returns you to your home
  terminal, tmux-style. Plain `ssh` remains the way to get a bare shell.

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
