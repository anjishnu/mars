# Mars — Try It Out (features landed this session)

Build & launch:

```bash
source ~/.cargo/env && cargo build
./target/debug/mars            # new session, opens a terminal + the MARS banner
```

> **Terminal note for fast movement:** the `⌘`/Cmd shortcuts below only reach Mars on
> **kitty-protocol terminals** (Ghostty, Kitty, WezTerm, recent iTerm2). On **Terminal.app
> or Warp** the OS eats ⌘, so use the **fallbacks** listed with each one. Everything else
> works everywhere.

---

## 1. The MARS logo is back
Just launch `./target/debug/mars`. The planet-art **MARS** banner now overlays the startup
screen (even though it opens into a terminal). **Press any key** to dismiss it.

## 2. Fast cursor movement (editor)
Open a code file: `./target/debug/mars src/app.rs`

| Do this | Keys | Fallback (any terminal) |
|---|---|---|
| Jump by **code token** (`foo·.·bar·(·baz·)`) | `⌘←` / `⌘→` | `M-b` / `M-f` (word) |
| **Page** up / down | `⌘↑` / `⌘↓` | `PageUp` / `PageDown` |
| Extend selection while jumping | add `Shift` (`⌘⇧→`) | `Shift`+`PageUp/Down` |
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

## 4. One-key terminal composer (no more double-press)
Focus a terminal pane, press **`Ctrl+Space`** once. You get **one** composer:

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
4. `⌘→` a few times (kitty terminal) or `M-f` — hop by token. `C-x }` — jump to the next fn.
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

## Not yet wired (coming next)
- **Subword motion** (`⌘⌥←/→` for `get·User·Name`) — planned fast-follow.
