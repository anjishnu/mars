# MARS

*Mission control for your terminal* — a non-modal, Emacs-compatible terminal editor
with a Claude-Code-style mission-control command bar, a built-in LLM agent, real terminal panes, and
tmux/zellij-style persistent sessions. One tool, one set of keys.

```
███╗   ███╗ █████╗ ██████╗ ███████╗
████╗ ████║██╔══██╗██╔══██╗██╔════╝
██╔████╔██║███████║██████╔╝███████╗
██║╚██╔╝██║██╔══██║██╔══██╗╚════██║
██║ ╚═╝ ██║██║  ██║██║  ██║███████║
╚═╝     ╚═╝╚═╝  ╚═╝╚═╝  ╚═╝╚══════╝
```

## Build & install

```bash
source ~/.cargo/env            # if cargo isn't on your PATH
cargo build --release
# put it on your PATH:
ln -s "$PWD/target/release/mars" ~/.local/bin/mars   # or copy it anywhere
mars --selfcheck               # optional: run the built-in test suite
```

## Quick start

```bash
mars                    # start a session: the MARS banner, then a shell in your cwd
mars notes.md           # edit a file (also inside a session)
mars -s notes.md        # standalone mode: no session daemon, just edit
mars help               # full CLI reference
```

Sessions are the default, tmux-style: a bare `mars` gets an auto-numbered session
that survives closing the window. Press any key to dismiss the startup banner.

Inside the editor, four keys carry you everywhere:

| Key | What it does |
|---|---|
| `Ctrl+Space` | **mission control** — search every command (type to filter, Enter to run); works in terminal panes too |
| `!` (in mission control) | run a shell command in a terminal pane |
| `?` (in mission control) | ask the built-in agent anything ("how do I split the screen?") |
| `C-x C-f` | **Navigator** — browse & jump to any project file (type to fuzzy-filter) |
| `C-t` | space warp: tabs, panes, splits — with an on-screen cheat panel |

`C-g` cancels anything. Every menu row shows its real keybinding, so the fast path
teaches itself as you go.

## Moving around fast

Fast motion is bound to **both `⌘` and `⌥` (Option)**. `⌘` reaches Mars only on
kitty-protocol terminals (kitty, WezTerm, Ghostty, recent iTerm2); on Terminal.app /
Warp the OS eats `⌘` — use `⌥` there (enable "Use Option as Meta" if offered). The
`C-x` jumps work on every terminal.

| Do this | Keys | Also |
|---|---|---|
| Jump by code token (`foo·.·bar·(·baz·)`) | `⌘←`/`⌘→` or `⌥←`/`⌥→` | `M-b` / `M-f` (word) |
| Page up / down | `⌘↑`/`⌘↓` or `⌥↑`/`⌥↓` | `PageUp` / `PageDown` |
| Extend selection while jumping | add `Shift` (`⌥⇧→`) | `Shift`+`PageUp/Down` |
| Next/prev blank-line block | `C-x ]` / `C-x [` | |
| Next/prev definition (`fn`/`def`/`class`…) | `C-x }` / `C-x {` | |
| Matching bracket `()[]{}` | `C-x m` | |

**Search doubles as teleport.** `C-s`, type a word you can see — it jumps as you
type, with a `3/12` counter. Press `Tab` and every visible match gets a one-letter
label; press a label to teleport straight there. And you don't have to press Enter:
just start editing or hit a motion key — the search commits at the current match and
your key applies. `C-s`/`C-r` cycle matches, `C-g` cancels (restoring where you
started).

## Sessions — replace tmux/zellij

Sessions keep your buffers, panes, and **running shells** alive when the window
closes, the SSH connection drops, or you just walk away.

```bash
mars new work           # start (or re-attach) a session named "work"
mars ls                 # what's running, and whether anything is attached
mars attach             # reattach the most recent session
mars attach work        # reattach a specific one
mars rename work api    # rename a running session (live — nothing restarts)
mars kill work          # end a session from outside (autosaves first)
```

The daily rhythm:

1. **Start**: `mars` or `mars new work` — everything from here on lives in the daemon.
2. **Detach** when you want the terminal back: press `C-t` then `D` — or just close
   the window. Both leave shells running and buffers intact.
3. **Come back**: `mars attach` (or `mars attach work`). Your layout, buffers, and
   that build you left running in a terminal pane are exactly where you left them.
   If anything happened while you were gone — a shell exited, a watched task
   finished, files changed — one **`while away 3h20m — …`** line greets you, failures
   first, duration up front. Press **`C-x g`** for the full **Away Digest**: a
   sectioned timeline (✗ needs you / ✓ done / context) with each run's duration,
   re-summonable anytime. Nothing changed → no briefing.
4. **Finish for real**: `C-x C-c` inside the session quits it (with an
   are-you-sure prompt if anything is unsaved), or `mars kill work` from outside.

`mars ls` tells you the state at a glance:

```
SESSION              STATUS
work                 detached — reattach: mars attach work
review               attached
```

Safety nets, on by default: modified files autosave every 30s and on every
detach/disconnect (scratch buffers are never touched), and each daemon logs to
`~/.local/state/mars/<name>.log` — if a session ever dies, the postmortem is there.

Notes: one client per session — attaching from a second window takes over from the
first (it gets a clean "another client attached" message). Attaching from a
different-sized terminal just reflows.

## The agent

> **Beta.** The AI features — the `?` ask flow, agent-proposed `RUN:`/`TYPE:`
> directives, refactors, triage, watch summaries, and the away digest — are new and
> still being tuned. Treat the agent as an assistant, not an authority: review what it
> proposes before running it (destructive actions are gated, but read them). The core
> editor, multiplexer, and sessions are stable.

Works out of the box with a free-tier key from any of:

```bash
export GROQ_API_KEY=...        # Groq (free tier) — defaults to qwen/qwen3-32b
export GEMINI_API_KEY=...      # Google AI Studio (free tier) — gemini-3.1-flash-lite
# or any OpenAI-compatible endpoint (e.g. local Ollama):
export MARS_LLM_KEY=... MARS_LLM_URL=http://localhost:11434/v1 MARS_LLM_MODEL=llama3
# override the model for any provider:
export MARS_LLM_MODEL=qwen/qwen3-32b
```

Reasoning models (Qwen3, DeepSeek-R1) work — their `<think>` blocks are stripped from
answers automatically.

Then `?` in mission control, or from the shell:

```bash
mars ask "how do I move a pane to the other side?"
```

### The agent works on every box — your key never leaves home

> **Beta.** The SSH features (`mars ssh`, `mars keyd`, the fleet view, and the
> remote installer) are new and still being hardened — as are the AI features they
> carry (see [The agent](#the-agent)). The core editor, multiplexer, and sessions are
> stable; the remote/tunnel path may have rough edges — please report anything you hit.

You set your key **once**, on your own machine, and the agent works on every host you
SSH into — without the key ever landing on a remote box (not in its env, not in its
shell history, not on its disk).

```bash
mars ssh gpubox           # ssh in — forwards the auth socket AND auto-starts the
                          # key broker if needed (inheriting this shell's API key).
                          # `mars` on gpubox → the agent just works. No key on the box.
```

(The broker — `mars keyd` — starts on demand the first time you `mars ssh`; run it
explicitly only if you want it in a specific shell.)

**Installing mars on a fresh host.** Mars needs a modern Rust toolchain (≥ 1.85) — a
distro-packaged `cargo` (e.g. Ubuntu's 1.75) is too old and fails with a cryptic
`edition2024` error. Don't `apt install cargo`; install rustup (the official way,
from [rust-lang.org/tools/install](https://www.rust-lang.org/tools/install)), then
install mars from crates.io:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust toolchain
. "$HOME/.cargo/env"
cargo install mars-terminal                                       # the `mars` binary
```

(`install.sh` in this repo automates exactly those steps, including detecting a
too-old distro cargo.) Your API key never lands on the box — it's served from home
over the tunnel.

The remote never makes the LLM call itself — it proxies the request home through the
SSH tunnel, and the completion comes back. Compromise the box and there's nothing to
steal; close your laptop and remote access ends with the tunnel. Jump hosts,
`ProxyCommand`, and hardware keys all work (it wraps your real `ssh`). `mars ls` shows
the hosts you've been on, newest first — type a number or a name to hop back.

The agent **sees your screen** — editor buffers, terminal output, your layout — so
"why did this build fail?" needs no copy-paste. It holds a conversation (`C-l`
starts a fresh one), and it can *act*: `RUN:` fires an editor action, `TYPE:` types
a shell command into your terminal pane, `OPEN:` jumps to a `path:line` from a stack
trace — always shown first, always one explicit Enter away, never automatic.

What that unlocks:

- **Ask about a selection.** Select code (`Shift`+arrows or `C-x h`), then `?` —
  the exact selection goes along as context, not just the visible screen.
- **Reversible refactors.** Ask "simplify this" / "add error handling" on a
  selection: the panel shows `▶ Enter to replace the selection (N lines)`. Enter
  applies it as **one undo step**, so a single `C-/` reverts the whole AI edit.
- **Triage.** `C-x ?` (or "why did this fail?" in the bar) grounds the agent in the
  focused terminal's output; `C-x e` explains what's at the cursor.
- **Watch a pane.** Kick off a long command, then `C-t w`. When it exits or goes
  quiet (~20s), Mars leaves a one-line verdict at the bottom, failures first
  (`✗ failed: linker error · build`). `Esc` dismisses. This fires **even while
  detached** — the daemon keeps watching, so `mars attach` later lands on the verdict.
- **Ask beyond the visible screen.** Questions like "when did this first start
  failing?" or "does the error in the api tab match this code?" let the model
  request more context (the pane's full scrollback, or another tab); Mars supplies
  it and re-asks once, silently. You do nothing special.
- **Shell translation.** In a terminal pane, `Ctrl+Space` then plain English
  ("find big files here") → the agent translates it to a shell command and shows it
  for you to confirm. Typed a real Mars command instead? It's recognized and run
  directly. (`!` still forces shell, `?` asks, `@` opens Navigator.)

With an agent connected, tabs you haven't named get a quiet auto-generated label
from their content (rename one yourself and it's yours forever; `auto_name_secs = 0`
turns it off).

## Keys you already know

Mars speaks three dialects at once — whichever your fingers know:

- **Navigator (browse files)**: `C-x C-f` — or `Ctrl+Space` then `@` — opens **Navigator**, the
  file sidebar on the left.
  Folders are bold + colored and collapsed — arrow to one and `Enter`/`→` expands it in
  place (`←` collapses); on a file, `→` previews it (reversible) and `Enter` opens it;
  `../` at the top steps up a directory. Start **typing** to fuzzy-filter the whole
  project to a shortlist; `Esc` closes.
- **Emacs**: `C-x C-s` save · `C-x C-f` open · `C-s` isearch · `M-%` query-replace
  (`y`/`n` step, `!` all) · `C-k`/`C-y` kill/yank · `C-x 2`/`C-x 3`/`C-x o` windows ·
  `M-x` mission control
- **Modern/Mac**: `C-c`/`C-v` copy/paste (system clipboard) · Shift+arrows select ·
  typing replaces selection · `Tab`/`Shift-Tab` indent/dedent a selected block ·
  mouse click/scroll/wheel · `⌘C/⌘V` on kitty-class terminals
- **tmux/zellij**: `C-t` space warp · `M-{`/`M-}` or `C-PgUp/PgDn` switch tabs ·
  `M-1..9` jump to tab · `C-o`/`Ctrl+arrows` move between panes · `C-|`/`C--` splits ·
  scrollback with the wheel or `Shift+PgUp/PgDn`

**Undo, two ways.** `C-x u` (or `C-/`) undoes — a typed run coalesces into one step, and
an applied AI refactor is always exactly one step. `M-/` redoes. For bigger jumps, **`C-u`
opens time-travel**: `←`/`→` scrub back and forward through your edit history, `Home`
rewinds to the very start, `End` returns to now, `Esc` done.

Everything is remappable in `~/.config/mars/keys.json`; behavior knobs (autosave
interval, scrollback depth, colors, timings, watch quiet threshold) live in
`~/.config/mars/tuning.json`, each with a plain-English description of what it does.
Broke your config experimenting? **`mars reset`** restores default keys + tuning (your
old files are kept as `*.bak`).

## Troubleshooting

- **Staircase output** (lines drifting right, like `mars help` printing diagonally):
  your shell's terminal was left in raw mode — usually by a force-killed program.
  Run any `mars` command (it repairs the terminal automatically on startup) or
  `stty sane`.
- **`M-…` keys do nothing (macOS)**: enable "Use Option as Meta" in Terminal/iTerm —
  or use the `Ctrl`-based twins (`C-o`, `Ctrl+arrows`), which always work.
- **`⌘` chords do nothing**: `⌘` only reaches Mars on kitty-protocol terminals
  (kitty, WezTerm, Ghostty, iTerm2 3.5+); elsewhere the OS keeps it. The `⌥` and
  `M-…` twins work everywhere.
- **A session shows `dead (cleaned up)`** in `mars ls`: the daemon crashed or the
  machine rebooted. Check `~/.local/state/mars/<name>.log` for the reason; autosaved
  file changes are already on disk.
- **Fancy chords (`C-{`, `C--`, `⌘C`) don't fire**: they need a kitty-protocol
  terminal. The Alt-based twins work everywhere.

## More

- [`architecture_overview.md`](./architecture_overview.md) — a file-by-file tour of
  the code: what lives where and how the pieces connect.
- [`DESIGN.md`](./DESIGN.md) — architecture rationale, tradeoffs, and how the pieces fit.
- [`key_design.md`](./key_design.md) — the design doctrine and product vision
  (why the keys are what they are, and where Mars is going).
- [`AGENTS.md`](./AGENTS.md) — instructions for AI coding agents working on Mars.
