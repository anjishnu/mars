# Mars — Engineering Design: AI-Enabled Workflows (W1–W7)

*This is the engineering companion to [`workflows_design.md`](./workflows_design.md) (the
product spec of W1–W7) and [`strategy.md`](./strategy.md) §4 (the six primitives). It is
grounded in the code as it stands: every proposal cites a real `file:function`, extends a
real struct/enum, and lands on the existing `tick()` / daemon / directive seams. It drives
implementation — decisions are made, not enumerated.*

Reading order for an implementer: this doc → the two functions it leans on hardest
(`app.rs:tick` and `session.rs:server_main`) → the primitive you're building.

---

## 1. Current state — what is shipped and what substrate exists

### 1.1 Shipped workflows

| WF | Ships as | Entry point | Seam it rides |
|----|----------|-------------|---------------|
| **W1** ExplainThis | `Action::ExplainThis` → `app.rs:ask_prefilled("Explain what's on screen…")` | `C-x e`, bar row | `screen_context` + `submit_agent_query` |
| **W2** ExplainFailure | `Action::ExplainFailure` → `ask_prefilled("Why did this fail?…")` | `C-x ?`, travel `?` (`app.rs:1870`), bar row | `screen_context` + `OPEN:` directive |
| **W3** shell-translate | `agent::translate_shell` → `AgentEvent::ShellTranslation` | `Ctrl+Space` in `Mode::Terminal` → `BarMode::Shell`; Tab/Enter translates | `render_shell_overlay` at `app.cursor_screen` |

All three are thin: they seed a canned string and submit, or add one enum arm. The real
substrate is what they share.

### 1.2 The reusable substrate already in place

**The directive seam — `agent.rs`.** `parse_directive(text) -> (String, Option<AgentDirective>)`
is a pure, unit-tested function that scans the last 4 non-empty lines, strips markdown noise
(`match_directive`), and returns a display string plus at most one directive:

```rust
pub enum AgentDirective { Run(String), Type(String), Open(String) }   // agent.rs:9
```

Every directive is *applied* in exactly one place — `app.rs:handle_bar_ask`, the `Enter` arm
(2103–2131) — behind the confirm gate:
- `Run(name)` → `Action::from_name` → `run_action`, but destructive actions
  (`Action::is_destructive`, `palette.rs:137`) route through `PromptKind::ConfirmAction` first.
- `Type(cmd)` → `run_shell_command` (reuse-or-open a terminal pane, write bytes + `\n`).
- `Open(loc)` → `open_at` (parse `path:line`, split if a terminal is focused, goto, recenter).

This is the operator boundary. **New workflows that want the agent to *do* something add an
`AgentDirective` variant + one arm in `handle_bar_ask`, and nothing else changes.**

**The context slice — `app.rs:screen_context` (2643).** A single `String` builder, `CAP = 6*1024`,
head/tail truncated. Emits `session:` + `tabs:` + every pane of the active tab (editor: the
visible window `scroll_row .. scroll_row+view_h+10`; terminal: full `screen().contents()`),
each labeled with role and a `(focused)` marker. It is called by `submit_agent_query`,
`translate_shell_query`, `maybe_auto_name`, and `maybe_auto_name_session`. **This is the
Context Bus v0 — one function, four consumers, no registry.**

**The background-agent pattern — `auto_name`.** `app.rs:maybe_auto_name` (3133) is the
template every proactive feature copies: gate on a single in-flight bool
(`auto_name_inflight`), fire on a cadence (`frame_tick % ticks == 0`), spawn a background
thread (`agent::auto_name`) that delivers via an `AgentEvent` variant, drained and applied in
`tick` (3052). One request in flight, result applied on the main thread, user-rename wins the
race (numeric-name check on apply). **W6's watcher and W7's briefing are this pattern with a
different trigger and a different `AgentEvent`.**

**The event clock — `app.rs:tick` (3013).** Called once per main-loop iteration (standalone)
*and once per `session.rs:server_main` loop iteration whether or not a client is attached*.
It (1) bumps `frame_tick`, (2) drains `term_rx` marking `Term::exited`, (3) autosaves on a
cadence, (4) drains `agent_rx`, (5) runs `maybe_auto_name*`. **Everything proactive hangs off
this function, and its daemon-residence is exactly why W6/W7 are impossible for tmux.**

**The Ask panel confirm-gate.** `agent_pending` (spinner), `agent_answer` (transient
error/notice), `agent_directive` (the pending action shown before `Enter`), `agent_history`
(conversation, last 12 turns sent by `agent::build_messages`). The panel renders the
transcript; `Enter` on an empty query fires the gated directive. This is the pull-rendered,
confirm-before-act discipline the proactive tier must also obey.

---

## 2. The primitives, engineered

### 2.1 Context Bus — formalize `screen_context` into a `ContextSource` registry

**Problem with today's `screen_context`.** It is a hard-coded sequential builder: adding git
status, the project index, or scrollback means editing one 60-line function and re-testing all
four callers. There is no per-source budget (one giant buffer can starve the terminal tail),
no consent (a secrets file in a pane is always sent), and no way for the model to ask for
*more* of a specific source.

**Design — a source registry with a token budget.** Each source is a small struct that can
snapshot itself into a budgeted string and declare a default consent. Sources are pure reads
of `&App`; the bus composes the enabled ones under a global budget and tags each block.

```rust
// context.rs (new)
pub struct Budget { pub remaining: usize }           // chars, seeded from tuning.agent_context_budget
impl Budget { fn take(&mut self, n: usize) -> usize { let g = n.min(self.remaining); self.remaining -= g; g } }

/// A named, consented, budgeted slice of workspace state.
pub trait ContextSource {
    fn id(&self) -> &'static str;                    // "buffers", "terminals", "git", "index", "selection"
    fn default_consent(&self) -> bool { true }
    /// Render at most `budget` chars, or None if this source has nothing now.
    fn snapshot(&self, app: &App, budget: &mut Budget) -> Option<String>;
}
```

`screen_context` becomes the composition, tagging each block so the model knows what it is
seeing and — critically — what it can ask for:

```rust
// app.rs
fn build_context(&self, sources: &[Box<dyn ContextSource>]) -> String {
    let mut budget = Budget { remaining: self.tuning.agent_context_budget };
    let mut out = String::new();
    for s in sources {
        if !self.consent.get(s.id()).copied().unwrap_or_else(|| s.default_consent()) { continue; }
        if let Some(block) = s.snapshot(self, &mut budget) {
            out.push_str(&format!("\n### source:{} ###\n{}\n", s.id(), block));
        }
    }
    out
}
```

Migration is mechanical and keeps `screen_context()`'s exact output as the default source set,
so the four existing callers don't change behavior:
- `SessionTabsSource` — the `session:`/`tabs:` header (always, tiny).
- `BuffersSource` — the visible editor windows (today's editor branch, 2664–2678).
- `TerminalsSource` — the terminal `screen().contents()` branch (2680–2686), but budgeted:
  **default to the visible screen only**, not the full scrollback (which today's code already
  does — `screen()` is the live grid, not history).
- `SelectionSource` — replaces the inline `selected_text()` append in `submit_agent_query`
  (2586); consent defaults true, budget generous (selections are small and precise).

New sources register once and light up every consumer:
- `GitStatusSource` — shells `git status --porcelain` + `git diff --stat` async (reuse the
  `delighters_design.md` git-gutter's async shell pattern), cached per `frame_tick` window.
- `IndexSource` — the `project::Index` file list (already lazily built by
  `ensure_project_index`), truncated to names under budget.
- `ScrollbackSource` — **not in the default set**; supplied only on `NEED: scrollback`
  (below), reading `Term`'s retained history up to a hard cap.

**The `NEED:` selector-expansion — the model-driven vs. send-everything tradeoff.** Two ways to
decide whether a question needs history/another tab:

- *Send everything* — put full scrollback + all tabs in every prompt. Simple, single-turn,
  but blows the budget (10k lines on "what's this?") and distracts the model.
- *Model-driven expansion* — the default context tells the model what exists and how to ask;
  the model emits a `NEED:` line; Mars re-asks once with that source expanded.

**Decision: model-driven `NEED:`.** It is the only option that keeps everyday asks cheap
(free-tier discipline) while still answering history questions. Cost is one extra round-trip
*only when the model decides it needs one*. This is a **read-side directive** — parsed by the
same `parse_directive` machinery but handled before display instead of after confirm:

```rust
pub enum AgentDirective {
    Run(String), Type(String), Open(String),
    Need(NeedKind),                              // NEW — read-side, auto-satisfied, never gated
}
pub enum NeedKind { Scrollback, Tab(String), Git }
```

In `app.rs:tick`, the `AgentEvent::Answer` arm checks for a `Need` directive *before* pushing
to history: if present, re-invoke `agent::ask` with the extra source enabled and a
`need_depth` counter (hard cap 1 — one expansion, never a loop), and do **not** surface the
intermediate answer. This reuses the existing one-in-flight discipline: the expansion request
is just another background `ask`.

- *Enables:* scrollback (W5) and cross-tab (W4) without a fixed token tax; cheap default asks.
- *Disables:* single-turn history answers (the round-trip is the honest price); determinism
  (same question may or may not expand). Both accepted in `workflows_design.md` §2B.

**New knobs (`tuning.rs`)**, following the `{value, description}` convention:
- `agent_context_budget` (default 6144) — "Total characters of workspace context sent with an
  ask, split across sources. Higher = more grounding, more tokens/cost."
- `agent_scrollback_context` (default 200) — "Lines of terminal scrollback appended when the
  focused pane is a terminal (W2/W5). Full history is fetched only on the model's `NEED:`."

### 2.2 Trigger / Watch framework — the priority (unlocks W6 + W7)

This is the moat-widener per `strategy.md` §7: it turns the shipped *sight* into across-time
*vigilance*, and it is daemon-resident, which tmux and ephemeral CLIs structurally cannot
match. It is built entirely inside `app.rs:tick` and the two `session.rs:server_main` hooks —
no new threads beyond the existing background-agent spawn.

**The three trigger signals, all already present:**

| Trigger | Source | Detection |
|---------|--------|-----------|
| process-exit | `TermEvent::Exited(id)` (`terminal.rs:16`, sent on reader EOF) | already drained in `tick` (3018); set a `WatchState` verdict-pending flag |
| output-quiet | `TermEvent::Output(id)` + `frame_tick` | record `last_output_tick[id]`; quiet ⇔ `frame_tick - last_output_tick > quiet_ticks` |
| detach / attach | `session.rs:server_main` — `SrvEvent::ClientGone` (316) / `SrvEvent::Attach` (292) | call `app.on_detach()` / `app.on_attach()` hooks |

**Data structures.** Watch state is per-terminal, so it keys on `TermId` and lives beside
`terms`. A pane opts in; the registry tracks last output, whether the trigger has fired, and
whether a verdict is still owed.

```rust
// app.rs — new App fields
pub struct WatchState {
    pub watched: bool,          // user ran `> watch` / Action::WatchPane on this pane
    pub last_output_tick: u64,  // frame_tick of the most recent TermEvent::Output
    pub triggered: bool,        // quiet-or-exit already fired → don't re-fire until new output
    pub verdict: Option<String>,// last summary ("build failed — linker error"), for W7 diff
}
// App { … , watches: HashMap<TermId, WatchState>, notices: Vec<Notice>, bg_busy: bool }

pub struct Notice {
    pub text: String,           // "build failed — linker error (pane 3)"
    pub kind: NoticeKind,       // Failure | Change | Info — orders the queue, failures first
    pub tick: u64,              // for age / dismissal
}
```

**Tick integration** — a new `maybe_fire_watches()` called at the end of `tick`, mirroring
`maybe_auto_name`. First, feed the trigger clock while draining `term_rx` (extend the existing
loop at 3017):

```rust
while let Ok(ev) = self.term_rx.try_recv() {
    match ev {
        TermEvent::Output(id) => {
            if let Some(w) = self.watches.get_mut(&id) { w.last_output_tick = self.frame_tick; w.triggered = false; }
        }
        TermEvent::Exited(id) => {
            if let Some(t) = self.terms.get_mut(&id) { t.exited = true; }
            if let Some(w) = self.watches.get_mut(&id) { if w.watched && !w.triggered { self.pending_watch = Some((id, WatchReason::Exit)); } }
        }
    }
}
```

```rust
fn maybe_fire_watches(&mut self) {
    if self.bg_busy || self.agent_pending { return; }        // one global in-flight; user asks win
    let quiet_ticks = self.tuning.watch_quiet_secs * 1000 / self.tuning.poll_interval_ms.max(1);
    // Prefer an exit trigger already queued; else find the first quiet watched pane.
    let fire = self.pending_watch.take().or_else(|| {
        self.watches.iter().find(|(_, w)|
            w.watched && !w.triggered && self.frame_tick - w.last_output_tick > quiet_ticks
        ).map(|(id, _)| (*id, WatchReason::Quiet))
    });
    let Some((id, reason)) = fire else { return };
    if let Some(w) = self.watches.get_mut(&id) { w.triggered = true; }
    let cfg = agent::AgentConfig::from_env();
    if !cfg.is_configured() { return; }
    let tail = self.terminal_tail(id, self.tuning.agent_scrollback_context);
    self.bg_busy = true;
    agent::watch_summary(cfg, id, reason, tail, self.agent_tx.clone());
}
```

`agent::watch_summary` is a copy of `agent::auto_name` (agent.rs:218) with a triage prompt
("One line: did this succeed or fail, and the single most important reason. No preamble.") and
a new event:

```rust
pub enum AgentEvent { …, WatchSummary { term_id: TermId, verdict: String } }
```

Applied in `tick`: clear `bg_busy`, store `watches[id].verdict = Some(verdict.clone())`, and
`self.notices.push(Notice { text, kind: NoticeKind::Failure_or_Info, … })`. Failures sort
first (`NoticeKind` ordering).

**The pull-model notices queue — the interruption budget made structural.** The agent has *no
push path to the screen*. `notices: Vec<Notice>` is only ever *read* by the renderer.
`ui.rs`'s status-line render gains one line: if `!app.notices.is_empty()`, show the
highest-priority notice (`Failure` before `Change` before `Info`) as a single dim status line
with a dismiss hint. Dismissal is a keystroke (`Esc` when no bar is open, or a bound
`DismissNotice`) that pops the front of the queue. **Never modal, never a chime, at most one
line, silent when the queue is empty** — the "err quiet" doctrine of `agentic_inline.md` §5 is
enforced by the data flow, not by policy.

**One global in-flight gate.** Generalize today's `auto_name_inflight` into `bg_busy`,
covering auto-name + session-name + watch + briefing. Foreground asks keep their own
`agent_pending` and **preempt**: `maybe_fire_watches` and `maybe_auto_name` both early-return
when `agent_pending` is set, so a user question is never starved by background work, and a
watch summary simply waits for the next quiet tick.

- *Enables:* free-tier survival (no thundering herd when three panes finish at once);
  watch-while-detached (all of this runs in `server_main`'s `app.tick()` with no client).
- *Disables:* parallel background summaries; sub-second reaction (bounded by `poll_interval_ms`
  and `watch_quiet_secs`). Both fine — vigilance is not a race.

**New knob:** `watch_quiet_secs` (default 20) — "Seconds a watched pane must be silent before
Mars summarizes it (W6). Also fires immediately on process exit. Generous by design — a false
'done' costs more than the feature earns."

### 2.3 Parameterized actions — typed args on directives

Today `AgentDirective::Run(String)` carries only an action *name*; `Action::from_name`
round-trips it through serde. A directive cannot say "find file X" or "goto line N" — `OPEN:`
is a bespoke string-parse precisely because the directive vocabulary can't carry an argument.
This blocks multi-step plans (W-later) and makes every parameterized action a special case.

**Design — one typed-arg form, additive over the shipped parser.** Extend the enum and teach
`match_directive` a `Name(arg, arg)` shape while keeping the bare-name form working:

```rust
pub enum AgentDirective {
    Run(String),                        // RUN: SplitVertical            (bare, shipped)
    RunWith(String, Vec<String>),       // RUN: FindFile("src/app.rs")   (NEW, typed)
    Type(String), Open(String), Need(NeedKind),
}
```

`match_directive` (agent.rs:49): after matching `RUN:`, if the remainder is `Name(args…)`
parse the paren-list (comma-split, strip quotes) into `RunWith`; else fall back to the
first-token `Run` as today. `Action` grows an arg-carrying dispatch — the cleanest migration
is to fold `Open` into `RunWith("Open", [loc])` and `FindFile`/`GotoLine` into the same shape,
so `handle_bar_ask` gains one arm:

```rust
Some(AgentDirective::RunWith(name, args)) => {
    self.close_bar();
    match (name.as_str(), args.as_slice()) {
        ("Open", [loc])        => self.open_at(loc),
        ("FindFile", [path])   => { self.open_file(path).ok(); }
        ("GotoLine", [n])      => { if let Ok(n) = n.parse() { self.goto_line(n); } }
        _                      => self.agent_answer = Some(format!("⚠ unknown action: {name}")),
    }
}
```

A **plan** is then just `Vec<AgentDirective>` (relax `parse_directive`'s one-per-reply rule to
collect all matching trailing lines) confirmed once — but that is out of scope for W1–W7 and
stays a Phase-4 item. For the seven workflows, `RunWith` is only needed if W4/W5 want the
model to *act* across panes; the read-side `Need` covers their context needs, so
**parameterized actions are a Phase-3 nice-to-have here, not a blocker.** Ship the enum arm
when the first W8-class action lands.

- *Enables:* multi-step plans, macros, `FindFile`/`GotoLine` as first-class directives, schema
  validation later.
- *Disables:* nothing shipped — `Run`/`Type`/`Open` parse unchanged; the seam is the same
  `parse_directive` → `handle_bar_ask` path.

---

## 3. Each remaining workflow, engineered

Ranked by value × tractability (highest first). W6 and W7 lead because the Trigger framework
is the moat-widener and they share all of its machinery; W5 and W4 are cheaper but ride the
already-designed Context Bus expansion.

### W6 — "Watch this and tell me" *(value: high · tractability: high)*  ← **build first**

- **Trigger:** `TermEvent::Exited(id)` OR output-quiet (`frame_tick - last_output_tick >
  watch_quiet_secs`), evaluated in `maybe_fire_watches` (§2.2).
- **Context assembled:** `terminal_tail(id, agent_scrollback_context)` — the last ~200 lines of
  the watched pane's `screen().contents()` (extend to retained history via `scroll_view` read
  if the tail is short). No other panes — a watch is about *this* command.
- **Agent call:** `agent::watch_summary` (auto_name clone), triage prompt, `bg_busy` gated.
- **Output surface:** one `Notice` pushed to the queue → single status line, failures first,
  dismissible. **Fires while detached** because `server_main` calls `app.tick()` every loop
  with no client (session.rs:289).
- **Data structures:** `WatchState`, `Notice`, `AgentEvent::WatchSummary`, `Action::WatchPane`
  (+ `label()` arm "watch this pane", `palette.rs` menu row, `run_action` arm that sets
  `watches.entry(term_id).watched = true`). Not destructive → no confirm gate needed.
- **Integration points:** `app.rs:tick` (term_rx loop + `maybe_fire_watches`), `agent.rs`
  (new fn + event), `ui.rs` (notice status line).
- **Tradeoffs:**
  1. *Quiet-heuristic vs. shell "done" detection.* **Quiet + exit** — no shell integration
     needed, works on any command; a slow-but-alive command may trigger early, mitigated by a
     generous default and "still running?" phrasing in the summary.
  2. *Per-pane summary vs. whole-tab.* **Per-pane** — the trigger is a specific command; a tab
     summary would dilute the verdict.
  3. *Notice tier.* **Passive one-liner only** — no bell/popup; sets the trust precedent
     (`agentic_inline.md`: "the first false alarm costs more than the feature earns").

### W7 — "Where was I?" *(value: high · tractability: high)*

- **Trigger:** detach (`SrvEvent::ClientGone` → `app.on_detach()`), attach (`SrvEvent::Attach`
  → `app.on_attach()`). Add these two calls to `session.rs:server_main` beside the existing
  `attached.store(...)` / `app.autosave()` at 320 and 298.
- **Context assembled:** a cheap deterministic snapshot at detach — **counts and flags, not
  content**:

  ```rust
  pub struct Snapshot {
      per_pane: HashMap<PaneId, PaneSnap>,   // scrollback line count, exited flag
      dirty_buffers: Vec<String>,            // buf.name where buf.modified
      verdicts: HashMap<TermId, String>,     // watches[id].verdict at detach time
      tick: u64,
  }
  ```
- **Agent call:** *optional and gated.* The deterministic diff is the spine — at attach, diff
  the live state against `Snapshot` (panes whose line count grew, shells that exited while
  away, buffers now dirty, watch verdicts). Only if a key is configured, one gated
  `agent::ask`-style pass turns the diff into prose. **Absent when nothing changed.**
- **Output surface:** a dismissible ≤3-line panel at attach (a `Notice` batch, or a dedicated
  `briefing: Option<Vec<String>>` rendered like the dead-shell overlay), failures first.
- **Data structures:** `Snapshot`, `App::detach_snapshot: Option<Snapshot>`, `on_detach`,
  `on_attach`, `diff_snapshot`.
- **Integration points:** `session.rs:server_main` (two hook calls), `app.rs` (snapshot/diff),
  `ui.rs` (briefing panel).
- **Tradeoffs:**
  1. *Deterministic diff vs. LLM summary.* **Deterministic spine, LLM optional polish** —
     works with no API key, cheap, and the facts are the value.
  2. *Snapshot granularity.* **Counts + flags, not content diff** — tiny snapshots that
     survive daemon lifetime; reports "log grew by N lines" and the tail, not a line-diff.
  3. *Snapshot on every detach vs. only when watched.* **Every detach** — the whole point is
     that leaving is when you lose context; the snapshot is a few hundred bytes.

### W5 — Scrollback archaeology *(value: med-high · tractability: high)*

- **Trigger:** user ask while a terminal is focused; model emits `NEED: scrollback` (§2.1).
- **Context assembled:** default = terminal tail (`agent_scrollback_context`). On `NEED:`,
  `ScrollbackSource` supplies the full retained history up to a hard cap (`terminal_scrollback_lines`
  = 10k already retained by `vt100::Parser`), head/tail truncated if over budget.
- **Agent call:** the re-ask path in `tick`'s `Answer` arm (§2.1), `need_depth` capped at 1.
- **Output surface:** the normal Ask transcript.
- **Data structures:** `NeedKind::Scrollback`, `ScrollbackSource`, `need_depth` counter.
- **Integration points:** `agent.rs:match_directive` (Need), `app.rs:tick` (re-ask),
  `context.rs` (ScrollbackSource reading `Term`).
- **Tradeoffs:**
  1. *Always-send vs. on-demand.* **On-demand** — don't pay 10k lines to ask "what's this?".
  2. *Raw dump vs. Mars-side grep-prefilter.* **Raw within cap** for v1 — the model searches
     well; a prefilter is a later refinement (head/tail truncation may miss the middle, a known
     v1 limit).
  3. *Reading history — `screen()` vs. `scroll_view` sweep.* Add `Term::scrollback_text(n)`
     that snapshots the parser's history rows directly (cleaner than mutating `view_offset`).

### W4 — Cross-pane correlation *(value: med · tractability: high — mostly shipped)*

- **Trigger:** a normal ask; `screen_context` already sends *all* active-tab panes with role +
  `(focused)` labels, so single-tab cross-pane reasoning **works today**. The only new piece is
  cross-*tab*.
- **Context assembled:** default multi-pane context (shipped) + `NEED: tab <name>` →
  `TabSource` pulls another tab's panes.
- **Agent call:** the `Need(NeedKind::Tab)` re-ask path.
- **Output surface:** Ask transcript.
- **Data structures:** `NeedKind::Tab(String)`, a system-prompt line teaching the model it sees
  multiple labeled panes and may `NEED: tab <name>`.
- **Integration points:** `agent.rs:system_prompt` (one sentence), `app.rs:tick` (re-ask),
  `context.rs` (TabSource).
- **Tradeoffs:**
  1. *Label by position vs. by role.* **Role + focus marker** (already done) — the model
     disambiguates "the terminal" / "the left pane" without pixel geometry.
  2. *All tabs always vs. on-demand.* **On-demand `NEED: tab`** — active tab is the common case;
     cross-tab is the exception, don't pay for it every ask.
  3. *Whether W4 needs any new code at all.* **Barely** — it is the cheapest workflow; single-tab
     is shipped, cross-tab is one `NeedKind` arm. Ranked below W5/W6/W7 only because its
     incremental value over today is smallest.

### Not in W4–W7 but in the design's scope

`workflows_design.md` scopes exactly W1–W7; W1/W2/W3 are shipped (§1.1). The `config concierge`
(`agentic_inline.md` #4) is *not* one of the seven but is nearly free given the `{value,
description}` knob format — it is a `RunWith("EditTuning", …)` once §2.3 lands, and is noted
here only so it isn't rediscovered later. Out of scope for this doc.

---

## 4. Phasing & dependencies

```
Context Bus (registry) ──┬─► W4 cross-pane (mostly shipped; +TabSource)
                         └─► W5 scrollback (ScrollbackSource + NEED:)

Trigger/Watch framework ─┬─► W6 watch-while-detached   ◄── HIGHEST LEVERAGE
                         └─► W7 reattach briefing

Parameterized actions ──────► (Phase-4 plans; not required by W1–W7)
```

**Build order (opinionated, leads with the moat-widener per `strategy.md` §7):**

1. **Trigger/Watch framework → W6, then W7.** This is the priority. W6 is the smaller half
   (all inside `tick` + one `AgentEvent` + `Action::WatchPane`); W7 adds the two `server_main`
   hooks and the snapshot/diff. Together they turn shipped *sight* into *vigilance* — the
   capability no competitor can follow without a daemon. Start here even though the Context Bus
   is "more foundational," because W6 needs only `last_output_tick` + the notices queue, not the
   full registry.

2. **Context Bus registry → W5, then W4.** Refactor `screen_context` into `ContextSource`s
   (behavior-preserving), add `Need`/`ScrollbackSource`/`TabSource`. W5 is the higher-value
   half (history archaeology is uniquely Mars); W4 cross-tab is a one-arm follow-on.

3. **Parameterized actions (`RunWith`).** Only when the first acting-across-panes or
   config-concierge workflow lands. The enum arm is cheap; defer until there's a consumer.

Cross-cutting, decided once and shared by all: **one global `bg_busy` in-flight gate**;
**every proactive surface pull-rendered** through `notices`/`briefing` (no push path); **a
selfcheck per new seam** (§5).

---

## 5. Verification plan

The `--selfcheck` harness (`main.rs:selfcheck`) drives the real `App` on
`ratatui::TestBackend(120,40)`, spawns real PTYs, and is **hermetic** — it clears
`GEMINI_API_KEY`/`GROQ_API_KEY`/`MARS_LLM_*`/`ARES_LLM_*` at the top (255–267) so no inherited
key flips a code path. Background agent replies are simulated by pushing an `AgentEvent`
straight onto the channel and ticking:

```rust
app.agent_tx.send(agent::AgentEvent::WatchSummary { term_id, verdict: "build failed — linker error".into() })?;
app.tick();   // drains agent_rx, applies the event, clears bg_busy
```

This is exactly how the shipped auto-name test works (`main.rs:997`). Per-primitive checks:

- **Context Bus registry** — assert `build_context` with the default source set produces the
  same tagged blocks as today; assert `Budget` truncation caps total length; assert a
  `NEED: scrollback` answer triggers exactly one re-ask (spy on a fake `AgentConfig` or assert
  `need_depth` increments and stops at 1). No real key needed — inject the `Answer` event
  carrying a `Need` directive.
- **Directive parser** — extend the existing pure-function tests (`main.rs:1015–1033`):
  `parse_directive("…\nRUN: FindFile(\"src/app.rs\")")` → `RunWith("FindFile", ["src/app.rs"])`;
  `NEED: scrollback` → `Need(Scrollback)`; bare `RUN: SplitVertical` still parses.
- **Trigger — output-quiet** — spawn a real PTY (like `main.rs:561`), mark it `watched`, drive
  `frame_tick` past `watch_quiet_secs` worth of ticks with no `Output`, assert
  `maybe_fire_watches` sets `bg_busy` / emits the request (gate on config so it no-ops without a
  key, then inject `WatchSummary` and assert a `Notice` lands).
- **Trigger — process-exit** — reuse the shipped dead-shell test (`main.rs:587`): `exit` the
  shell, tick until `TermEvent::Exited` drains, assert a watched pane queues a verdict-pending.
- **Notices render** — push a `Notice`, `term.draw`, assert the status line contains the text
  (via `screen_text`, the parsed TestBackend buffer — never raw-byte `contains`, per the
  ratatui cell-diff gotcha in `AGENTS.md`).
- **W7 snapshot/diff** — call `on_detach()`, mutate a buffer / grow a terminal, call
  `on_attach()`, assert `diff_snapshot` reports the dirty buffer and the growth; assert the
  briefing is `None`/empty when nothing changed.
- **W4/W5 `NEED:`** — inject an `Answer{ directive: Some(Need(Tab("api"))) }`, tick, assert the
  intermediate answer is *not* pushed to `agent_history` and a re-ask fires.

**What still needs a real-terminal / PTY pass** (headless can't cover, per `AGENTS.md`
§"What headless testing cannot verify"):
- **Watch-while-detached end-to-end** — the daemon calling `app.tick()` with no client. Verify
  via `script -q /dev/null mars --session w6 …`, start a `sleep 3; false`, detach, wait, `mars
  attach`, confirm the verdict is waiting. `app.tick()`'s daemon-residence is proven by the
  session tests but the *detached-summarize* path only exists in `server_main`.
- **Detach/attach hooks firing** — `SrvEvent::ClientGone`/`Attach` only occur over a real
  socket; selfcheck's `App` isn't run under `server_main`. Drive `on_detach`/`on_attach`
  directly in selfcheck for the diff logic, but confirm the *wiring* in a real session.
- **Real notice legibility** — one-line-at-a-time redraw of the notice status line in a real
  terminal (cursor position, no flicker).

---

## 6. Risks

- **Interruption budget / proactivity firehose.** The single largest product risk. Mitigated
  *structurally*: the agent has no push path — it can only append to `notices`, which the
  renderer reads and shows one line of, failures first, dismissible. No modal, no chime.
  `watch_quiet_secs` defaults generous. The first false alarm is the feature's whole reputation
  — err quiet. **Do not add an "urgent" tier in v1** even though the queue could support it.
- **Token cost / free-tier limits.** Every watch trigger and briefing is an LLM call. The one
  global `bg_busy` gate prevents fan-out when three panes finish at once; foreground asks
  preempt so users never wait on background work; `NEED:` keeps default asks small. Watch
  summaries reuse the tiny `auto_name`-style prompt (no registry, short tail). Still, a user who
  watches ten panes on a chatty CI box will spend tokens — `watch_quiet_secs` and the
  one-in-flight gate are the throttle; consider a per-session watch-summary rate cap if abuse
  shows up.
- **Transaction-journal dependency for write-heavy workflows.** W1–W7 are read-mostly: the only
  writes are `TYPE:`/`OPEN:`/`RunWith` (already confirm-gated and single-step) and the shipped
  refactor (one `checkpoint()`, C-/ reversible). **No W1–W7 workflow needs the journal.** But
  the moment a workflow proposes a *multi-step* or *multi-file* edit (Phase-4 plans, autonomous
  triage-and-fix), it must wait on the transaction journal (`strategy.md` P5) — reversibility,
  not permission dialogs, is what makes autonomy adoptable. Keep `RunWith` plans read-only or
  single-edit until the journal lands.
- **Daemon memory.** `WatchState` and `Snapshot` are counts/flags/short strings — negligible.
  The real memory is the existing 10k-line `vt100` scrollback per terminal (`terminal.rs:68`),
  unchanged by this work. `notices` and `agent_history` are bounded (history already sends only
  the last 12 turns; cap `notices` length and drop oldest `Info` first). No new unbounded state.
- **`bg_busy` starvation of auto-naming.** Folding auto-name + watch + briefing into one gate
  means a busy watch cadence could delay tab auto-naming. Acceptable — auto-naming is
  best-effort and one-shot; watches are the higher-value signal. If it bites, give auto-name a
  separate low-priority slot rather than a second concurrent request.

---

## Executive summary — build order

1. **Build the Trigger/Watch framework first — it is the moat-widener** (`strategy.md` §7):
   daemon-resident vigilance that tmux and ephemeral CLIs structurally cannot copy.
2. **The single highest-leverage first workflow is W6 ("watch this pane").** It is entirely
   inside `app.rs:tick` — record `last_output_tick` per `TermId`, fire on `TermEvent::Exited`
   or output-quiet, summarize via an `auto_name`-clone under one `bg_busy` gate, push one
   `Notice`. It fires while detached because `server_main` ticks with no client — the exact
   thing no competitor can follow.
3. **Then W7 (reattach briefing)** — same framework, add `on_detach`/`on_attach` hooks in
   `server_main` and a deterministic counts-and-flags `Snapshot` diff (LLM optional).
4. **Then the Context Bus registry** — refactor `screen_context` into `ContextSource`s
   (behavior-preserving), add the read-side `NEED:` directive → **W5** (scrollback) then **W4**
   (cross-tab; single-tab is already shipped).
5. **Parameterized actions (`RunWith`) last** — the enum + parser change is cheap but has no
   W1–W7 consumer; defer to Phase-4 plans.
6. Cross-cutting, decided once: one global `bg_busy` in-flight gate (user asks preempt); every
   proactive surface pull-rendered through `notices`/`briefing` (no push path — the interruption
   budget is structural, not policy); one selfcheck per new seam, agent replies simulated via
   `app.agent_tx.send(...)` + `app.tick()`.
7. No W1–W7 workflow needs the transaction journal; keep any `RunWith` plan read-only or
   single-edit until the journal lands, per `strategy.md` P5.
