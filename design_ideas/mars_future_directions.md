# Mars — Future Directions: What Should This Thing Become?

*A first-principles strategy study of how to make Mars maximally valuable — and an honest
evaluation of the proposed reframe ("MARS = Mission-Aware Rust Shell") against four
alternatives. Sibling to [`strategy.md`](./strategy.md) (the sight × persistence thesis and
the six primitives), [`memory_ideas.md`](./memory_ideas.md) /
[`memory_design_alternatives.md`](./memory_design_alternatives.md) (the third axis), and
[`ssh_strategy.md`](./ssh_strategy.md) (the key-never-leaves-home broker). Those docs answer
"what do we build?"; this one answers "what is the thing FOR?" — the framing question that
decides which builds matter. Decisions are made, not enumerated; weaknesses are named, not
softened.*

**The assets on the table** (the raw material every framing must account for):

| # | Asset | Status |
|---|---|---|
| A1 | **The daemon** — sessions outlive the client; `tick()` runs while detached | shipped (v0.1.0 on crates.io) |
| A2 | **Line of sight** — the agent reads every pane: editor, terminals, scrollback, layout | shipped |
| A3 | **The event/watch framework** — watches summarize long runs even while detached; the Away Digest event log | shipped / in progress |
| A4 | **Empirical memory** — signature-keyed, outcome-derived, self-correcting | designed (Alt A in `memory_design_alternatives.md`) |
| A5 | **The SSH broker** — key + memory never leave home, proxied over the tunnel | designed |

A framing is a claim about which of these is the *product* and which are *plumbing*. The
wrong framing leaves assets as orphaned features; the right one makes each asset a chapter
of one story.

---

## 1. First principles — where does terminal value actually come from?

Start from zero. Ignore the codebase, ignore the roadmap. What makes a terminal-shaped tool
valuable *in the agentic era specifically*?

### 1.1 The human's three scarce resources

Compute got cheap and parallel; models got competent and tireless. What did *not* change:

1. **Attention is serial and interruptible.** A human attends to one thing at a time, and
   every context switch costs 2–5 minutes of reconstruction (`strategy.md` §3 priced this).
   In a world of one workstream this was a tax; in a world where an engineer can *cause*
   five concurrent workstreams (three agents, a training run, a deploy), attention becomes
   the binding constraint on how much of that parallelism they can actually harvest.
2. **Working memory holds ~4 chunks** (Cowan; `key_design.md` §1.1). The live state of N
   concurrent efforts — what's running, what finished, what's blocked, what it means — does
   not fit in a head. It must live *in the world* or it is lost.
3. **Intent is the one thing only the human has.** Everything else — typing, watching,
   recalling, correlating — is now delegable. The residual human job is deciding *what
   should be true* and *whether it is yet*. Every minute spent on the delegable parts is
   the era's definition of waste.

### 1.2 The terminal's three unique positions

Why is a *terminal-shaped* tool — not an IDE, not a chat app, not a dashboard — the right
place to return those resources? Because the terminal alone holds three positions:

1. **Presence.** The terminal is where work *executes* — commands, exit codes, processes,
   logs. Ground truth flows through it natively; every other tool must be told what
   happened, secondhand. A chat app knows what you pasted; the terminal knows what *ran*.
2. **Sight.** A terminal that owns the whole workspace (Mars, uniquely) can observe every
   actor — the human's commands, each agent's output, each build's progress — as
   co-present structured data. No incumbent has this: agent CLIs see their own pane;
   editors see files; tmux sees bytes it can't interpret.
3. **Persistence.** A daemon outlives attention. It is *present when the human is not* —
   the only position from which "what happened while you were gone" can even be answered.

**The value theorem this study uses:** *in the agentic era, a terminal's value is the
amount of scarce human resource (attention, working memory, intent-time) it returns, per
unit of trust it consumes.* The trust denominator is load-bearing: a tool that acts
autonomously but wrongly, or remembers confidently but stale, returns negative value.
Every framing below must answer both halves — what it gives back, and how it earns the
right to.

### 1.3 The evaluation lens

Five axes, applied identically to the proposed framing and every alternative:

| Axis | The question |
|---|---|
| **Ownability** | Could an incumbent (Warp, Claude Code, tmux/zellij, Cursor, atuin) copy it without rebuilding their foundations? Architecture-level moats score high; feature-level ideas score low. |
| **Frequency** | How many times per day does the framed value *fire* for a working engineer? |
| **Depth of pain** | When it fires, how much did the moment hurt without it? (A daily papercut and a monthly catastrophe both matter; a monthly papercut does not.) |
| **Coherence** | Does the framing give all five assets (A1–A5) a job in one story, or does it orphan some as "also, features"? |
| **Brand fit** | Does it fit "Mars — mission control for your terminal," the local-first/Rust identity, and the author's agentic-memory expertise? |

One meta-rule, learned from `strategy.md` §2: **invest where the architecture is the moat.**
A framing whose killer demo could be shipped by Warp as a feature flag next quarter is a
marketing coat, not a strategy.

---

## 2. The proposed framing — MARS = Mission-Aware Rust Shell

### 2.1 The steelman, from first principles

The shell is the oldest intent-blind tool in computing. It has executed trillions of
commands and never once known *why*. `cargo build` is identical bytes whether it's a
smoke-test of a one-line fix or the final gate on a week-long refactor — and so every tool
downstream of the shell (history, prompts, multiplexers) is intent-blind too. The gap
between *commands* and *goals* is the terminal's original sin.

Now notice: **every asset Mars has built is secretly about that gap.**

- A **watch** (A3) is a fragment of a *done-criterion* — "tell me when this run reaches a
  verdict" is "tell me whether the mission advanced."
- The **Away Digest** (A3) is a progress report with the "toward what?" missing — it lists
  events; a mission would rank them by relevance to the goal.
- **Empirical memory** (A4) records what fixed what — which is exactly "what has advanced
  or unblocked missions of this shape before."
- **Sight** (A2) gathers context — but context selection is a relevance problem, and
  relevance is distance-to-*mission*, not recency or proximity on screen.
- The **broker** (A5) carries identity across hosts — and a mission is precisely the unit
  of work that spans hosts (the incident that walks you across five boxes).

Make the **mission** first-class — a small object: `{goal, done-criterion, current state,
blockers, artifacts}` — and everything reinterprets:

| Existing mechanism | Reinterpreted under mission-awareness |
|---|---|
| Context selection (`screen_context`, `NEED:`) | inject what's *relevant to the mission*, not what's visible |
| Watch verdicts | evaluated *against the done-criterion* ("this failure blocks the mission" vs "unrelated noise") |
| Away Digest | a **mission progress report**: "make CI green — advanced: repro found · blocked: staging creds" |
| Memory keys | facts scoped to mission-shapes: "last time this mission-shape stalled here, X unblocked it" |
| Triage (`C-t ?`) | "what does this failure *mean for what you're doing*," not just "what is this error" |
| Reattach / handoff | the mission state is the transferable unit — to your phone, to tomorrow-you, to a colleague |

### 2.2 Extracting maximum value: what it implies concretely

- **Ship first: silent inference, one-line surface.** The daemon already logs the event
  stream (Away Digest). A cheap periodic classification distills it into a one-line mission
  header — `mission: fix flaky test_auth · 2 advanced · 1 blocked` — shown only in the
  digest and the reattach briefing, correctable with one word (`? mission is actually the
  deploy`), never asked for. No declaration ceremony ever.
- **Mission-ranked digest.** The Away Digest's hardest problem is ranking (what matters
  among 40 events?). "Distance to mission" is the principled ranking function the digest
  otherwise lacks.
- **Mission-keyed memory.** A4's signature-keyed store gains a second key axis: the
  mission-shape. "CUDA OOM" as a fact is good; "CUDA OOM *while running sweeps*, fixed by
  expandable_segments, 3 observations" is better retrieval.
- **Mission handoff.** `mars attach` from the phone answers "where is the mission?" in
  three lines — the A1+A5 combination cashed out as a sentence, not a screen.
- **The anti-vision (non-negotiable).** Mars is never a nagging PM bot. No standup
  prompts, no forms, no "what are you working on today?", no required declaration. Missions
  are *inferred* silently, corrected with one word, and *used* only at high confidence — a
  wrong mission header shown once costs more trust than ten right ones earn (the same
  first-false-alarm economics `agentic_inline.md` established for watches). Below the
  confidence bar, Mars behaves exactly as it does today, and that must always be a good
  product.

### 2.3 The honest critique

Steelmanned, it's genuinely attractive. Now the weaknesses, none of them hand-waved:

1. **Mission inference is the hardest inference problem in the building.** Intent is
   latent, interleaved (a real session braids three missions and some puttering),
   hierarchical (missions contain sub-missions), and often absent (exploration has no
   done-criterion). Worse, inference errors *compound downstream*: a mis-inferred mission
   mis-ranks the digest, mis-keys the memory, and mis-selects context — the framing wires
   its least reliable component into everything else's input.
2. **Users will not declare, and inference has no training data yet.** Engineers who won't
   update Jira will not maintain mission objects. Inference-only is the right call — but
   the classifier needs exactly the event-log + memory substrate (A3, A4) that doesn't
   fully exist yet. **Mission-awareness consumes the other assets as inputs; it cannot
   lead them.**
3. **Ownability is thin at the feature level.** "Goals" as a text field is copyable by
   Warp in a sprint; Claude Code already tracks a task list inside a session. What's
   ownable is the *substrate* — presence to observe real outcomes across panes and days —
   and that substrate is better named by other framings. The mission layer itself is a
   semantic coat anyone can claim to wear.
4. **The Aware-vs-Assist trust gap.** Awareness that doesn't yield materially better
   assistance reads as surveillance. The demo of "mission-aware" is… a header. The demo of
   "it triaged your failure" is a rescue. Users grant awareness *after* competence, not
   before.
5. **The frequency is conditional.** If inference works, the value fires constantly; if it
   works 70% of the time, the product is a coin-flip header users learn to ignore. There
   is no graceful middle.
6. **Backronym-driven design.** Candidly: "Mission-Aware Rust Shell" is reverse-engineered
   from the letters M-A-R-S. That's charming, but a strategy derived from a name is
   exactly backwards — and the existing tagline "mission control for your terminal"
   already owns the word *mission* while pointing somewhere subtly better (see §4.3).

**Lens score:** Ownability **medium** (substrate ownable, layer copyable) · Frequency
**high-if-it-works, conditional** · Depth of pain **medium** (re-orientation pain is
already served by the digest without the mission layer) · Coherence **high** (it does
unify all five assets) · Brand fit **high** (the word is in the name).

**Verdict, previewed:** the right *v2 semantic layer*, the wrong *lead*. It is what the
digest and memory substrate should grow into once they exist and have accumulated the data
that makes inference honest — not the banner to march under while building them.

---

## 3. Four alternative reframes

Chosen for strength and distinctness; each argued from first principles, grounded in
personas (SRE / MLE / applied scientist), mapped to the assets, scored, and critiqued.

---

### 3.1 The Delegation Surface — "mission control for your agents"

**One-liner:** *The place you dispatch and supervise a fleet of workers — coding agents,
scripts, long jobs — with sight of all of them, verdicts instead of babysitting, and a
shift report when you return.*

**First-principles argument.** §1.1's constraint — serial attention, parallel work — has a
name in every other discipline: *supervision*. When workers became cheap (industrial
labor, cloud instances, now agents), the scarce role shifted to the supervisor, and a
dedicated supervisory layer always emerged (the foreman, the control room, Kubernetes).
Agents are having that moment now: a working engineer in 2026 routinely has Claude Code in
one pane, a test loop in another, a training run in a third — and supervises this fleet by
*alt-tabbing and squinting*, i.e., with no tooling at all. The terminal is where the
workers already live. A supervisor needs exactly three things no incumbent has together:
**sight of all workers at once** (A2 — an agent CLI structurally cannot see its sibling
panes), **presence while the supervisor is away** (A1+A3 — Claude Code's work dies or
dangles when the laptop closes; Mars's daemon keeps watching), and **judgment accumulated
across shifts** (A4 — which approach failed twice before). Panes are workers; watches are
supervision policies; the Away Digest is the shift report; the broker (A5) is fleet-wide
identity. **Every asset has a job, and the job is the same story.**

**Killer stories.**

- *MLE, 2pm.* She dispatches three Claude Code agents in three panes — fix the flaky
  test, upgrade the dependency, draft the benchmark — and kicks off a training run in a
  fourth, watches on all four (`C-t w`). She goes to a two-hour meeting *with the laptop
  closed*. On reattach: `while away — ✗ training crashed step 40k: OOM · ✓ flaky-test
  agent done, diff ready · ⏸ dep-upgrade agent waiting on your answer · benchmark still
  running`. Failures first, one screen, fifteen seconds to re-acquire command of four
  workstreams. Today this is four terminal windows and dread.
- *SRE, 2:17am.* The incident spans the LB, two app boxes, and the DB primary. `mars ssh`
  onto each (A5 — no key ever lands on the jump host); each remediation job gets a watch;
  the war-room is four panes with verdicts surfacing as they conclude, and the agent —
  sighted on *all* panes — answers "what have we ruled out?" She is supervising the
  incident instead of being its clipboard.
- *Applied scientist.* Eight sweep configs, eight panes, one watch policy: summarize each
  run's verdict as it exits. He returns to a ranked digest — two diverged early, five
  completed, one hit NaN at step 3k with the offending config named — instead of eight
  scrollbacks to spelunk.

**Architectural mapping:** A1 = workers persist across the supervisor's absence · A2 = the
supervisor's sight · A3 = the supervision policies + the shift report · A4 = supervisory
judgment ("this worker's approach failed before") · A5 = one identity across the whole
fleet's hosts. **Five for five — the only framing with no orphans and no strain.**

**Lens score:** Ownability **high** — supervision requires cross-pane sight plus a
daemon, which is precisely the architecture (`strategy.md` §5) incumbents must rebuild
foundations to get; crucially, no agent *vendor* can be the neutral supervisor of all
agents. Frequency **high and rising** — fires every multi-workstream session, the fastest-
growing behavior in the field. Depth of pain **high** — supervision bandwidth is the
emerging bottleneck, and today's tooling for it is literally nothing. Coherence
**maximal**. Brand fit **maximal** — "mission control" (NASA sense) *means* humans
supervising autonomous systems operating far away; the existing tagline was this framing
all along.

**Honest weaknesses.** (1) *The behavior is early-majority, not yet universal* — fleets of
agents are a power-user pattern in mid-2026; leading here bets the pattern generalizes
(the trend line strongly says it does, but it's a bet). (2) *Platform absorption risk* —
Anthropic and OpenAI are building native orchestration (subagents, background tasks,
cloud workers); the mitigation is structural neutrality: Mars supervises *all* workers —
Claude Code, codex, aider, cron jobs, bare scripts — which no single agent vendor will
ever do, and owns the layer they can't: the workspace that outlives them. (3) *Deep
supervision of third-party agents is shallow at first* — Mars reads their output like any
pane (robust, via LLM summarization) but can't introspect their internal state; richer
integration (agent status protocols) is speculative. (4) *Trust ceiling without the
transaction journal* — supervising is safe today (watches only observe); the moment Mars
*acts* on workers (restart, kill, approve), it needs the P5 journal and the destructive-
action gates, already doctrine.

---

### 3.2 The Second Memory — "the terminal that knows you"

**One-liner:** *Every session makes it smarter: a terminal whose empirical, self-correcting
memory of what actually worked compounds until leaving it means leaving your own
experience behind.*

**First-principles argument.** Almost everything a terminal does is a stateless service —
worth the same on day 1000 as day 1. The only property whose value *increases
monotonically with use* is accumulation (`memory_ideas.md` §1), and the daemon is the only
process present for everything worth accumulating: every command, exit code, fix, and
verdict — including the ones that happen while detached. The differentiated bet is
*empirical* memory (Alt A: signature-keyed, outcome-derived, edge-weights grown by
repetition and decayed by contradiction) versus everyone else's *prescriptive* memory
(CLAUDE.md files humans write and forget to update). Prescriptive memory says what someone
once claimed; empirical memory says what has been *observed to work, N times, most
recently Tuesday*. And the moat is a flywheel with a cold-start wall: a competitor can
bolt an embedding store onto a chat pane tomorrow, but they have nothing to embed — no
daemon was present for the user's last six months.

**Killer stories.**

- *MLE.* CUDA OOM at step 40k. The agent's answer opens: "You've hit this 3 times; twice
  `expandable_segments:True` resolved it (last: June 20). Apply?" — not from a doc, from
  counted observation. Confidence stated because the edge weight is real.
- *SRE.* "Have we seen this alert before?" — episodic recall: the same p99 alert three
  weeks ago, root cause a stale advisory lock, fix `make unlock-migrations`, plus what was
  *ruled out* then. The 2am twenty-minute re-derivation becomes a ten-second recall.
- *Any engineer, any fresh box.* `mars ssh new-host` and the agent already knows the
  deploy command, the flaky test, the `-j1` workaround — memory retrieved from home over
  the broker (A5), never stored on the box. "It knows me *everywhere*" is the story no
  per-machine tool can tell.

**Architectural mapping:** A1 = present for everything worth remembering · A2 = what
feeds it · A3 = pre-computed episodic facts (watch verdicts are memories the moment
they're logged) · A4 = *the product itself* · A5 = memory made portable. Coherent — but
note the strain: sight and watches become *tributaries* of memory rather than stars, and
the pane/workspace assets read as delivery mechanism.

**Lens score:** Ownability **highest over time** (the flywheel; cold-start wall) but
**lowest on day one** (an empty store is indistinguishable from no store). Frequency
**high but diffuse** — it improves every answer a little rather than any moment a lot.
Depth of pain **medium** — re-derivation is a thousand papercuts, not one scream; real
but hard to feel as a single moment. Coherence **high with strain**. Brand fit —
**maximal for the author** (the agentic-memory-expert brand makes this the framing the
founder can argue better than anyone), **weaker for the category**: "memory" as a word has
been commoditized by ChatGPT/Claude memory features into meaning "it remembers my name."

**Honest weaknesses.** (1) *Undemoable on day one* — "it gets better in three weeks" is a
terrible pitch; every rival demo fires in thirty seconds. (2) *Trust-fragile* — one
confidently-wrong recall (`memory_ideas.md` §5's own top risk) damages the entire brand
promise, because the brand IS the memory. (3) *Word dilution* — competing for a term
incumbents have already defined down. (4) *Privacy is the identity's own edge* — a product
whose pitch is "it watches and remembers everything in your terminal" must win the trust
argument before the value argument, a hard ordering. The local-first/broker stance
(A5, `ssh_strategy.md`) is the strongest possible answer, but the question gets asked
first either way.

---

### 3.3 The Autonomous Workspace — "work continues while you're gone"

**One-liner:** *You leave; the workspace doesn't stop. Runs are watched to a verdict,
queued steps fire on success, agents finish their tasks — you return to results, not to
where you left off.*

**First-principles argument.** Human attention is serial and clock-bound; work is
parallel and continuous. The gap between them — nights, meetings, lunches, the walk to
the coffee machine — is dead calendar time that a persistent, *judging* daemon can convert
into progress. tmux proved half of this: processes surviving the window is so valuable
that a 40-year-old protocol multiplexer is still universal. But tmux's daemon is inert —
it keeps bytes flowing and understands none of them. A1+A3 make the daemon *sentient
enough to conclude*: a watch reaches a verdict at minute 12, not when you check at minute
40; a queued follow-up ("if the build passes, start the eval") fires at 2am; the digest
compresses the elapsed time into three lines. The colleague test: a tool waits for you;
a colleague makes progress without you.

**Killer stories.**

- *MLE, overnight.* Training launched at 6pm with a watch and one queued step: "on clean
  exit, run the eval suite." At 9am the digest reads: training done 2:14am · eval ran ·
  two regressions, names cited. The night worked.
- *SRE, Friday 5:50pm.* The slow-rollout deploy gets a watch with a failure criterion.
  She leaves. At 6:40 the daemon catches the canary error-rate line in the log, concludes
  "failed," and the verdict is waiting on her phone (A5 thin-attach) — she rolls back at
  6:45 instead of discovering it Monday.
- *Applied scientist.* The 3-hour data pipeline's watch doesn't just report exit — it
  catches the schema-drift warning at minute 20 that would have silently poisoned the
  downstream job, and the queued step never fires. The save isn't time; it's the
  three-hour *wrong* run that didn't happen.

**Architectural mapping:** A1 = *the star* · A3 = its senses and judgment · A2 =
instrumental (what the daemon reads) · A4 = supporting (better verdicts over time) ·
A5 = check-in-from-anywhere. Coherent but top-heavy on two assets.

**Lens score:** Ownability **high** — structurally impossible for every window-bound
incumbent (`strategy.md` scenario #3's argument). Frequency **medium** — detach windows
number a few per day, not dozens. Depth of pain **high when it fires** (the lost 40-minute
run; the Monday-discovered failure). Coherence **medium-high**. Brand fit **good**
(mission control watches the spacecraft around the clock).

**Honest weaknesses.** (1) *It is the passive half of §3.1* — "work continues while
you're gone" is what a Delegation Surface does during the supervisor's absence; as a
standalone frame it's a subset with a smaller surface. (2) *v1 honesty gap* — today the
daemon watches and concludes; it does not yet *do* (queued steps are unbuilt, agent panes
work but act ungated); the frame overpromises its own tense. (3) *Autonomy's trust
denominator* — unattended action without the transaction journal (P5) risks the one
horror story ("it ran something at 2am") that defines a small product forever. The gates
exist in doctrine; the frame pushes hardest against them.

---

### 3.4 The Flight Recorder — "total recall of your work"

**One-liner:** *Everything observable in your workspace is recorded, searchable, and
replayable — so provenance ("what exact commands produced this artifact?"), postmortems,
and 'how did I do this last time?' become queries instead of archaeology.*

**First-principles argument.** The terminal is the only place where actions and outcomes
co-occur at full fidelity — and today that record *evaporates* (scrollback dies with the
pane; history stores commands but not outputs, outcomes, or order-across-panes).
Recording is nearly free at write time; reconstruction after the fact is unboundedly
expensive (the worst hours of any postmortem are timeline reconstruction; the worst weeks
of any ML project are "which run produced this checkpoint?"). Asymmetry that steep is
usually a product. Only a present-and-persistent daemon (A1) with sight (A2) can capture
it — atuin syncs commands without outcomes; asciinema replays one pane's pixels without
semantics; neither can answer a *question*.

**Killer stories.**

- *Applied scientist.* "Which exact command, env, and git SHA produced `model_v3.ckpt`?"
  — answered from the record in seconds, six weeks later, when the paper reviewer asks.
- *SRE postmortem.* The incident timeline — every command on every box, interleaved with
  the alerts, in true order — assembles itself from the recorded sessions. The doc that
  took an afternoon takes twenty minutes.
- *Any engineer.* "I fixed exactly this LaTeX/ffmpeg/openssl incantation eight months
  ago" — replayed, not re-derived.

**Architectural mapping:** A1+A2 = the recorder · A3 = event annotations on the tape ·
A4 = *the distillation of the tape* (memory is the recorder's index) · A5 = the record
stays home. Note what's missing: the *agent as actor* has no role — the frame is about
observation, and the delegation/autonomy assets sit idle.

**Lens score:** Ownability **high on capture** (presence required), **low on concept**
(partial substitutes exist and "recording" is easy to claim). Frequency **low-medium** —
queried episodically (weekly postmortems, monthly provenance), not hourly. Depth of pain
**deep but rare** — a flight recorder is valuable only when something crashed. Coherence
**medium-low** (agentic assets orphaned). Brand fit **weak** — "everything is recorded"
leads with the surveillance connotation the local-first stance then has to argue back
from; the trust denominator is charged before value is delivered.

**Honest weaknesses.** (1) *It's a substrate, not a product* — the event log absolutely
should exist (it's being built as the Away Digest's spine, and memory consumes it), but
substrates make poor banners. (2) *Storage and redaction are the sharpest edges in the
whole portfolio* (secrets in scrollback, retention policy, disk growth) taken on for the
least frequent value. (3) *The best parts are reachable through other frames* — provenance
and episodic recall are exactly what the Second Memory serves, with distillation instead
of raw tape.

---

## 4. Synthesis and recommendation

### 4.1 The comparison

| Framing | Ownability | Frequency | Depth of pain | Coherence (A1–A5) | Brand fit | Fatal flaw if led with |
|---|---|---|---|---|---|---|
| **Mission-Aware Shell** | med (layer copyable, substrate not) | conditional-high | med | **high** | high (the name) | leads with its least reliable component; consumes substrates that don't exist yet |
| **Delegation Surface** | **high** (architecture) | **high, rising** | **high** | **maximal (5/5)** | **maximal** ("mission control" literal) | bets on fleet behavior generalizing; platform absorption |
| **Second Memory** | **highest over time**, lowest day-1 | high but diffuse | med | high (with strain) | maximal for author, diluted word | undemoable day-1; trust-fragile |
| **Autonomous Workspace** | high | med | high per-event | med-high | good | subset of Delegation; autonomy before the journal |
| **Flight Recorder** | high capture, low concept | low-med | deep, rare | med-low | weak | substrate mistaken for product; privacy-first impression |

### 4.2 They are not exclusive — they are a stack

The decisive observation: these framings name *layers of one architecture*, in strict
dependency order. Each consumes the one below it:

```
interpret   MISSION-AWARENESS      what does it all mean for the goal?   (v2 — needs everything below)
supervise   DELEGATION SURFACE     dispatch, watch, digest, command      (the lead — assets are ready)
compound    EMPIRICAL MEMORY       distill the record into judgment      (the arc — flywheel, moat)
record      EVENT LOG / RECORDER   capture what happened                 (substrate — being built as Away Digest)
be there    DAEMON + SIGHT         persist and see                       (shipped — v0.1.0)
```

This resolves the study's question cleanly. Mission-awareness is not wrong — it is
*premature as a banner and correct as a destination*: it is what the digest and memory
layers grow into once they exist and have accumulated the observational data that makes
mission inference honest rather than coin-flip. Leading with it would sequence the
hardest inference problem first, before its own inputs. The Flight Recorder is not wrong
either — it is the substrate two layers up depend on, and it should be built exactly as
planned (the Away Digest event log) and never marketed as recording. And the Autonomous
Workspace is the Delegation Surface's night shift — one frame, not two.

### 4.3 The recommendation (one, argued)

**Lead the next 6 months with the Delegation Surface, under the sharpened tagline:
"Mars — mission control for your agents."**

The argument, compressed: it is the only framing that scores at the top of *every* axis
simultaneously — architecture-moated (cross-pane sight + detached daemon is precisely
what `strategy.md` §5 shows incumbents cannot retrofit), high-frequency (every
multi-workstream session, the fastest-growing behavior in the field), deep-pain (the
supervision bottleneck currently has *zero* tooling — the competition is alt-tab),
five-for-five coherent (every existing asset has a named job in one story), and
brand-perfect — "mission control," which Mars has said since the rebrand, *means* humans
supervising autonomous systems operating at a distance. The reframe was hiding in the
tagline all along. And unlike Mission-Aware, it is demoable in thirty seconds on day one:
dispatch three agents, close the laptop, come back to a shift report.

Concretely, the 6-month build is mostly the existing roadmap, re-aimed: agent-pane
ergonomics (dispatch N workers, fleet status at a glance), watch-as-supervision-policy
(per-pane done/failure criteria, failures first), the **Away Digest as shift report**
(finish it — it is the lead feature's centerpiece, not a side utility), and the broker
for fleet identity across hosts. Stay agent-agnostic on principle: Mars supervises Claude
Code, codex, scripts, and cron alike — neutrality is the defense against platform
absorption, because no agent vendor will ever be the trusted supervisor of its rivals.

**Keep Empirical Memory as the deeper arc** — the compounding moat and the founder's
signature. Ship it *quietly inside* the delegation story rather than as its own banner:
memory is what makes the supervisor senior ("this worker's approach failed twice
before"; "last time this alert fired, here's what unblocked it"). This ordering fixes
memory's two lead-with weaknesses at once: the delegation surface gives memory a daily,
demoable delivery vehicle, and every supervised session feeds the flywheel so that by the
time memory *is* the headline ("the terminal that knows you"), the claim is a year of
observations deep and no competitor can cold-start it. The author's agentic-memory
expertise is the moat's *depth*; delegation is its *door*.

**Hold Mission-Awareness as the v2 semantic layer, inferred, never declared.** When the
digest ranks events and memory keys facts, both will be asking the same latent question —
"relevant to *what*?" — and the mission object is the answer. Build it then, on real
data, behind the confidence gate, per the §2.2 anti-vision. Do **not** rename to
"Mission-Aware Rust Shell": the backronym buys nothing the tagline doesn't already own,
loses the word *control* (which names the actual product), and commits the brand to the
layer that ships last.

**What not to pursue:** the Flight Recorder as identity (build the log as substrate;
never ship a "recording" pitch or a replay-first UI); a mission declarator or any PM-bot
surface (violates the anti-vision, and dead on arrival with the persona); autonomy
beyond observation before the transaction journal lands (verdicts and queued *proposals*
yes; ungated unattended action no); and — unchanged from `strategy.md` §6 — ghost-text,
context-free chat, and head-on code editing, which remain other people's moats.

One sentence to remember the whole study by:

> **Mars is mission control: today, for your agents; underneath, a memory that compounds;
> eventually, aware of the mission itself — in that order, because that is the order the
> architecture can keep its promises in.**
