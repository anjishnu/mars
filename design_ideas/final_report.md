# Mars — Final Report: Mobility + AI Workflows

*What was built this cycle, how it was verified, and how to try it. Companion to
`instructions.md` (hands-on tryout) and the design docs (`speed_design.md`,
`workflows_eng.md`).*

---

## Scope delivered

Two bodies of work, both complete and committed:

1. **Mobility / speed** — make the editor and terminal laser-fast to move around and act in.
2. **AI-enabled workflows** — the daemon-resident "ops-mate" tier (watch, briefing,
   scrollback archaeology, cross-tab), on top of the engineering design in `workflows_eng.md`.

**Verification:** `./target/debug/mars --selfcheck` — **54 checks, all pass**. The suite is
now **hermetic** (clears inherited agent keys so it's deterministic). Search-teleport and the
startup banner were additionally verified live in a real PTY (pyte). ⌘-chords and the LLM
paths were verified by design + unit-level simulation; they need a kitty-class terminal / an
agent key for full live use (called out below).

---

## Part 1 — Mobility (commit `764d517`)

| Feature | How to use | Verified |
|---|---|---|
| **Code-token motion** | `⌘←/→` (token), `⌘↑/↓` (page), `⌘⇧+arrow` (select) | selfcheck: token stops across `foo.bar(baz)` |
| **Structural jumps** | `C-x [ ]` block · `C-x { }` definition · `C-x m` matching bracket | selfcheck: symbol jump, bracket match |
| **Search = teleport** | `C-s` → type; `n/m` counter; `Tab` labels matches, press one to jump; any motion key commits | selfcheck + **live PTY** (counter + a/s/d/f labels rendered) |
| **Unified terminal composer** | `Ctrl+Space` once in a terminal: command suggestions, else LLM-translated shell (confirm-gated) | selfcheck: no-command query → shell |
| **Selection-aware refactor** | Select code, `?`, "simplify this" → `▶ Enter to replace` → one `C-/` reverts | selfcheck: code-block extract + reversible apply |

**Caveat:** `⌘` reaches Mars only on **kitty-protocol terminals** (Ghostty/Kitty/WezTerm/
recent iTerm2). On Terminal.app/Warp the fallbacks are `M-f/M-b` (word) and `PageUp/Down`.

**Also fixed:** the startup **MARS banner** stopped showing once sessions-by-default opened a
terminal (the splash was editor-pane-only) — now a top-level overlay; verified live.

---

## Part 2 — AI workflows

Engineering design first (`workflows_eng.md`), then each workflow built + tested. Build order
followed `strategy.md`'s recommendation: lead with the daemon-resident vigilance pair (the
moat no competitor can copy without a daemon).

### W6 — Watch this pane (commit `3183471`)
`C-t w` on a terminal → Mars summarizes it in one line when it **exits** or goes **quiet**
(~20s), failures first, `Esc` to dismiss. Runs inside the daemon's `tick`, so **it fires
while you're detached.**
- *Engineering:* `WatchState` per `TermId` fed by the `term_rx` drain; `maybe_fire_watches`
  summarizes via a background `agent::watch_summary` under one global `bg_busy` gate
  (foreground asks preempt); a **pull-model `notices` queue** — the agent's only path to the
  screen, so the interruption budget is structural, not policy.
- *Verified:* selfcheck drives watch → injects a verdict event → asserts a Failure notice
  renders and `Esc` dismisses it.

### W7 — Reattach briefing (commit `483e8c3`)
On detach the daemon snapshots cheap counts+flags; on `mars attach` it diffs and, if anything
changed while away (exited shells, dirty files, watch verdicts), greets you with one
`while away — …` line. Absent when nothing changed. **Deterministic — no key needed.**
- *Engineering:* `Snapshot` + `on_detach`/`on_attach`, hooked into `session.rs:server_main`'s
  `ClientGone`/`Attach` arms. Pairs with W6: a task that finished while you were gone is
  waiting on attach.
- *Verified:* selfcheck drives detach → change → attach and asserts the briefing (and its
  absence when idle).

### W5 + W4 — Ask beyond the visible screen (commit `74d1130`)
A read-side `NEED:` directive: the model replies `NEED: scrollback` (the focused terminal's
full history — "when did this first fail?") or `NEED: tab <name>` (another tab), and Mars
**silently re-asks once** with the extra source, then answers. Everyday asks stay cheap;
history/cross-tab cost one round-trip *only when the model asks for it*.
- *Engineering:* `AgentDirective::Need(NeedKind)` parsed by the shipped directive machinery;
  `tick` auto-satisfies it (`reask_with_need`, depth-capped at 1 — never a loop);
  `Term::history_tail` pages the vt100 scrollback and restores the live view. Single-tab
  cross-pane already shipped via `screen_context`, so W4's only new surface is cross-*tab*.
- *Verified:* selfcheck asserts `NEED:` parses and that the first NEED re-asks unsurfaced
  while a depth-capped second one surfaces normally.

**Caveat:** W6/W5/W4's *summaries and answers* need an agent key (`GROQ_API_KEY` etc.). The
*mechanisms* (triggers, notices, briefing diff, NEED routing) are key-free and tested; W7's
briefing is fully deterministic.

---

## What's deferred (designed, not built)

Per `workflows_eng.md`, two primitives have **no W1–W7 consumer** and were deliberately not
built:
- **Context Bus registry** — formalizing `screen_context` into consented `ContextSource`
  trait objects. Today's function works; the registry is the refactor for when git/index/etc.
  become first-class sources.
- **Parameterized actions** (`RUN: FindFile("x")`) — the enum+parser change is cheap but
  gates only Phase-4 multi-step *plans*, which themselves wait on the **transaction journal**
  (reversibility before autonomy). No shipped workflow needs either yet.

Also outstanding as noted in the tryout guide: **subword motion** (`⌘⌥←/→`).

---

## Commit trail

```
74d1130  W5/W4 NEED: context expansion (scrollback + cross-tab)
483e8c3  W7 reattach briefing
3183471  W6 watch-this-pane + AI-workflows engineering design
764d517  Mobility: fast movement, teleport search, unified composer, selection refactor
de8610e  Initial commit
```

## How to try it
See `instructions.md` (§1–§10) for a step-by-step tryout, including the 2-minute smoke test.
Fastest end-to-end proof of the moat: in a session, run a failing command in a watched pane
(`C-t w`), detach, reattach — the verdict is waiting.
