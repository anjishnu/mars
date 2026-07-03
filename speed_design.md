# Mars — Design Proposal: Laser-Fast Movement & the Anchored Query

*A proposal (not yet built) for making the editor and terminal extremely fast to move
around and act in. Written with the UX/cognitive-science/ergonomics hat on. Every
section states a recommendation and surfaces the real tradeoffs to decide before we
implement.*

---

## The cost model (why these choices)

Navigation cost = **keystrokes × motor-cost + visual-reacquire cost**. Five levers,
each grounded:

1. **Bigger jumps (Fitts's law).** Time ∝ distance ÷ target-size. Char-by-char is the
   slowest possible motion. Every larger unit — token, line, paragraph, block, symbol,
   teleport — collapses many keystrokes into one.
2. **Chunk-aligned units (Miller/chunking).** Move by units that match how you *mentally*
   parse code: token, subword (camelHump), argument, block, function.
3. **Modifier-scaling (motor consistency).** One motor pattern with escalating reach:
   `arrow → +mod = token → +mod = line-edge`. Learn once, applies everywhere.
4. **Recognition-teleport (Nielsen: recognition > recall).** Label on-screen targets,
   type a label, teleport. O(1) for *any visible* target, no counting. The single biggest
   long-jump win (avy / vim-easymotion / VS Code "flash").
5. **Reversible & previewed.** Fast motion must be non-destructive and show where you'll
   land (and let you snap back).

---

## Part A — Editor: fast movement

### The blocker (decide this first)
Today: `←/→` = char, `↑/↓` = line, **`Ctrl+arrow` and `Alt+arrow` = pane navigation**,
word-jump only on Emacs `M-f`/`M-b`. So the *intuitive* "hold a key + arrow to skip by
token" has nowhere to live — the obvious modifier is taken by panes.

**Recommendation:** reclaim `Alt/Option+arrow` for **token movement** and leave panes on
`C-o` (cycle) + `C-t` (travel hub), which already fully cover pane nav. `Option+arrow` is
*also* the macOS-native "move by word," so this is zero-learning for Mac users.

### A1. Modifier-scaled motion (the intuitive core)
| Keys | Motion | Rationale |
|---|---|---|
| `←/→` · `↑/↓` | char · line | unchanged |
| **`Option+←/→`** | **token** (next/prev) | Mac-native word-jump; the headline "skip tokens" gesture |
| **`Cmd+←/→`** | line start/end | Mac-native |
| **`Cmd+↑/↓`** | buffer top/bottom | Mac-native |
| add `Shift` to any | extend selection | one rule, universal |
Emacs bindings (`M-f/M-b`, `C-a/C-e`, `M-</M->`) stay — three dialects at once, as ever.

### A2. Three granularities of "token" (code-aware)
"skip to the next token instead of next character" deserves precision:
- **Word** — whitespace/punctuation delimited (today's `move_word`).
- **Token** — code-aware: identifiers, operators, brackets, string literals as atoms
  (`foo.bar(baz)` → `foo · . · bar · ( · baz · )`). Smarter than words in code.
- **Subword (CamelHumps)** — stop at `camelCase`/`snake_case` seams (`getUserName` →
  `get·User·Name`). Essential for editing *part* of an identifier.

**Recommendation:** `Option+arrow` = token; `Option+Shift+arrow` = token + select;
`Ctrl+Option+arrow` = subword (once panes vacate that modifier). Decide how many
granularities are worth the surface — I'd ship **token + subword** and treat plain "word"
as the Emacs-dialect alias.

### A3. Vertical speed
- **PageUp/PageDown** = screen (exists). Add **half-page** (`C-d`/`C-u`): research and
  practice both show half-page scroll preserves orientation far better than full-page.
- **Block jump** — `Option+↑/↓` (or `M-{`/`M-}`) to prev/next blank line: fly between code
  blocks.
- **Symbol jump** — next/prev top-level definition (heuristic: column-0 `fn`/`def`/`class`/
  `pub`/`impl`, no parser needed v1). The biggest single win for reading real code.
- **Matching bracket** — one key to hop to the matching `)/]/}`; instantly traverse nested
  structure.

### A4. Teleport — "jump to what you see" (the killer feature)
Press a trigger → every word on screen gets a 1–2 char label → type the label → the cursor
teleports there. Recognition-based, O(1) to *any visible* target, no counting or repeated
taps. Plus a "jump to character" variant (type a char → occurrences get labels). This is
the highest-leverage movement feature in the proposal. Needs one free, easy chord.

### A5. Return & counted motion
- **Jump-back ring** — after any big jump (teleport, search, goto-line), one key snaps back
  to where you were (vim `''` / VS Code `Ctrl+-`). Removes the fear of jumping.
- **Counted motion** — a numeric prefix (`C-u N` Emacs-style) for "10↓", for the rare exact
  count. Low priority next to teleport.

---

## Part B — Editor: the Anchored Query (`Ctrl+Space` in the editor)

Mirror the terminal's inline composer *for code*. `Ctrl+Space` opens a query box anchored
at the cursor/selection; the **selection becomes precise, line-ranged context** — tighter
and cheaper than whole-screen context. This is the editor twin of the shell composer and
the natural home for selection-scoped agent actions (the editor slice of the "context
bus").

**What the anchored query can do** (the brainstorm — "what else"):
- **Explain** — "what does this do?" scoped to the selection (sharper than `C-x e`).
- **Generate at cursor** (no selection) — "a function that parses X" → inserts.
- **Tests / docs** — "write a test for this," "add a docstring."
- **Transform** — "make async," "convert to iterator," "add error handling," "type-annotate."
- **Refactor / rewrite** — "extract a function," "simplify" → proposes a replacement.
- **Fix** — on an error region, "fix this" (ties into failure triage).
- **Review / ask** — "edge cases?", "is this thread-safe?", "why is this slow?"
- **Scoped rename / find** — "rename this var within this function."
- **Port / translate** — "port to Python."

**How output routes:** an *answer* → the Ask panel; an *edit* → a confirm-gated change.
Reversibility gates ambition (see strategy.md): ship **ask / explain / generate-insert**
now (read-only + insert-at-cursor, trivially undoable); put **replace / refactor** behind a
single undo checkpoint as phase 2, and full multi-file refactor behind the transaction
journal (phase 3). Decisions to confirm: selection-vs-cursor context; insert-vs-replace
semantics for edits.

---

## Part C — Terminal: laser-fast

### C1. Parallel `Ctrl+Space` (your routing idea)
Today: `Ctrl+Space` in a terminal → inline shell composer; `Ctrl+Space` *again* → command
bar. Two presses to reach commands.

**Proposal — one composer that disambiguates by content.** As you type, matching Mars
commands appear (fuzzy, like the command bar). `Enter` routes:
- a **selected/close command match** → run the Mars action;
- **no match** (or you keep typing past the suggestions) → the text is a shell command
  (literal, or English→shell via the existing translator) → runs in the pane.

So command-bar and shell composer *merge*: commands surface as suggestions above, and shell
execution is always the fallback. The double-press disappears.
**Tradeoff to decide:** ambiguity — a shell string that happens to fuzzy-match an action.
Mitigation: only treat it as a command when a suggestion is *explicitly selected* (or an
exact prefix like `>`); default to shell when uncertain; destructive actions keep the
confirm gate. This "shell-first, commands-as-suggestions" default is the safe reading.

### C2. Other terminal speedups (ranked)
- **Jump between prompts** — hop to prev/next shell prompt in scrollback (iTerm2 `Cmd+↑`).
  Navigate by *command*, not by line. High value.
- **Copy last command / last output** — one key each; the constant "grab that" moment.
- **Select last output → anchored query** — "why did this fail?", "explain this output"
  (unifies with Part B; `C-t ?` triage already proves the appetite).
- **Fuzzy history search** — `Ctrl+R`-style over the pane's history; rerun fast.
- **Terminal quick-actions** — clear, split, new-terminal-in-same-cwd, rerun-last — as
  command suggestions in the C1 composer.
- **English→shell** (exists) — keep as the composer's natural-language fallback.

### C3. The unifying idea
Editor and terminal get the **same gesture** — `Ctrl+Space` = "do something here, now" —
with context-appropriate behavior: in the editor, query the selection; in the terminal,
run a command or ask about output. One thing to learn, everywhere. That consistency is
itself the ergonomic win.

---

## DECISIONS MADE (2026-07 — locked with the user)

1. **Movement modifier = `Cmd`/⌘ (not Option), so pane-nav is untouched.**
   - `Cmd+←/→` = **code-token** (identifiers/operators/brackets/strings as atoms).
   - `Cmd+↑/↓` = **page up/down**.
   - `Cmd+Shift+arrow` = extend selection by token/page.
   - **Subword (CamelHumps)** deferred to a fast-follow on `Cmd+Option+←/→`.
   - Plus block-jump (blank line), symbol-jump (column-0 `fn`/`def`/`class` heuristic),
     matching-bracket hop.
   - **Caveat:** ⌘ chords only reach Mars on kitty-protocol terminals (Ghostty/Kitty/
     WezTerm/iTerm2). On Terminal.app/Warp the terminal eats ⌘ → fall back to the existing
     `M-f`/`M-b` (word) + `PageUp`/`PageDown` (page). Revisit the modifier if the user is
     primarily on a non-kitty terminal.

2. **No separate teleport — fold its trick into incremental search instead.**
   Search already highlights ALL matches (search_hl). Make it the fast-jump primitive:
   - **Match counter** (`3/12`) in the prompt.
   - **Land-on-any-key**: any motion/edit key accepts at the current match and is then
     applied (Emacs-isearch sticky exit) — no explicit Enter to commit.
   - **Jump-labels for visible matches**: after a couple chars, overlay 1-char labels on
     on-screen matches; press a label to jump there (teleport's O(1) pick, via search).

3. **Anchored editor query — DEFERRED as a composer.** Exception: the existing agent
   query becomes **selection-aware** — a live selection is added to the LLM context
   (precise, not whole-screen) and the agent may propose a **refactor of the selection**,
   reversibly (single undo checkpoint).

4. **Terminal `Ctrl+Space` — MERGE into one composer.** Mars-command suggestions on top;
   if no command is picked, the text is **LLM-translated to a shell command and shown for
   confirmation** before running (never blind literal execution). No double-press.

## Build order
1. Editor motion: `Cmd+arrow` token/page + `Cmd+Shift` select + block/symbol/bracket jumps.
2. Search-as-teleport: match counter + land-on-any-key + visible-match jump-labels.
3. Terminal unified composer (suggestions + LLM-translate + confirm).
4. Selection-aware agent query + reversible selection refactor.
