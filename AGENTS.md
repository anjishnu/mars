# AGENTS.md

Instructions for AI coding agents working in this repository. (`CLAUDE.md` in this
directory is a symlink to this file, so Claude Code picks it up automatically.)

## What this is

Mars — a non-modal, Emacs-compatible terminal editor with a Claude-Code-style command
bar, LLM agent integration, and tmux/zellij-style session persistence. Read
[`DESIGN.md`](./DESIGN.md) for the architecture and [`key_design.md`](./key_design.md)
for the UX/interaction philosophy and product vision before making non-trivial
changes — both are living documents, not historical records, and should be updated
alongside code changes that affect what they describe. [`architecture_overview.md`](./architecture_overview.md)
is a file-by-file tour of the code.

**Root `.md` files describe the system as it exists; forward-looking proposals and
product visions live in [`design_ideas/`](./design_ideas/)** (see its README). A doc under
`design_ideas/` may be unbuilt or partially built — don't read it as a description of the
shipped system, and when one ships, fold its durable rationale into the root docs.

## Build, run, verify

```bash
source ~/.cargo/env && cargo build          # cargo is not on the default PATH
./target/debug/mars --selfcheck             # the primary test suite — run after every change
./target/debug/mars                         # try it interactively
./target/debug/mars --ask "how do I split the screen?"   # headless agent smoke test
```

`--selfcheck` is a headless run against `ratatui::TestBackend` that drives the real
`App` — no mocks. It spawns real PTYs and a real session daemon over a real Unix
socket. **Run it after every change and keep it passing; extend it for new behavior
rather than adding a separate test harness.**

Other CLI entry points (see `mars help` for the user-facing reference): `mars new
<name>` create-or-attach a session, `mars attach [name]` reattach, `mars ls` list
with attached/detached state, `mars kill <name>` end a session, `mars ask "<q>"`
one-shot agent query. Long-flag forms (`--session`, `--resume`, `--list`) are
aliases. `--server <name>` is internal (the daemon entry — don't call directly).
Unknown `-`/`--` arguments exit 2 with help; bare arguments are filenames.

## What headless testing cannot verify

Real terminal byte encodings (e.g. `M-<` arrives as ALT|SHIFT, `C-/` arrives as `C-_`
on many terminals, kitty-protocol negotiation) and the session daemon's `setsid`/
process-detachment behavior. Changes to `config.rs` chord parsing or `session.rs`
process spawning need a manual real-terminal pass — see `DESIGN.md` §9 for how to do
this (e.g. `script -q /dev/null mars --session <name>` + `ps` inspection for daemon
tests, since there's often no real TTY in an agent's shell).

## A durable testing gotcha

ratatui's incremental cell-diffing interleaves cursor-repositioning escape codes
*between* individual changed characters when redraws happen one keystroke at a time.
Typed text will **not** appear as a contiguous substring in a raw accumulated ANSI
byte stream. If a test needs to assert "this text is visible," parse the byte stream
through a real ANSI interpreter (`vt100`, already a dependency) and check the
*parsed screen contents* — never `bytes.contains(needle)` on raw output.

## Code conventions observed in this repo

- Default to **no comments**; when one is warranted, it explains a non-obvious *why*
  (a terminal-encoding quirk, a recorded design ruling), never *what* the code does.
- Every user-facing keybinding hint is derived live from `KeyBindings::binding_for()`
  at render time — never hardcode a keybinding string in a menu label, status hint, or
  panel. A remap must update every surface at once (the "honesty invariant" — see
  `DESIGN.md` §2).
- Behavioral magic numbers belong in `tuning.rs` as a named, described knob — not as
  a literal in the call site. The description field is read by humans *and* meant to
  be safely editable by an agent asked to change editor behavior.
- Destructive actions (quit, close, kill) go through a confirmation gate before
  firing — this applies equally to direct user input and agent-proposed `RUN:`
  directives (`Action::is_destructive`).
- New `Action` variants get: a menu entry with a verb-first description
  (`palette.rs`), a `label()` arm, an `is_destructive` check if applicable, and a
  `run_action` dispatch arm — that's the whole surface; keybindings are optional and
  live in `config.rs` defaults.

## Persistent memory

`.claude/memory/` holds accumulated, non-obvious operational facts (build quirks,
terminal-encoding gotchas, past debugging discoveries) — read `INDEX.md` first, then
load the topic file(s) relevant to the task. This is separate from `DESIGN.md`/
`key_design.md`: memory is *discovered facts*, the design docs are *durable
architecture and rationale*. Update memory when you learn something non-obvious that
will save time next session; update the design docs when the architecture, vision, or
a recorded tradeoff actually changes.

## Git

This is a git repository; `main` is the only branch and the PR target. Commit
messages: imperative mood, one-line summary, body explains *why*. Commit or push
only when asked.
