# Mars — AI Product Strategy

*Which scenarios Mars uniquely owns, the customer time it saves (before/after), the
primitives that unlock them (with engineering designs and trade-offs), and the final
build recommendation.*

---

## 1. The thesis

Every competitor puts the AI **next to** your work. Mars puts it **inside the thing that
holds your work** — a persistent daemon that owns your panes, scrollback, editor buffers,
layout, and a session that survives the weekend. Two properties are load-bearing and **no
competitor has both**:

- **Line of sight** — the AI reads *every* pane (editor + terminal + scrollback + layout)
  as structured context, with no copy-paste.
- **Persistence** — the workspace lives in a daemon that outlives the client, the SSH link,
  and the closed laptop.

Every ownable scenario is the intersection of those two. That is an *architecture*
difference, not a feature difference — which is what makes the scenarios below defensible
rather than rentable.

---

## 2. Scenarios, ranked by ownability × frequency

Ownability = how architecturally impossible it is for an incumbent to copy without
rebuilding their foundations.

| # | Scenario | Why incumbents lose it | Mars property |
|---|---|---|---|
| 1 | **Failure triage** — "why did this fail?" | tmux has no AI; Claude Code/aider in a pane can't see the *adjacent* pane; Warp sees its own block, not your editor buffer | cross-pane sight + scrollback |
| 2 | **Remote/SSH dev** — AI lives on the box | Cursor's AI runs on the laptop while state is on the box, across a laggy link; the daemon+agent should be *where the state is* | daemon-resident agent + thin client |
| 3 | **Watch-while-detached** — the long run you check on | structurally impossible for all: tmux's daemon has no AI; Cursor/Claude-Code die with the window | trigger framework in the daemon |
| 4 | **Reattach briefing** — "where was I?" | nobody has a persistent, self-observing workspace to diff | detach/attach snapshot + diff |
| 5 | **Cross-pane correlation** | only Mars holds editor buffer + terminal output as co-present labeled context | context bus |
| 6 | **On-call / incident response** | shuttling logs into a chat tab under pressure is the worst time to be a clipboard | sight + session-as-war-room |
| 7 | **Scrollback archaeology** — "when did this first fail?" | tmux has the history but can't reason over it | on-demand scrollback selector |
| 8 | **English→shell, situational** | *contested* — Warp does AI command search; Mars's edge (cwd + scrollback context, shown-before-run teaches) is real but narrow | `!` translate + confirm gate |

**The pattern:** 1–5 are ours because of *architecture*; only #8 is contested — because it
needs neither sight nor persistence. **Invest where the architecture is the moat; treat the
contested ones as retention glue, not the wedge.**

---

## 3. Customer workflows: before / after (validating time saved)

Concrete, per-occurrence. "Daily save" multiplies by a conservative frequency for a working
developer. These are the numbers that justify the build.

### 3.1 Failure triage — **the crown scenario**
- **Before:** command errors → select the stack trace → switch to a browser/chat tab →
  paste → type 2-3 sentences of context ("I'm in this repo, ran X, here's the output") →
  read → switch back → find the file → jump to the line → apply. **~2–4 min**, and it
  breaks flow.
- **After:** `C-t ?` → 3-line diagnosis grounded in the pane + scrollback → `Enter` to jump
  to the cited `file:line` or run the fix. **~15–30 s**, no context switch.
- **Save: ~2–3 min × ~8/day ≈ 20–25 min/day.** Highest-frequency, highest-emotion moment.

### 3.2 Remote/SSH dev
- **Before:** the error is on box 2; the AI is on the laptop. Copy across a laggy link, or
  run a separate CLI on the box that sees only its pane, and re-explain context every query.
  Repeated clipboard round-trips; state and intelligence on opposite ends of the link.
- **After:** the daemon *and* the agent run on the box; the thin client attaches. The agent
  already sees every pane and the file open in the editor. No clipboard crosses the link.
- **Save: ~1–2 min per query + eliminates the clipboard round-trip entirely.** For an
  SSH-heavy engineer, **10–20 min/day** and a large drop in friction.

### 3.3 Watch-while-detached
- **Before:** kick a 40-min build/deploy/training run → either babysit it (dead time), or
  check back every few minutes (fragmented attention), or miss the failure and lose the
  40 min.
- **After:** `> watch` the pane, close the laptop; the daemon notices exit/quiet, summarizes,
  and has a verdict waiting on reattach — *even while detached*.
- **Save: the babysitting time + faster failure detection** (catch a min-12 failure at min
  12, not min 40). **Several 10s of minutes on a bad day; peace of mind every day.**

### 3.4 Reattach briefing
- **Before:** after lunch / a meeting / Monday, spend 2–5 min reconstructing "where was I" —
  scroll each pane, recall what was running, notice the failing test.
- **After:** `mars attach` → a dismissible 3-line diff, failures first, absent when nothing
  changed.
- **Save: ~2–4 min × every context switch (many/day) ≈ 10–20 min/day.**

### 3.5 Cross-pane correlation
- **Before:** "does the function on the left match the error on the right?" — eyeball both,
  or paste both into a chat with context.
- **After:** ask once; the model sees both panes.
- **Save: ~1–2 min per occurrence.**

**Bottom line:** for a terminal-heavy developer, the sight-and-persistence scenarios
plausibly return **~45–75 min/day** — and, more importantly, remove the context-switches
that fragment deep work. The wedge (triage) alone justifies adoption; the rest compounds it.

---

## 4. The primitives — engineering design proposals

Six platform investments sit under the scenarios. Each is a substrate multiple scenarios
load onto. Dependency-ordered.

### P1 — The Context Bus  *(partially shipped as `screen_context`)*
- **What:** one consented read-interface over registry + buffers/cursors + project index +
  terminal screens + recent-action trace.
- **Design:** a `ContextSource` registry — `trait ContextSource { fn id() -> &str; fn
  snapshot(&App, budget) -> String; fn default_consent() -> bool }`. `screen_context()`
  becomes the composition of enabled sources under a token budget, each source tagged in the
  prompt. New sources (git status, the failing test, the project index) register once and
  light up every consumer (agent, finder ranking, watchers, briefing).
- **Trade-offs:** always-send-everything (simple, blows the budget) vs. selectors
  (model-driven `NEED:` expansion — cheaper, one extra round-trip on demand). *Decision:
  selectors,* already specced in `workflows_design.md`.
- **Unlocks:** 1, 2, 5, 6, 7. This *is* the differentiation, made into an API.

### P2 — The Trigger / Watch framework  *(daemon-resident)*
- **What:** event-driven triggers (output-quiet, process-exit, detach, attach) feeding a
  pull-model notices queue.
- **Design:** ride the existing `TermEvent` stream + `frame_tick` clock — "output quiet for
  N s" is `frame_tick - last_output_tick > threshold`. A `WatchRegistry` (per-pane
  `watched` flag) → on trigger, one background summarize (the `auto_name` pattern, one
  global in-flight gate) → push one `Notice`. Render pulls from the queue as a single
  status line; the agent never pushes to the screen.
- **Trade-offs:** event-driven vs. polling (*decision: event-driven*, near-zero idle cost);
  passive one-liner vs. urgent interrupts (*decision: passive for v1* — the no-push-path
  structurally enforces the interruption budget, so proactivity can't become a firehose).
- **Unlocks:** 3, 4, 6 — all across-time proactivity. Its daemon-residence is the exact
  thing tmux and ephemeral CLIs cannot replicate.

### P3 — Parameterized actions / the plan layer
- **What:** a declarative arg schema per `Action`, so the agent emits `RUN: FindFile("x")`,
  macros bind partial applications, and multi-step effects batch into one confirmable plan.
- **Design:** each `Action` declares args `(name, type, completion-source, default)`;
  migrate `OPEN`/`INSERT` onto it; the trailing-directive parser gains a typed-arg form (the
  `AgentDirective` seam already isolates this). A plan = a short list of parameterized
  actions the user confirms once.
- **Trade-offs:** stringly-typed directives (shipped, portable, no validation) vs. a schema
  (validated, composable, more code). *Decision: build the schema now* — it's the vision
  doc's #1 blocker; nearly everything scriptable is gated on it.
- **Unlocks:** the agent as *operator* not commentator; macros; the finder; workspace
  orchestration.

### P4 — Session-as-artifact
- **What:** harden the daemon session into a durable, inspectable, shareable, multi-client
  object.
- **Design:** N-client mirroring (broadcast Output frames to all attached clients; merge
  input); OSC-52 clipboard so a remote attach can copy to the *local* clipboard; a session
  snapshot/restore across daemon restarts.
- **Trade-offs:** one-client-per-session (shipped, simple) vs. mirroring (enables pairing,
  more sync logic). *Decision: defer mirroring until a pairing/handoff scenario is on the
  roadmap; ship OSC-52 first* (cheap, unblocks remote copy).
- **Unlocks:** 2, 6, 9 (pairing/incident handoff), and agent-operated headless sessions you
  supervise remotely.

### P5 — The Transaction journal
- **What:** global, multi-buffer, grouped undo — one agent run = one labeled reversible chunk.
- **Design:** an editor-wide op journal; every multi-step effect (agent run, macro,
  project-wide replace) brackets its edits into one labeled entry; `undo` pops the whole
  entry.
- **Trade-offs:** per-buffer snapshot undo (shipped, cannot express cross-file revert) vs.
  the journal (more state, correct). *Decision: build it before autonomous multi-file
  editing* — reversibility, not permission dialogs, is what makes autonomy adoptable.
- **Unlocks:** the entire autonomy layer. Gated late because autonomous editing is
  deliberately not the near-term game.

### P6 — Project index  *(designed in `delighters_design.md`)*
- One index, two consumers (the human's finder + the agent's project awareness). The
  readiness test — *can both a pane and the agent read it?*

**Build order:** Context Bus → Trigger framework → Parameterized actions → Session-as-artifact
→ Transaction journal, with the Project index landing alongside the finder delighters. The
journal is last because it gates autonomous editing, which is not the near-term game.

---

## 5. The 10x wedge (unowned territory)

Reframe the category: everyone builds "an AI in your editor." Mars builds **an AI ops-mate
that lives in your session** — on the remote box, watching long/detached work, triaging
across every pane, briefing you on return, and eventually *operating* panes while you
supervise from a thin client (even your phone).

To follow, **all four competitors would have to rebuild their foundations** — Cursor grows a
server-side daemon and moves its AI off the client; Claude Code owns the whole workspace not
one pane; tmux becomes an AI; Warp adds remote-first persistence + an editor + a
daemon-resident agent. That's a moat, not a lead.

**Sequence to the wedge:** Context Bus (sight, ~shipped) → Trigger framework (turns sight
into vigilance) → Session-as-artifact (supervise/hand off from anywhere) → Parameterized
actions (commentator → operator) → Transaction journal (safe to trust). Endgame: **agent-
operated headless sessions you supervise remotely** — dispatch a task, close the laptop, the
agent works in a pane inside a persistent session, you check in from your phone and approve
or revert one chunk.

---

## 6. Anti-scenarios — what Mars must refuse

- **Ghost-text completion.** Commodity, latency-hostile on free-tier keys, months inside
  someone else's moat, and noise in a calm editor. Every hour here is an hour not widening
  the sight-and-persistence lead.
- **A context-free chat pane.** Strictly worse than the ChatGPT tab the user already has;
  shipping it concedes that line-of-sight is the point. **Every agent surface is
  workspace-aware or it doesn't exist.**
- **Head-on multi-file code editing.** That's Cursor/Claude-Code's moat; fighting there
  dilutes identity and loses on their turf. The winning posture is the *inversion*: let
  Claude Code run *inside a Mars pane*, and be the workspace that gives it sight of the other
  panes. **Be the substrate the code-agents run in, not a worse copy of them.**
- **A full GUI IDE.** Throws away the remote/SSH/daemon architecture that makes scenarios
  2, 3, 6 unwinnable for anyone else.

---

## 7. Final recommendation

**Own one scenario in 12 months: failure triage across your whole workspace — "your
terminal errored, press one key."** It is the highest-frequency, highest-emotion moment; it
is structurally impossible for every incumbent when the failure lives in an adjacent pane or
on a remote box; and it is the on-ramp that pulls shells *into* Mars — which widens the
agent's field of view, which improves every answer. That is the flywheel.

> **Positioning:** *Mars is the terminal where the AI can see your whole workspace — so when
> something breaks, one key tells you why, with the fix ready to run.*

**Concretely, build in this order:**
1. **Now (shipped/near):** triage (`C-t ?`, done), sessions-by-default (done), the Context
   Bus formalized, the navigation delighters.
2. **Next — the moat-widener:** the **Trigger/Watch framework** → watch-while-detached +
   reattach briefing. This is the single highest-leverage unbuilt primitive: it turns the
   shipped *sight* into across-time *vigilance*, and it's the capability no competitor can
   follow without a daemon.
3. **Then:** parameterized actions → session-as-artifact (OSC-52, then mirroring) → the
   transaction journal, in step, each unlocking the next tier of the ops-mate.

Win the daily one-key rescue first — it's the Trojan horse that earns the right to build the
ops-mate, and the ops-mate is the thing no competitor can follow.
