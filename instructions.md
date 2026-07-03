# Mars — Try It Out (features landed this session)

Build & launch:

```bash
source ~/.cargo/env && cargo build
./target/debug/mars            # new session, opens a terminal + the MARS banner
=```

> **Terminal note for fast movement:** fast motion is bound to **both `⌘` and `⌥` (Option)**.
> `⌘` reaches Mars only on **kitty-protocol terminals** (Ghostty, Kitty, WezTerm, recent
> iTerm2); on **Terminal.app / Warp** the OS eats ⌘ — use **`⌥` (Option)** there, which is
> the universal binding (enable "Use Option as Meta" if your terminal has it). The `M-…`
> chords and `PageUp/Down` also work everywhere.

---

## 1. The MARS logo is back
Just launch `./target/debug/mars`. The planet-art **MARS** banner now overlays the startup
screen (even though it opens into a terminal). **Press any key** to dismiss it.

## 2. Fast cursor movement (editor)
Open a code file: `./target/debug/mars src/app.rs`

| Do this | Keys (⌘ on kitty terminals, ⌥ everywhere) | Also |
|---|---|---|
| Jump by **code token** (`foo·.·bar·(·baz·)`) | `⌘←`/`⌘→` or `⌥←`/`⌥→` | `M-b` / `M-f` (word) |
| **Page** up / down | `⌘↑`/`⌘↓` or `⌥↑`/`⌥↓` | `PageUp` / `PageDown` |
| Extend selection while jumping | add `Shift` (`⌥⇧→`) | `Shift`+`PageUp/Down` |
| Jump to next/prev **blank-line block** | `C-x ]` / `C-x [` | (same) |
| Jump to next/prev **definition** (`fn`/`def`/`class`…) | `C-x }` / `C-x {` | (same) |
| Jump to the **matching bracket** `()[]{}` | `C-x m` | (same) |

`C-x` means Ctrl+x, release, then the next key. These jump keys work on **every** terminal.

## 3. Search that doubles as teleport
In a file, press **`C-s`** and start typing a word you can see:

- It **jumps to the match as you type**, and the prompt shows a **`3/12` counter**.
- Press **`Tab`** → every visible match gets a **1-letter label** (a, s, d, f…). **Press a
  label** to teleport straight to that match.
- **Land-on-any-key:** instead of pressing Enter, just **start editing or press a motion
  key** — the search commits at the current match and your key applies. (Type target → go.)
- `C-s`/`C-r` cycle matches · `Enter` accept · `C-g` cancel (restores where you started).

## 4. One-key terminal composeCtrl+Space
Focus a terminal pane, press Ctrl+Space once. You get one composer:

- Type a **Mars command** (e.g. `split`, `new tab`) → it appears as a suggestion → `Enter`
  runs it.
- Type something that's **not** a command (e.g. `find big files here`) → `Enter` treats it
  as a **shell command**: with an agent key set it's **LLM-translated and shown for you to
  confirm** before running; with no key it runs directly.

No need to press `Ctrl+Space` twice anymore. (`!` still forces shell mode, `?` asks the
agent, `@` opens the file tree.)

## 5. Left file tree (`@` / `C-x d`)
Press **`@`** in the command bar (or **`C-x d`**) to open the left sidebar:

- **Folders are bold + colored**, collapsed. `↑↓` move (selected row is a solid band),
  **`→`/`Enter` expands** a folder, **`←` collapses**.
- On a **file**: **`→` previews** it (shows it, stays in the tree — reversible), **`Enter`
  opens** it (focus moves to the editor).
- **`↑ ../`** at the top steps **up a directory**; type any text to **fuzzy-filter** the
  whole project to a shortlist. `Esc` closes (and resets it to the project root next time).

## 6. Agent on Groq + qwen3-32b
```bash
export GROQ_API_KEY=...        # defaults to qwen/qwen3-32b
./target/debug/mars
```
Then `?` in the command bar, `C-t ?` for "why did this fail?" triage, or the terminal
composer's shell-translate (feature 4). Reasoning models' `<think>` blocks are stripped
automatically. (Gemini also works: `export GEMINI_API_KEY=...` → gemini-3.1-flash-lite.)

---

## Quick smoke test (2 minutes)
1. `./target/debug/mars` → see the **banner**, press a key → land in a **terminal in your
   cwd** (`pwd` to confirm).
2. `Ctrl+Space`, type `open file`, Enter → open the file tree; type `app` → jump to
   `src/app.rs`; `Enter`.
3. In the file: `C-s`, type `fn `, press `Tab`, press a label → teleported.
4. `⌥→` (Option, any terminal) or `⌘→` (kitty) a few times — hop by token. `C-x }` — next fn.
5. Back to a terminal pane, `Ctrl+Space`, type `list files by size`, Enter → (with a key) a
   translated command to confirm.

## 7. Selection-aware agent + reversible refactor
Needs an agent key (`GROQ_API_KEY` / `GEMINI_API_KEY`). In a code file:

1. **Select** a chunk of code (`Shift`+arrows, or `C-x h` select-all).
2. Open the agent: `?` in the command bar.
3. **Ask about it** — "what does this do?", "any bugs?" — and the exact selection is sent
   as precise context (not just the whole screen).
4. **Refactor it** — "simplify this" / "add error handling" / "make this async". The model
   replies with the rewritten code, and the panel shows **`▶ Enter to replace the selection
   (N lines)`**. Press **Enter** to apply it — as **one undo step**, so **`C-/` reverts the
   entire AI edit** at once. `C-l` cancels instead.

## 8. Watch a pane (W6) — needs an agent key
Kick off a long command in a terminal, then **`C-t`** (travel mode) **`w`** to watch it (or
the "Watch this pane" command). When the command **exits** or its output **goes quiet**
(~20s), Mars summarizes it in **one line at the bottom** — failures first, e.g.
`✗ failed: linker error · build`. **`Esc`** dismisses it. This fires **even while you're
detached** (the daemon keeps watching), so you can `mars attach` later to a waiting verdict.

## 9. Reattach briefing (W7)
Start a session (`mars` or `mars new work`), kick off some work, **detach** (`C-t D`), and
later **`mars attach`**. If anything changed while you were gone — a shell exited, a watched
task finished, files got modified — you're greeted with one **`while away — …`** line
(failures first). Nothing changed → no briefing. Deterministic; no key needed.

## 10. Ask beyond the visible screen (W5 / W4) — needs an agent key
Ask the agent (`?`) a question that needs more than what's on screen:
- **Scrollback archaeology (W5):** with a terminal focused, ask *"when did this first start
  failing?"* — the model can request the pane's **full history** (`NEED: scrollback`) and
  Mars silently re-asks with it, then answers.
- **Cross-tab (W4):** *"does the error in the api tab match this code?"* — the model can pull
  **another tab** (`NEED: tab api`). (Panes in your *current* tab were already always sent.)
You don't do anything special — the model asks for what it needs and Mars supplies it once.

## Not yet wired (coming next)
- **Context Bus registry** (formalize `screen_context` into consented sources) and
  **parameterized actions** (`RUN: FindFile("x")`) — designed in `workflows_eng.md`, deferred.
- **Subword motion** (`⌘⌥←/→` for `get·User·Name`) — planned fast-follow.
