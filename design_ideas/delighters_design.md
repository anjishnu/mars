# Mars — Design: Navigation & Polish Delighters

*Product/design spec for the pre-launch delighters — the lightweight, high-daily-use
features that make Mars feel finished. Written to be reviewed before implementation.
Every choice states its trade-off and the **decision made**; this is not a menu of
options, it's a plan to approve or amend.*

## What's in scope, and why these

The brainstorm surfaced ~8 ideas. This doc commits to the ones that are **high daily
use, low risk, and share a substrate** — plus records what's deferred and why. Two
user asks anchor it: a **fuzzy file finder with live autocomplete**, and a **shortcut
to move through the codebase to switch files fast**.

| Tier | Feature | Ships on |
|---|---|---|
| **1** | Fuzzy file finder (`C-x C-f` upgrade) | Picker + Project index |
| **1** | Quick-open / project switcher (`C-x p`) | Picker + index + file frecency |
| **1** | Buffer switcher (`C-x b` upgrade) | Picker |
| **2** | Git change gutter | Git reader |
| **2** | Autosave "saved ✓" pulse | (trivial) |
| **—** | Deferred: command-bar starter set, smart terminal paste, dashboard splash | — see §5 |

The Tier-1 cluster is the priority: it's the file-navigation experience, three features
sharing two new substrates.

---

## Part I — The two substrates

### Substrate A — the fuzzy Picker

A reusable overlay: given a list of candidates and a live query, rank with the existing
`palette::fuzzy_score`, render a dropdown, and support `↑/↓` select, `Tab` complete,
`Enter` choose, `Esc` cancel. The finder, quick-open, and buffer switcher are all thin
callers.

- **Decision — one generic Picker, not three bespoke prompts.** Today `FindFile` /
  `SwitchBuffer` are plain text `Prompt`s (type a full string, no candidates). Generalize
  the minibuffer `Prompt` with an optional ranked candidate list + selected index,
  rendered as an upward dropdown reusing the `render_bar_dropdown` pattern. Stays in
  `Mode::Prompt`; no new mode.
  - *Enables:* one component, identical feel across every "pick a thing" flow; reuses
    `fuzzy_score` and the dropdown renderer we already have.
  - *Disables:* per-picker bespoke UX (fine — consistency is the point).
- **Decision — `Tab` completes to the longest common prefix of the filtered candidates;
  NO trie.** The user asked for "trie autocomplete." A literal trie only accelerates
  *prefix* queries, but Mars ranks by *subsequence* fuzzy match (`src/mn` → `src/main.rs`),
  which a trie doesn't help. The "autocomplete" feel — Tab fills in the shared prefix —
  is a longest-common-prefix over the current filtered set, computed in O(n) per keypress.
  - *Trade-off:* O(n) per keystroke vs. a trie's O(query). For a cached list of ≤20k
    paths this is sub-millisecond; a trie is premature complexity that also can't do the
    fuzzy matching users actually want. **Decision: flat cached `Vec` + fuzzy rank +
    LCP-on-filtered.** Revisit a prefix index only past ~100k files.

### Substrate B — the Project index

A bounded list of the project's files, built lazily, feeding the finder/quick-open.

- **Decision — lazy build, session-cached, manual refresh; NOT a live file-watcher.**
  Built on the first finder open, cached for the session, rebuilt by a `refresh` key
  (and on an explicit miss). A live watcher (the `notify` crate) is heavier and a new
  dependency for marginal benefit.
  - *Trade-off:* a file created mid-session won't appear until refresh. *Mitigation:* the
    finder still lets you type a literal new path (see Feature 1), and refresh is one key.
    Accept the staleness.
- **Decision — root = git root if present, else cwd (`startup_cwd`).** Walk up for
  `.git`; that's the natural project boundary.
- **Decision — v1 uses a skip-list, not full `.gitignore` parsing.** Walk the tree
  skipping a tuning-knob ignore-list (`target`, `node_modules`, `.git`, `dist`, `build`,
  `.venv`, …) and dotdirs, capped at `project_index_max` files (knob, default 20k).
  - *Trade-off:* won't honor a repo's custom `.gitignore`, so a few build artifacts may
    show. Full fidelity means adding the `ignore` crate (ripgrep's walker). **Decision:
    skip-list for v1 (zero new deps, covers ~90%); document `ignore` as the upgrade** if
    users hit noise. Stated so review is complete.
  - *Enables:* fast fuzzy open; and — per the vision doc's H1 — the *same* index the agent
    will read for project awareness ("can both a pane and the agent read it?"). Building
    it now is a down payment on that primitive.

---

## Part II — Tier 1: the file-navigation cluster

### Feature 1 — Fuzzy file finder (`C-x C-f`)

Replace the plain find-file text prompt with the Picker over the Project index.

- Type → live fuzzy filter; `Tab` → LCP complete; `↑/↓` → pick; `Enter` → open selected.
- **Decision — preserve "open a path that doesn't exist yet" (Emacs find-file).** If a
  candidate is highlighted, `Enter` opens it. If the query is a literal path with no match
  (a new file), `Enter` creates/opens it. Resolves the picker-vs-path ambiguity by
  precedence: *selection wins; else literal path.*
  - *Enables:* fast open of existing files AND new-file creation in one prompt.
  - *Disables:* a sliver of ambiguity, resolved by the precedence rule above.
- *Rides:* Substrates A + B; the existing `open_file`.

### Feature 2 — Quick-open / project switcher (`C-x p`)

The same Picker + index, but ranked by **file frecency** — the files you touch most/most
recently float to the top, and an empty query shows your recent files.

- **Decision — bind `C-x p`** (the project prefix reserved in the keymap zoning law), plus
  a bar row + menu entry. *Not* `Ctrl+P`: bare `C-p` is cursor-up (Emacs) in Edit mode and
  can't be stolen.
- **Decision — persist file frecency in `state.json`.** We already persist an action
  `frecency: HashMap<String,u32>`; add a parallel `file_frecency` map, incremented on open,
  same load/save path (`PersistedState`).
  - *Enables:* "jump back to the file I was just in" as a two-keystroke reflex.
  - *Disables:* a small schema addition to persisted state (backward-compatible via
    `#[serde(default)]`).
- *Rides:* Substrates A + B + the frecency infra.

### Feature 3 — Buffer switcher (`C-x b`)

The existing `C-x b` becomes the Picker over *open buffers*, frecency-ranked. Near-free
once the Picker exists.

- **Decision — buffers, not files, ranked by the same file-frecency.** Distinguishes
  "switch among what's already open" (`C-x b`) from "open something from the project"
  (`C-x p`) — the Emacs mental model, kept.

---

## Part III — Tier 2 polish

### Feature 4 — Git change gutter

A 1-char change marker beside the pointer, so you see at a glance what you've edited.

- **Decision — shell out to `git`, no libgit2.** On buffer open and on save, run
  `git diff --no-color -U0 -- <file>` (working tree vs index), parse the `@@` hunk headers
  into added/modified/deleted line ranges, cache per-buffer.
  - *Trade-off:* a process spawn per open/save vs. a `git2` dependency. Git is always
    present for git users; spawning is cheap and can be async (a thread → an event, like
    the agent). **Decision: spawn `git`, async, cache the result.** Zero new deps.
- **Decision — the marker shares the existing 2-col gutter.** Today the gutter is
  `▸ ` (pointer + space). Make it `[pointer][git]`: col 0 = `▸` on the cursor line, col 1 =
  the git marker (`│` green added / `│` amber modified / `▁` red delete-hint). No width
  change; the pointer and git status coexist.
- **Decision — on by default in a git repo, knob `git_gutter`.** Silently off outside a
  repo.
  - *Enables:* ambient change awareness with zero interaction.
  - *Disables:* markers are stale between saves (we diff on save). Acceptable — live diff
    on every keystroke is wasteful; save-time is the natural refresh.

### Feature 5 — Autosave "saved ✓" pulse

We autosave silently; users don't trust silent saves. On each autosave that writes a
dirty buffer, set a transient status (`✓ saved main.rs`) that auto-clears in ~1s.

- **Decision — reuse `status_msg` with a tick-based expiry.** Trivial; builds trust in the
  crash-safety feature we already ship.

---

## Part IV — Shared UX decisions (apply across the cluster)

- **The Picker lives in the bottom bar + upward dropdown**, exactly like the command bar —
  one spatial model for every list-and-filter surface. *(Not* a centered modal; that's a
  second visual language.)
- **`Esc`/`C-g` cancels, `Enter` chooses, `Tab` completes, `↑/↓` + `C-n/C-p` move** —
  identical to the command bar, so the muscle memory transfers.
- **Honesty invariant holds:** the finder/quick-open/buffer menu rows show their real
  binding via `binding_for`, like every other surface.

---

## Part V — Deferred (recorded so review is complete)

- **Command-bar starter set (recent/suggested actions on empty query).** *Deferred* — it
  conflicts with the shipped fixed-menu-order ruling (spatial stability). The file-frecency
  work here is the right place to revisit whether a "recent" section earns its keep.
- **Smart terminal paste ("run as script?").** *Deferred* — niche; bracketed paste already
  covers the safety case.
- **Dashboard splash (recent files / sessions on the empty screen).** *Deferred* — now that
  a bare `mars` opens a *terminal* by default, the empty-scratch splash rarely shows; low
  ROI until we have a reason to land on an editor first.

---

## Part VI — Sequencing

- **Phase A (the cluster) — ~days.** Substrate A (Picker) + Substrate B (index) + Feature
  1 (finder) + Feature 2 (quick-open, incl. file-frecency persistence) + Feature 3 (buffer
  switcher). Ships the whole file-navigation experience together.
- **Phase B — ~1–2 days.** Feature 4 (git gutter) + Feature 5 (autosave pulse). Independent
  of A.

## Files touched (for the implementation round)
`src/app.rs` (Picker prompt state, project index, file frecency, git-diff spawn/cache,
autosave pulse), `src/palette.rs` (reuse `fuzzy_score`; maybe a shared LCP helper),
`src/ui.rs` (picker dropdown = generalize `render_bar_dropdown`; git marker in the gutter),
`src/config.rs` (`C-x p` + finder/quick-open bindings), `src/tuning.rs`
(`project_index_max`, ignore-list, `git_gutter`), `src/main.rs` (selfchecks). Possibly
`src/git.rs` (new, the diff reader). No new crates in v1 (`ignore` noted as a later
upgrade).

## Verification (for the implementation round)
Selfchecks: index build skips the ignore-list and caps at the knob; fuzzy finder filters +
LCP-completes a known path; quick-open orders by seeded file-frecency; buffer switcher picks
an open buffer; git gutter marks a line changed after an edit+save in a temp git repo;
autosave pulse appears then clears. Real-terminal pass: `C-x C-f` a deep path by fuzzy
fragments, `C-x p` to jump back to a recent file, edit a tracked file and watch the gutter
mark it.
