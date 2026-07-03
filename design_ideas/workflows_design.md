# Mars — Design: The First 7 Agentic Workflows

*Implementation-oriented design for workflows W1–W7 from [`agentic_inline.md`](./agentic_inline.md).
Every choice states what it **enables** and **disables**, so the trade-offs are
reviewable rather than buried. Nothing here is built yet — this is the spec to build
against.*

The seven, and the substrate each forces:

| # | Workflow | New substrate it forces |
|---|---|---|
| W1 | "What am I looking at?" | (none — ships on today's grounding) |
| W2 | "Why did this fail?" | directive: `OPEN:` + a triage entry point |
| W3 | "Do it in English" (`!`→shell) | Tab-translate + cursor-anchored overlay |
| W4 | Cross-pane reasoning | context selectors (which panes, budgeted) |
| W5 | Scrollback archaeology | on-demand full-scrollback selector |
| W6 | "Watch this and tell me" | trigger framework + notices queue |
| W7 | "Where was I?" (reattach brief) | detach/attach snapshot + diff |

Three substrates underlie all seven — **(1) a directive vocabulary, (2) context
selectors, (3) a trigger framework.** Designing those three well is ~80% of the work;
the workflows are thin layers on top.

---

## Part I — The three substrates

### Substrate 1: Directive vocabulary

**Today.** `agent::parse_directive` recognizes a single trailing line, `RUN:` or
`TYPE:`, into `enum AgentDirective { Run(String), Type(String) }`. One directive per
reply. The Ask panel shows a confirm bar; Enter fires it (`handle_bar_ask`).

**Choice 1A — how the model expresses an action. Four options:**
(1) **trailing-line directive** `VERB: arg` (what ships); (2) **native tool-calling**
(API `tools`, structured out-of-band JSON); (3) **inline JSON block** in the text;
(4) **XML/tag markup** in the text.
**Chosen: (1), extend the trailing-line parser**, adding `OPEN: path:line`.

Two hard requirements decide it:

- **Model portability.** Mars runs Gemini / Groq / any OpenAI-compatible endpoint /
  **local Ollama**. Native tool-calling (2) support is uneven across that set and
  absent or unreliable on small local models — adopting it as the baseline forks
  behavior by provider and breaks the "bring any model" promise. This eliminates (2)
  as the baseline. Options (1)(3)(4) are all in-band text and therefore portable.
- **Human-readable confirm gate.** The user reads the directive before pressing Enter
  — that is the entire safety model. `TYPE: git status` is legible; a JSON blob or XML
  tag is not. This picks (1) over (3)/(4). (1) is also the form small models emit most
  reliably. Prior art: Aider's SEARCH/REPLACE blocks and similar — in-band text,
  chosen for the same portability reason.

- *Enables:* works with every model including tiny local ones; the directive is
  safety-checkable text in the gate; `parse_directive` stays a pure, unit-tested
  function; extends what ships (one match arm per new verb).
- *Disables / cons (real, bounded):*
  - **one directive per reply** — no batched "open AND run" (the fix is Phase-4 action
    *plans*, not tool-calling);
  - **stringly-typed args**, no schema validation until Phase-4 parameterized actions;
  - **formatting fragility** — the model must put the directive on its own last line
    with the exact prefix; this is the likeliest failure mode. *Mitigate early:* an
    explicit system-prompt rule plus a lenient parser (strip backticks, scan the last
    few lines rather than only the last);
  - **content / injection ambiguity** — bounded by last-line-only parsing *and* the
    mandatory user confirm (native tool-calling shares this surface, so it is not a
    reason to prefer it).
- *Not a one-way door:* `AgentDirective` is the abstraction boundary. A tool-call
  parse path can be added later for capable providers, mapping into the same enum with
  zero handler changes. Baseline = the universal path; structured tool-calls = a later
  per-provider optimization.

**Choice 1B — one directive per reply vs. many.** **Chosen: one.**
- *Enables:* a single, predictable confirm gate; no partial-plan execution ambiguity;
  the user always knows exactly what Enter does.
- *Disables:* "open the file AND run the test" in one turn — the model takes two turns.
  Revisited when parameterized-action plans land (Phase 4).

**`OPEN: path:line` design.** Parsed like `TYPE`. Handler (`app.rs`): resolve the path
(relative to the focused terminal's cwd if any, else the process cwd), open or focus a
buffer for it, `set_cursor(line-1, 0)`, recenter; if the current pane is a terminal,
open the file in a split so the terminal stays visible.
- *Enables:* W2's jump-to-error as one keystroke; a scoped down payment on
  parameterized actions (it is `FindFile(path)` + `GotoLine(n)` fused).
- *Disables:* opening at a *column* or a *symbol* (line-only); cross-repo paths
  (resolves within reachable cwd). Sufficient for stack-trace triage, the actual use.

### Substrate 2: Context selectors

**Today.** `App::screen_context()` dumps session + tabs + **every pane of the active
tab** (editor visible window; terminal full `screen().contents()`), capped at 6 KB
with head/tail truncation, on *every* ask.

**Choice 2A — always-send-everything vs. selectors.** **Chosen: selectors** — the ask
carries only what the question needs, chosen by cheap heuristics, with an explicit
"more" path.
- *Enables:* room for scrollback (W5) and cross-pane (W4) without blowing the token
  budget; cheaper calls (free-tier discipline); the model is not distracted by
  irrelevant panes.
- *Disables:* the "no thought, just works" simplicity of the dump; introduces a
  wrong-selection failure mode (mitigated by always telling the model what it *can*
  ask for and letting it request expansion).

**Selector design (v1, additive over today's builder):**
- *Default:* active tab, all panes, visible windows only (today's behavior minus full
  terminal scrollback → just the visible screen). Cheapest; covers W1/W2/W4.
- *Scrollback tail:* last N lines (knob `agent_scrollback_context`, default ~200) of
  the focused terminal, appended whenever the ask is terminal-focused. Serves W2
  (errors above the fold) and light W5.
- *Full-scrollback expansion:* the model emits `NEED: scrollback` → Mars re-asks with
  the full retained buffer (up to a hard cap) → one extra turn. Serves deep W5 without
  paying for 10k lines on every question.
- *Cross-tab:* `NEED: tab <name>` pulls another tab's panes. Serves "does the code in
  the api tab match this error."

**Choice 2B — how "does the question need history?" is decided.** **Chosen:
model-driven, not keyword-heuristic** — the default context tells the model what is
available and how to ask (`NEED:`); it pulls more when it decides to.
- *Enables:* no brittle keyword lists; the model's judgment scales to phrasing we did
  not anticipate; cost stays proportional to need.
- *Disables:* single-turn answers for history questions (expansion costs a round-trip);
  determinism (the same question may or may not expand). The latency is the honest
  price of not always sending 10k lines.

### Substrate 3: Trigger framework (the proactive pair, W6/W7)

**Today.** The daemon's PTY reader threads emit `TermEvent::Output(id)` on every chunk
and `TermEvent::Exited(id)` on EOF; `App::tick()` drains them. `auto_name` already
demonstrates the pattern: a background LLM request, one in flight, result delivered via
an `AgentEvent` variant applied in `tick`.

**Choice 3A — event-driven triggers vs. polling.** **Chosen: event-driven**, riding
the existing `TermEvent` stream + `frame_tick` clock.
- *Enables:* near-zero idle cost; "output went quiet for N s" is just
  `frame_tick - last_output_tick > threshold`; works headless/detached because it lives
  in the daemon loop — which tmux structurally cannot match.
- *Disables:* triggers on things not surfaced as events (e.g. a specific string in
  output would need a scan — deferred). v1 triggers: output-quiet, process-exit,
  detach, attach.

**Choice 3B — where proactive output goes.** **Chosen: a notices queue**
(`Vec<Notice>`) rendered as at most one status-line at a time, pull-model (the render
reads the queue; the agent never pushes to the screen).
- *Enables:* structural enforcement of the interruption budget — proactivity *cannot*
  steal focus or stack popups because there is no push path; deferrable to task
  boundaries; dismissible.
- *Disables:* urgent interrupts ("prod is down") — everything waits for the user to
  look. The correct default for a trust-building v1; an opt-in louder tier can come
  later.

**Choice 3C — one global in-flight LLM request vs. concurrent.** **Chosen: one global
in-flight** (generalize today's `auto_name_inflight` into a single `agent_busy` gate
covering ask + auto-name + watch + briefing).
- *Enables:* free-tier survival (rate limits); predictable cost; no thundering herd
  when three panes finish at once.
- *Disables:* parallel agent work; a watch summary can queue behind a user ask.
  Acceptable — user-initiated asks take priority; background work waits.

---

## Part II — The seven workflows

### W1 — "What am I looking at?"  *(ships on existing grounding)*
- *Moment:* cursor on a confusing error / log / config / function; one gesture → plain
  explanation.
- *Design:* a bound action `ExplainThis` opens the Ask bar pre-filled with "Explain
  what's on screen: <cursor context>" and submits immediately; the answer lands in the
  transcript. Reuses `submit_agent_query` + `screen_context`.
- *Choice — auto-submit vs. pre-fill-and-wait.* **Auto-submit** (a zero-typing
  gesture). *Enables* one-key use; *disables* editing the question first (Esc + retype
  to refine — cheap).
- *Rides:* today's grounding verbatim. The smallest possible win; it proves the loop.

### W2 — "Why did this fail?"  *(OPEN directive + triage entry)*
- *Moment:* a command just errored in a terminal pane.
- *Design:* action `ExplainFailure` (bind `C-x ?`, travel-mode `?`, bar row) opens the
  Ask pre-filled "Why did this fail? Cite the exact line and give the fix.", with the
  focused terminal's scrollback tail force-included. The "no essays" prompt rule steers
  the answer to end in a cited `file:line` (as `OPEN:`) and/or a fix (as gated `TYPE:`).
- *Choice — dedicated triage action vs. "just ask in English".* **Both** — a dedicated
  action *and* the general path. *Enables* a muscle-memory one-key reflex on the
  highest-emotion moment (the wedge), and it force-includes scrollback the user would
  not think to select; *disables* nothing (plain `?` still works).
- *Choice — OPEN auto-jumps vs. confirm.* **Confirm-gated** (Enter), like every
  directive. *Enables* consistency + safety; *disables* zero-friction jump (one Enter
  is the tax, worth it for the doctrine).
- *Rides:* Substrate 1 (OPEN), Substrate 2 (scrollback tail).

### W3 — "Do it in English"  *(bar shell mode, Tab-translate, cursor-anchored overlay)*
- *Moment:* knows the intent, not the incantation; already at a shell prompt.
- *Invocation:* `Ctrl+Space` then `!` — works from inside a terminal pane (the chrome
  layer already pops the bar there). Enters shell mode (`[SH !]`). *(Possible v1.1
  refinement: a dedicated one-chord opener; not needed for v1.)*
- *Translate:* in shell submode only, **Tab** sends the current text as "translate to
  ONE shell command" → the returned command replaces the query, shown editable →
  **Enter** types it into the focused terminal (`run_shell_command`). Enter is always
  "run what I see." No auto-detection of English-vs-command.
- *No eye-jump — the cursor-anchored overlay (the key UX choice):* the shell composer
  renders as a floating box **at the focused pane's cursor** (reusing the `(cx, cy)`
  that `render_terminal_pane` / `render_editor_pane` already return), not (only) in the
  bottom bar — like editor autocomplete. Content: `[SH !] text ▎`, then the translated
  command after Tab. Positioned just below the cursor (flips above near the bottom
  edge), width-capped with wrap, drawn with `Clear` + `Block` like `render_which_key`.
  - *Enables:* eyes never leave the prompt where the command will land; intent and
    result are spatially co-located; feels inline/native with no PTY-line hacking;
    generalizes later to an at-cursor ask overlay.
  - *Disables:* covers a few terminal rows while open (transient — closes on Enter/Esc);
    less horizontal room than the full-width bottom bar (mitigated by wrap + a sane
    width cap); adds edge-positioning logic (small, contained).
- *Choice — explicit Tab vs. auto-detect non-commands.* **Explicit Tab.** *Enables*
  zero false positives (a real command is never hijacked); *disables* pure-magic "type
  English into `!`" (one keystroke recovers it) — the legible choice for a surface that
  runs shell commands.
- *Choice — show command before running.* **Always.** *Enables* the teaching loop
  (Noor learns from approved commands) + safety; *disables* instant execution (unwanted
  here).
- *Rides:* `!` shell mode + `run_shell_command` + the confirm gate; a new render helper
  `render_cursor_overlay` anchored on the already-computed focused cursor position.

### W4 — Cross-pane reasoning  *(context selectors)*
- *Moment:* "does this terminal output match the function in the left pane?"
- *Design:* the default selector already sends all active-tab panes labeled by role +
  focus; the system prompt teaches the model it is seeing multiple panes and may
  reference them. `NEED: tab <name>` pulls another tab.
- *Choice — label panes by position vs. by content role.* **Role + focus marker**
  (editor:name / terminal + "(focused)"). *Enables* the model to disambiguate "the left
  pane" / "the terminal"; *disables* pixel-accurate spatial reasoning (it knows roles,
  not geometry — enough for the real questions).
- *Rides:* Substrate 2 default + cross-tab selector.

### W5 — Scrollback archaeology  *(on-demand full-scrollback selector)*
- *Moment:* "when did this first start failing?" over 10k retained lines.
- *Design:* the default ask carries only the tail; if the model needs history it emits
  `NEED: scrollback` → Mars re-asks with the full retained buffer (hard cap, head/tail
  if over) → answer. The 10k lines already exist (`terminal_scrollback_lines`).
- *Choice — always send full scrollback vs. on-demand.* **On-demand.** *Enables*
  affordable everyday asks (don't pay 10k lines to ask "what's this"); *disables*
  single-turn history answers (one extra round-trip when needed).
- *Choice — raw dump vs. Mars-side pre-filter.* **Raw within cap** for v1. *Enables*
  simplicity + the model does the search it is good at; *disables* precise handling of
  histories larger than the cap (head/tail truncation may miss the middle — a known v1
  limit; a Mars-side grep-prefilter is a later refinement).
- *Rides:* Substrate 2 expansion path.

### W6 — "Watch this and tell me"  *(trigger framework + notices)*
- *Moment:* a long build/deploy; the user wants to work or leave.
- *Design:* `> watch` (or action `WatchPane`) marks the focused terminal
  (`watched: bool`). Trigger = process-exit OR output-quiet-for-N s. On trigger →
  background-summarize the tail (auto_name pattern, `agent_busy` gate) → push ONE
  `Notice` → rendered as a single status-line ("build failed — linker error, pane 3"),
  dismissible; fires even while detached (daemon loop).
- *Choice — quiet-heuristic vs. explicit "done" detection.* **Output-quiet + exit**
  (both cheap, event-driven). *Enables* watching arbitrary long-running commands with no
  shell integration; *disables* precision (a genuinely slow-but-alive command may
  trigger early — mitigated by a generous default and "still running?" phrasing).
- *Choice — notify tier.* **Passive one-liner, no bell/popup** (Substrate 3B).
  *Enables* trust (never a firehose); *disables* urgency signaling — deliberate for v1.
- *Rides:* Substrate 3 fully.

### W7 — "Where was I?"  *(detach/attach snapshot + diff)*
- *Moment:* `mars attach` after lunch / Monday morning.
- *Design:* at detach/disconnect (hooks already exist in `session.rs`), snapshot per
  pane: scrollback line count, buffer dirty flags, watched-pane verdicts. At attach,
  diff against live → a dismissible 3-line panel, failures first, *absent when nothing
  changed*. Optionally one gated LLM pass over the diff for prose.
- *Choice — deterministic diff vs. LLM summary.* **Deterministic diff as the spine,
  LLM as optional polish.** *Enables* a briefing even with no API key, and cheapness;
  *disables* rich narrative unless the model runs (fine — the facts are the value).
- *Choice — snapshot granularity.* **Counts + flags, not full content diff.** *Enables*
  tiny snapshots that work across daemon lifetime; *disables* "show me exactly what
  changed in the log" (it reports *that* it grew by N and the tail, not a line diff).
- *Rides:* Substrate 3 (attach/detach triggers) + session daemon state.

---

## Part III — Sequencing & what each phase turns on/off

- **Phase 1 (W1, W2, W3) — days.** Directive `OPEN:`, triage action, translate gesture,
  no-essays prompt. *Turns on* the wedge + daily-use trio. *Leaves off* any new context
  beyond today's. Ships alongside pane resize/zoom (independent, acquisition-critical).
- **Phase 2 (W4, W5) — ~1 wk.** Context selectors + `NEED:` expansion. *Turns on* the
  only-Mars cross-pane / history moat. *Leaves off* proactivity.
- **Phase 3 (W6, W7) — ~1 wk.** Trigger framework + notices + snapshot/diff. *Turns on*
  the across-time agent. *Leaves off* louder/urgent tiers by design.

Cross-cutting, decided once: one global in-flight LLM gate; a selfcheck per substrate
(directive parse, selector budget, trigger firing, notice render, snapshot diff); every
proactive surface pull-rendered so the interruption budget is structural, not policy.

## Explicitly out of scope for these 7
Selection surgery (single-buffer edit — ruled buildable, separate track), full
parameterized actions (Phase 4 / W8, W10), the `INSPECT:` read-only autonomy allowlist
(pairs with W2 later), multi-file edits + the transaction journal, an urgent-notification
tier, and an in-scrollback grep prefilter.
