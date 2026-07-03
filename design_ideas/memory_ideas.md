# Mars — Memory: the third axis

*A first-principles study of augmenting Mars with MEMORY. People-first (the experiences we
want to create), then the technical substrate (how the agent reads and writes memory
efficiently — frecency, keyword retrieval, then RAG). Grounded in the code as it stands:
every proposal cites a real `file:function`, extends a real struct/enum, and lands on the
`screen_context` / `tick` / directive seams that already exist. Decisions are made, not
enumerated — sibling to [`workflows_eng.md`](./workflows_eng.md) and
[`strategy.md`](./strategy.md).*

Reading order for an implementer: this doc → `app.rs:screen_context` (the Context Bus v0) →
`app.rs:tick` (the event clock every proactive feature hangs off) → `agent.rs:parse_directive`
(the operator seam) → the phase you're building.

---

## 1. The thesis — sight × persistence × **memory**

`strategy.md` §1 names two load-bearing properties no competitor has together: **line of
sight** (the agent reads every pane) and **persistence** (the workspace lives in a daemon
that outlives the client). Memory is the **third axis, and it is the one that compounds the
other two over time.**

- Sight is *spatial*: what's on the screen right now.
- Persistence is *durational*: the state survives the weekend.
- **Memory is *temporal-semantic*: what has been true across every session, distilled into
  facts the agent can recall.** Sight without memory re-derives your build command every
  morning. Persistence without memory keeps your panes but forgets *why* they're arranged
  that way, *what* the flaky test was, *which* file you always reach for.

The moat argument is the same shape as §1's and it is *stronger*, because memory is the one
property whose value **increases monotonically with use**. A competitor could bolt an
embedding store onto a chat pane tomorrow — but they'd have nothing to embed. Mars's memory
is fed by a daemon that *already* sees every pane (`app.rs:screen_context`), watches long
runs while you're detached (`app.rs:maybe_fire_watches`), and persists a frecency substrate
across sessions (`app.rs:PersistedState`). **The daemon is the natural home of live memory
because it is the only process that is present for everything worth remembering.** tmux has
the persistence but no semantics; Cursor/Claude-Code have the semantics but die with the
window. Only Mars is present *and* smart *and* durable at once.

Stated as a sentence to remember it by:

> **Sight lets the agent see what you see. Persistence lets the workspace survive your
> absence. Memory lets the agent remember what it learned — so every session starts warmer
> than the last, and the tool that knows you best is the one you can't leave.**

---

## 2. People-first — the experiences

Taste starts with what memory *feels* like, not what it stores. Three kinds of memory,
distinguished by **who forms them and who they serve**:

| Kind | Formed by | Cost to user | The feeling | Substrate today |
|---|---|---|---|---|
| **Adaptive** (common-sense) | implicit, earned from behavior | **zero** — no save | "it just knows" | `frecency`, `bar_uses`, `file_frecency` (`app.rs:131/152/172`) — *already persisted* |
| **Agentic/explicit** (facts) | the agent or user records a fact | one deliberate act | "I told it once, it never forgot" | none yet — the `REMEMBER:` directive below |
| **Episodic** (what happened) | logged automatically from events | zero | "it remembers the story" | `notices`, `watches[].verdict`, `agent_history` (`app.rs:206/204/212`) — ephemeral today |

### 2.1 Adaptive memory — "it just knows" (the cheapest delight, already half-built)

The substrate exists and is *already persisted to disk*: `PersistedState` (`app.rs:3757`)
serializes `frecency`, `bar_uses`, and `file_frecency` to `~/.config/mars/state.json` via
`config.rs:state_path`. Doctrine already blesses this: `key_design.md` §1.7 —
*"Frequency × recency is how memory prioritizes… every ranked surface uses the same persisted
frecency substrate."* Adaptive memory is **frecency, generalized from a ranking signal into a
recall signal.**

> **Before.** Priya opens Mars in her repo. She types `cargo build` for the 400th time. She
> reaches for `src/app.rs` — the file she edits every day — and it's the eleventh result in
> the finder because the finder ranks alphabetically-then-frecency and today it's cold.
>
> **After.** The bar's empty state already surfaces `src/app.rs` at the top (file frecency),
> and `?` knows — without being told — that "the build" means `cargo build`, because Mars has
> watched her run it 400 times. When she asks *"why did the build fail?"*, the agent's context
> header already contains `usual build command: cargo build` and `frequently-edited: src/app.rs,
> src/agent.rs`. **She never taught it. It earned the knowledge by watching.**

What adaptive memory learns, all from signals already flowing through `tick`:
- **Your build/test/lint commands** — the shell commands you run most (a new `cmd_frecency`
  counter, fed where `run_shell_command` fires and from `TermEvent` output). This is the
  single highest-value adaptive fact: it lets the agent's `TYPE:` directives be *your*
  commands, not generic guesses.
- **Your reach-for files and actions** — already have `file_frecency` and `frecency`.
- **Your correction patterns** — when you edit the agent's proposed `TYPE:` command before
  running it, or reject a directive, that's a labeled correction (§4.4).

The magic: **no explicit save, no "memory" UI, no nag.** It is `strategy.md`'s frecency
principle extended one hop. This ships almost for free (§6, Phase 1).

### 2.2 Agentic/explicit memory — "I told it once" (the CLAUDE.md model, for the workspace)

This is the direct analog of the `.claude/memory/` model this very repo runs on — but for the
*end user's* workspace, formed in the flow of work rather than in a config file. A fact the
user or the agent decides is worth keeping.

> **Before.** Sam figures out, after twenty minutes of digging, that this repo deploys with
> `make ship` (not `make deploy`, which is a decoy target that no longer works). Next Tuesday
> he's forgotten, runs `make deploy`, waits four minutes, and it fails silently. He digs again.
>
> **After.** The moment he discovers it, he types `? remember this repo deploys with make ship,
> not make deploy`. The agent emits a `REMEMBER:` directive (§4.2); Mars stores it scoped to the
> project. Six days later he asks *"how do I ship?"* and the answer is instant and grounded —
> `make ship` — because that fact was retrieved into the prompt. **He paid the cost once.**

Kinds of explicit fact worth the one-time cost:
- **Project conventions** — "deploys with `make ship`", "tests need `DATABASE_URL` set", "the
  staging box is `box2.internal`".
- **Preferences** — "I prefer tabs", "always use `rg` not `grep`", "keep answers to one line".
- **Debugging discoveries** — "the flaky `test_auth` fails when run in parallel; use `-j1`".
- **Decisions** — "we chose Postgres over SQLite for the analytics table" (the *why* that
  `git blame` can't hold).

Two write paths, both riding the existing directive seam:
- **Agent-initiated:** the model, mid-answer, decides a fact is durable and appends
  `REMEMBER: <fact>` — parsed by `agent.rs:match_directive` exactly like `RUN:`/`NEED:`.
- **User-initiated:** an explicit `Action::Remember` (bar-searchable, `key_design.md` zoning:
  a `C-x a` agent-namespace binding) or the natural-language `? remember that …`.

### 2.3 Episodic memory — "it remembers the story" (uniquely Mars)

What *happened*: the session's history, the terminal commands and their outcomes, what a
watched task concluded, the arc of a debugging session. Mars is the only tool architecturally
positioned to hold this, because `session.rs:server_main` runs `app.tick()` **every loop
whether or not a client is attached** (`session.rs:289`) — it is *present for the whole
story*, including the parts you slept through.

> **Before.** Rae reattaches Monday morning. "What was I doing Friday?" She scrolls three
> panes, squints at half-remembered output, reconstructs the thread of it over five minutes.
>
> **After.** `mars attach` already gives her the W7 reattach briefing (`app.rs:on_attach`) —
> *"while away — build failed: linker error · 2 files modified"*. Episodic memory extends this
> from "since you detached" to "across time": `? what was I doing last Tuesday in this repo`
> retrieves the distilled episode — the commands she ran, the watch verdicts, the files she
> touched — and answers in three lines. **The session remembered the story so she didn't have
> to.**

Episodic sources, all already produced and today merely discarded:
- **Watch verdicts** — `watches[id].verdict` (`app.rs:WatchState`) — "build failed: linker
  error" is *already computed* by `agent::watch_summary`; today it lives in a `Notice` and is
  dismissed. Logged, it becomes episodic memory of every long run's outcome.
- **Command → outcome** — the `TermEvent::Exited` signal already drained in `tick` (3277)
  carries the fact "this command finished"; pairing it with the command that started the pane
  gives command-success history.
- **Reattach diffs** — every `on_detach`/`on_attach` diff is an episode boundary.
- **Ask transcripts** — `agent_history` (`app.rs:212`) is the conversation; today it caps at
  the last 12 turns (`agent.rs:build_messages`) and dies with the bar's `C-l`.

### 2.4 Ranking: delight × frequency × uniqueness

Scored the way `strategy.md` §2 scores scenarios — **ownability (how impossible for an
incumbent) × frequency × delight.** Build in this order:

| Rank | Experience | Kind | Delight | Freq | Uniqueness (moat) | Verdict |
|---|---|---|---|---|---|---|
| **1** | **"it just knows" my build/test/files** | adaptive | high | **every session** | med (frecency is portable, but *terminal-command* frecency needs the daemon's sight) | **ship first — nearly free** |
| **2** | **"I told it once" project facts** | explicit | **very high** | daily | high (the fact + the sight to apply it) | **ship first — the `REMEMBER:` directive** |
| **3** | **"remembers the story" (episodic recall)** | episodic | high | weekly-daily | **highest** — only a detached daemon holds it | Phase 3 |
| 4 | warm-start briefing across time | episodic | med-high | daily | high | rides W7 (`on_attach`) — extend, don't rebuild |
| 5 | semantic recall of a past discovery | explicit+episodic | high | weekly | high (needs embeddings) | Phase 2, RAG |

The top two are the wedge: **adaptive costs the user nothing and is half-shipped; explicit
costs one deliberate act and reuses the directive seam.** Episodic is the deepest moat but the
most build, so it lands third — exactly the `strategy.md` §7 pattern of winning the cheap
daily delight first to earn the right to build the deep one.

---

## 3. The cog-sci framing — why memory is ergonomics, not a feature

`key_design.md` §1 is already a science spine. Memory extends four of its invariants:

1. **Recognition over recall** (Norman; `key_design.md` §1.2). The doctrine already says
   *"knowledge lives in the world."* Memory is the purest expression of it: instead of the
   user recalling that this repo deploys with `make ship`, the knowledge lives in the
   workspace and is *recognized back to them* at the moment of need. The user's job shrinks
   from "remember the fact" to "recognize the fact when Mars surfaces it."

2. **Reducing re-derivation cost.** Every fact re-derived is wasted cognition —
   `agentic_inline.md`'s whole thesis is that the engineer is employing themselves as the
   AI's clipboard. Memory generalizes the anti-clipboard argument across *time*: don't re-explain
   your build command every morning, don't re-discover the flaky test every sprint. The
   `strategy.md` §3 time-savings math (~45–75 min/day from sight) gets a compounding tail: a
   fact learned once saves its re-derivation cost on every future occurrence.

3. **The warm start on reattach** (frecency, `key_design.md` §1.7; and the power law of
   practice §1.3). Skill accumulates on *stable mappings*. Memory is what makes the mapping
   stable across sessions: the finder ranks `src/app.rs` first *today* because it did
   *yesterday*, so the user's motor habit ("top result is my file") is reinforced instead of
   reset. A cold start every morning would reset the practice curve; memory is spatial-temporal
   stability made durable.

4. **The interruption budget — memory surfaces silently or on-pull, never nags**
   (`key_design.md` §3.5, `agentic_inline.md` §5, `workflows_eng.md` §2.2). This is the
   non-negotiable. Memory has **two legitimate surfaces and no third:**
   - **Silent injection** into the agent's context header (the user never sees it; it just
     makes answers better).
   - **On-pull retrieval** when the user asks (`?`) or a `NEED: memory` fires.

   Memory **never** gets a push path to the screen. The one exception — the reattach briefing
   — is already gated by the pull-model `notices` queue (`app.rs:on_attach` pushes a `Notice`;
   the renderer reads it; `dismiss_notice` pops it). This is the "err quiet" doctrine made
   *structural*, exactly as `workflows_eng.md` §2.2 enforced it for watches: **the memory
   subsystem has no `push_to_screen` method by construction.** A memory system that popped up
   "I remembered you like tabs!" would violate the single most important product invariant.

The failure mode to fear (§5): memory that makes the agent **confidently wrong** — recalling a
stale fact ("`make deploy`" after the repo switched to `make ship`) and stating it with the
authority of the present screen. Recognition-over-recall cuts both ways: a *wrong* fact
recognized back to the user is worse than no fact. This is why write-path dedup/decay (§4.3)
is not polish — it is the thing that keeps memory trustworthy enough to obey invariant 1.

---

## 4. Technical design — the memory substrate

Grounded, concrete, decisions made. Memory rides three existing seams and adds one small
module; **it is not a from-scratch build.**

### 4.1 Storage — where memory lives

Three scopes, three homes, one schema. The scope determines the home:

| Scope | Home | Rationale | Cite |
|---|---|---|---|
| **global** (prefs, cross-project habits) | `~/.config/mars/memory.json` — sibling of the existing `state.json` | reuse `config.rs:state_path` / `app_config_dir`; already migrates `ares→mars` | `config.rs:302/307` |
| **project** (deploy command, conventions) | `.mars/memory.json` at `project_root` | travels with the repo, shareable via git like `.claude/memory/`; project detection already exists | `project.rs:project_root` |
| **session/episodic** (the story) | daemon-resident in `App`, flushed to `~/.local/state/mars/<name>.episodes.jsonl` | the daemon is the only process present for the whole session (`session.rs:server_main` ticks detached) | `session.rs:243` |

**Decision: do not overload `state.json`.** `PersistedState` (`app.rs:3757`) stays exactly as
it is — three frecency maps, pure counters, hot-path-serialized. Memory *records* are a
different shape (text + provenance + embedding) with a different write cadence; they get their
own file. Adaptive memory (the frecency counters) *is* `state.json` and needs no new storage —
it needs a new *counter* (`cmd_frecency`) and a *surfacing path* (§4.3), not a new store.

**The record schema.** One struct, `memory.rs` (new module, ~120 lines, mirrors `project.rs`'s
size and shape):

```rust
// memory.rs (new)
#[derive(Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: u64,
    pub fact: String,               // "this repo deploys with `make ship`"
    pub source: Source,             // AgentInferred | UserStated | Episodic
    pub scope: Scope,               // Global | Project | Session
    pub created_tick: u64,          // frame_tick at write (for decay/recency)
    pub last_used_tick: u64,        // bumped on retrieval — frecency for facts
    pub uses: u32,                  // retrieval count — the self-improving signal (§4.4)
    pub embedding: Option<Vec<f32>>,// None in Phase 1 (keyword-only); Some after Phase 2
}
pub enum Source { AgentInferred, UserStated, Episodic }
pub enum Scope  { Global, Project, Session }
```

`created_tick`/`last_used_tick`/`uses` are deliberately the **same frecency shape** the codebase
already trusts (`app.rs:frecency` is `HashMap<String,u32>`) — memory recall ranks by the same
frequency×recency math (`key_design.md` §1.7), so a fact you keep needing floats up and a fact
you never touch decays out. `embedding` is `Option` so Phase 1 ships with it always `None` and
Phase 2 backfills it — no schema migration.

The store lives on `App` beside the frecency maps:

```rust
// App { … }
pub memories: Vec<Memory>,          // loaded from the three homes at startup, by scope
next_memory_id: u64,
```

### 4.2 Write path — how memory is *formed*

Two paths, matching §2's implicit/explicit split.

**Implicit (adaptive + episodic) — zero user cost, fed from `tick`.** The signals already
flow through `app.rs:tick`; today they're used and discarded. Tap them:

- **Command frecency** — where `run_shell_command` writes a `TYPE:` command or the user runs
  one, bump a `cmd_frecency: HashMap<String,u32>` (normalize to the first token + notable args,
  so `cargo build --release` and `cargo build` both credit "cargo build"). Persisted in
  `state.json` alongside the existing three maps — one field added to `PersistedState`
  (`app.rs:3757`), serialized by `save_state` (`app.rs:3740`). **This is the whole of adaptive
  build/test/lint learning.**
- **Watch-verdict logging** — in the `AgentEvent::WatchSummary` arm of `tick` (`app.rs:3373`),
  the verdict is already stored to `watches[id].verdict`. Additionally append an `Episodic`
  memory record (`fact = verdict`, scoped `Session`) to the episode log. Zero new LLM calls —
  the verdict is *already summarized*.
- **Correction detection** — see §4.4.

**Explicit (agentic facts) — one deliberate act, via a new directive.** Add one variant to the
enum that `agent.rs:AgentDirective` (line 9) already defines:

```rust
pub enum AgentDirective {
    Run(String), Type(String), Open(String), Need(NeedKind),
    Remember(String),               // NEW — "REMEMBER: <fact>" — a durable fact to store
}
```

Parsed in `agent.rs:match_directive` (line 66) exactly like the others — after the `NEED:`
block, add:

```rust
if let Some(rest) = l.strip_prefix("REMEMBER:") {
    let fact = rest.trim().trim_matches('`').trim().to_string();
    if !fact.is_empty() { return Some(AgentDirective::Remember(fact)); }
}
```

Taught to the model in `agent.rs:system_prompt` (line 187), one line beside the `RUN:`/`NEED:`
vocabulary:

> `REMEMBER: <fact>` — record a durable fact about this project or the user's preferences
> (e.g. a deploy command, a convention, a decision) so future sessions recall it. Use sparingly,
> only for facts that will still be true next week.

**Applied in `app.rs:handle_bar_ask`** — the same `Enter` arm (2261–2301) that dispatches every
other directive. Unlike `RUN:`, `REMEMBER:` is **not destructive** and needs no confirm gate,
but per the "visibility before action" doctrine (`key_design.md` §3.1) it *is* shown before it
lands — the pending directive renders as `▶ Enter to remember: "…"` and one keystroke confirms,
so memory is never written behind the user's back:

```rust
Some(agent::AgentDirective::Remember(fact)) => {
    self.agent_directive = None;
    let scope = self.default_memory_scope();   // Project if in a repo, else Global
    self.remember(fact, Source::UserStated, scope);   // dedup/decay handled inside
    self.status_msg = Some("Remembered ✓".into());
}
```

The user-initiated path (`? remember that …`, or an `Action::Remember`) funnels into the same
`App::remember` method — one write path, three triggers, exactly the `handle_bar_ask` single-
application-point discipline `workflows_eng.md` §1.2 praises.

### 4.3 Dedup, decay, contradiction — the CLAUDE.md "edit, don't duplicate" rule

The single biggest risk to memory's trustworthiness (§3, §5) is stale/contradictory facts.
This repo's own `CLAUDE.md` states the rule: *"When new evidence contradicts a stored fact,
edit the existing entry. Don't leave a conflicting version alongside it."* `App::remember`
enforces it on every write:

1. **Dedup on near-match.** Before inserting, scan same-scope memories for a close match. Phase
   1: cheap lexical overlap (shared significant tokens / Jaccard over word-sets) above a
   threshold ⇒ treat as the same fact. Phase 2: cosine similarity over embeddings ⇒ a far more
   reliable contradiction detector (this is a *reason* to want embeddings, §4.5).
2. **Contradiction → edit, not append.** If a new fact matches the *subject* of an old one
   ("deploys with X") but differs in the *predicate* ("make ship" vs "make deploy"), replace the
   old fact's text and reset `created_tick`. The most recent statement wins — the user just
   taught you the current truth.
3. **Decay.** Retrieval ranks by frecency (`last_used_tick`, `uses`). A fact never retrieved in
   N sessions sinks below the injection budget and, past a hard age with `uses == 0`, is pruned
   on save. Episodic records decay fastest (the story of last month rarely matters); `UserStated`
   facts decay slowest (the user paid to write them). **Decay is not deletion of value — it is
   the mechanism that keeps a stale fact from being confidently recalled.**

### 4.4 The self-improving loop — memory that gets better as you use it

Three feedback signals, all already observable in `tick`/`handle_bar_ask`, close the loop:

- **Retrieval frecency.** Every time a memory enters the prompt (§4.5), bump its `last_used_tick`
  and `uses`. Facts you keep needing rise; facts you never touch decay (§4.3). This is
  `key_design.md` §1.7 applied to facts instead of commands — *the same substrate, a new
  namespace*, exactly as the doctrine anticipates.
- **Command success rates.** Pairing `cmd_frecency` with the `TermEvent::Exited` exit signal
  (available in `tick`, 3277) lets adaptive memory prefer commands that *succeed* — "the build
  command" becomes "the build command that exits 0", not the one you typo and re-run.
- **Accepted vs. rejected suggestions.** The directive gate is already a labeled feedback
  channel. When the user presses `Enter` on a `TYPE:`/`RUN:` directive (`handle_bar_ask` 2261),
  that's an *accept*; when they edit the command first, or dismiss with `Esc`/`Backspace`
  (2302–2320, which already clears `agent_directive` on edit), that's a *reject/correction*.
  Logging accept/reject per suggested command teaches adaptive memory which of its guesses land
  — and a *correction* (user edits `make deploy`→`make ship` before running) is the highest-
  signal write there is: it is a `UserStated` fact discovered for free, mid-flow.

The loop is the point: **sight feeds memory (watch verdicts, screen context), memory feeds
sight (better-grounded answers), and use ranks memory (frecency) — a flywheel that a from-cold
competitor cannot start.**

### 4.5 Read path / RAG — how memory enters the prompt efficiently

This is where memory meets the token budget, and the design **reuses the Context Bus + `NEED:`
machinery `workflows_eng.md` §2.1 already specced** rather than inventing retrieval from
scratch.

**Memory is a new `ContextSource`.** `workflows_eng.md` §2.1 designs a `ContextSource` registry
where each source snapshots itself into a budgeted string. Memory is one more source:

```rust
// context.rs — the registry from workflows_eng.md §2.1
pub struct MemorySource;
impl ContextSource for MemorySource {
    fn id(&self) -> &'static str { "memory" }
    fn snapshot(&self, app: &App, budget: &mut Budget) -> Option<String> {
        let hits = app.retrieve_memories(app.last_question_or_screen_topic(), MEM_HEADER_MAX);
        if hits.is_empty() { return None; }
        Some(hits.iter().map(|m| format!("- {}", m.fact)).collect::<Vec<_>>().join("\n"))
    }
}
```

Until that registry lands, the interim is a **three-line append inside `screen_context`**
(`app.rs:2878`) — the same place `session:`/`tabs:` headers are built — tagged
`### source:memory ###` so the migration to the registry is mechanical and behavior-preserving.

**Retrieval, phased — cheap first, semantic later.** The `key_design.md` §6 retrieval ladder is
explicit: *"subsequence scoring to ~200 actions, add n-gram at ~200, embeddings only past ~1k
and only local."* Memory obeys the same discipline:

- **Phase 1 — keyword + frecency (ships today, no embeddings, no new dependency).** Retrieve by
  scoring each candidate memory on (a) significant-token overlap with the question / current
  screen topic and (b) its frecency (`last_used_tick`, `uses`). Take the top K under a small
  budget. **This is the same subsequence-scoring machinery the finder already uses**
  (`app.rs` ranks `file_frecency` results the same way) — no model call, no latency, works
  offline and with no API key. For a few hundred facts this is *entirely sufficient*; the
  doctrine's own ladder says embeddings don't pay rent until ~1k items.
- **Phase 2 — embedding-based semantic recall (when scale/precision demands it).** The `ureq`
  OpenAI-compatible path already exists — `agent.rs:chat` POSTs to `{cfg.url}/chat/completions`
  (line 397). The **`/embeddings` endpoint is a sibling on the same base URL with the same auth
  header** — an `agent::embed(cfg, text) -> Vec<f32>` is `chat`'s twin pointing at
  `{cfg.url}/embeddings`, ~20 lines, reusing `AgentConfig::from_env` (line 152). Embeddings are
  computed **once at write time** (fill `Memory.embedding`) and cached, so retrieval is a local
  cosine scan — no per-ask embedding cost for stored facts, only for the query.
  - **Which model / local vs. API.** Recommendation: **default to the provider's embedding
    endpoint** (Gemini/OpenAI-compat `text-embedding-*` — same key the user already set, one
    round-trip, no local model to ship), but **honor a `MARS_EMBED_URL` override to a local
    endpoint** (Ollama's `/api/embeddings` or an OpenAI-compat local server) for the
    privacy-first / offline user — precisely mirroring the existing `MARS_LLM_URL` provider
    story (`agent.rs` env precedence, `key_design.md` decision log). Local embeddings are the
    right *default* for a privacy-sensitive user because memory of terminal output may contain
    secrets (§5) — but shipping a bundled model is a Phase-2 weight decision, so start with the
    already-configured API endpoint and make local a one-env-var opt-in.

**The always-inject vs. retrieve-on-`NEED:` tradeoff — the core read-path decision.**

- *Always-inject a small memory header* — put the top ~3–5 retrieved facts in **every** ask's
  context (the `MemorySource` above, tight budget). Pro: zero extra round-trips, the agent is
  *always* grounded in your conventions, the "it just knows" feeling is constant. Con: a small,
  fixed token tax on every ask, and irrelevant facts occasionally injected.
- *Retrieve-on-`NEED:`* — inject only a one-line *index* ("N project facts on record; ask
  `NEED: memory <query>` for them"), and let the model pull on demand via the re-ask machinery
  `app.rs:reask_with_need` (2798) already implements for `NEED: scrollback`/`NEED: tab`. Pro:
  everyday asks stay cheap; the model pulls memory only when the question is about a remembered
  thing. Con: one extra round-trip when it *does* need memory; determinism cost.

**Decision: both, layered — a tiny always-on header + `NEED: memory` for depth.** Inject the
top **3** highest-frecency facts (a ~200-char header — trivially within the 6 KB `screen_context`
CAP) so the constant "it just knows" delight is free and always present, AND expose
`NEED: memory <query>` (a new `NeedKind::Memory(String)` arm, satisfied by `expand_context`
at `app.rs:2815` exactly as `Scrollback`/`Tab` are) so the model can pull the long tail on the
rare ask that needs a specific old discovery. This mirrors `workflows_eng.md`'s ruling for
scrollback — *cheap default, model-driven expansion for depth* — and reuses its **entire
re-ask path** (`reask_with_need`, the `need_depth` cap of 1 in `tick` at 3310, the "not surfaced"
discipline). **Memory adds no new retrieval control flow; it adds one `NeedKind` variant and one
`ContextSource`.**

`NeedKind` grows one arm:

```rust
pub enum NeedKind { Scrollback, Tab(String), Memory(String) }   // agent.rs:23
```

and `expand_context` (`app.rs:2815`) gains a `Memory(query)` arm that runs the Phase-1/2
retrieval at full depth and returns the hits — one match arm beside the existing two.

---

## 5. Tradeoffs, risks, privacy

**The overriding risk: memory that makes the agent *worse*.** A confidently-recalled stale fact
is more dangerous than no memory, because it wears the authority of the live screen (§3). This
is the reputation risk `agentic_inline.md` §5 names for watches — *"the first false alarm costs
more than the feature earns"* — applied to recall. Mitigations, all in the design above:
edit-don't-duplicate on contradiction (§4.3), decay of unused facts (§4.3), frecency-ranked
injection so only *live* facts surface, and — the backstop — **the model is told memory is
*prior belief, not present fact*** (a system-prompt line: *"Memory below may be stale; the LIVE
SCREEN always wins on conflict"*). Sight overrides memory by construction.

**Staleness / contradiction** — covered by §4.3. The one residual: a fact true in project A
injected in project B. Mitigated by **scope** (`Scope::Project` facts load only under their
`project_root`; global facts are deliberately few — prefs, not commands).

**Prompt-budget cost** — the always-on header is 3 facts / ~200 chars, well inside
`screen_context`'s 6 KB CAP (`app.rs:2879`); the long tail is pulled only on `NEED: memory`,
paying its round-trip *only when used* (`need_depth` capped at 1, `tick` 3310 — no loops). This
is the exact budget discipline `workflows_eng.md` §2.1 already ships.

**Embedding cost / latency** — sidestepped by computing embeddings **once at write time**, not
per ask (§4.5); retrieval is a local cosine scan. A user with 500 facts embeds 500 times total,
amortized over months, versus per-ask. Phase 1 has *zero* embedding cost — it's keyword+frecency.

**Privacy — the sharpest edge.** Terminal output that feeds episodic memory may contain
secrets: API keys echoed, `.env` dumps, tokens in URLs. Rules, non-negotiable:
- **Same consent gate as `screen_context`.** Memory of terminal content is written and sent to
  the LLM under **exactly** the consent the Context Bus applies to screen content
  (`workflows_eng.md` §2.1's per-source `default_consent`) — never a wider gate. If a pane isn't
  consented for sight, its content isn't remembered.
- **Redaction on write.** Before an episodic record is stored, run the same secret-shaped
  redaction the store should apply to any logged output (high-entropy tokens, `KEY=…`,
  `Bearer …`). Store the redacted form; never the raw secret.
- **Per-project isolation.** `Scope::Project` memory lives in that repo's `.mars/memory.json`
  and is never loaded outside `project_root` — a client's secrets never leak into another
  client's session. Global memory is deliberately restricted to preferences (no output).
- **Explicit facts are the safe default.** `UserStated`/`AgentInferred` facts are short,
  user-visible-before-write (§4.2's confirm), and secret-free by construction. Episodic
  auto-logging is the only path that touches raw output, so it is the one that gets redaction +
  the strictest decay + an off switch (`tuning.rs` knob `episodic_memory: bool`, default the
  privacy-conservative choice until redaction is proven in a real-terminal pass).

**"Err quiet" — memory never nags** (§3, invariant 4). Enforced structurally: memory has no
push path to the screen. It injects silently or answers on pull. The only visible surface is the
already-gated reattach briefing (`app.rs:on_attach` → `notices` → `dismiss_notice`). There is no
"memory panel," no toast, no "I learned something!" A memory subsystem that interrupts has
already failed the product.

---

## 6. Phased build recommendation

Mapped to the six primitives of `strategy.md` §4 — memory is **not a seventh primitive; it
rides P1 (Context Bus), P2 (Trigger/Watch), and P6 (Project index)**, which is exactly why it's
cheap. Each phase is a small delta on shipped seams.

### Phase 1 — "it just knows" + "I told it once" (almost free; ships now)
*Rides: P1 Context Bus (`screen_context`), the directive seam (`parse_directive`), the frecency
substrate (`PersistedState`).*
1. **Promote frecency into an adaptive-memory surface.** Add `cmd_frecency` to `PersistedState`
   (`app.rs:3757`, `save_state` 3740); feed it from `run_shell_command`/`TermEvent`. Surface the
   top build/test commands and reach-for files as a 3-line memory header in `screen_context`
   (`app.rs:2878`). *No new store, no LLM call, no UI.*
2. **The `REMEMBER:` directive** (`agent.rs:AgentDirective` +1 arm, `match_directive` +1 block,
   `system_prompt` +1 line, `handle_bar_ask` +1 dispatch arm — the exact 4-point surface
   `workflows_eng.md` §2.3 uses for a new directive) writing to `memory.rs`'s
   `~/.config/mars/memory.json` (global) / `.mars/memory.json` (project via `project.rs:project_root`).
3. **Keyword+frecency retrieval** as a `MemorySource` (interim: append in `screen_context`),
   the always-on top-3 header. Dedup/decay in `App::remember` (§4.3).
4. **Verify:** the `--selfcheck` harness (`main.rs`) — assert a `REMEMBER:` directive parses
   (extend the pure `parse_directive` tests), a stored fact is retrieved into `screen_context`,
   a contradicting write edits-not-appends, and a same-question re-ask injects the fact. Agent
   replies simulated via `app.agent_tx.send(...)` + `app.tick()`, exactly as the watch test does.

Phase 1 delivers ranks 1 and 2 (§2.4) — the wedge — with **no embeddings and no new control
flow**, only new data on shipped seams.

### Phase 2 — semantic recall (RAG; when the fact count / precision demands it)
*Rides: P1, the `NEED:` re-ask machinery (`reask_with_need`), the `ureq` OpenAI-compat path.*
1. `agent::embed` — `chat`'s twin against `{cfg.url}/embeddings`; `MARS_EMBED_URL` for local.
2. Backfill `Memory.embedding` at write; retrieval upgrades to cosine + frecency blend.
3. `NeedKind::Memory(query)` (`agent.rs:23` +1) satisfied by `expand_context` (`app.rs:2815`
   +1 arm) — the model pulls the long tail on demand, reusing the *entire* `need_depth`-capped
   re-ask path. Embedding-based dedup sharpens contradiction detection (§4.3).

### Phase 3 — episodic timeline (the deepest moat)
*Rides: P2 Trigger/Watch (`maybe_fire_watches`, watch verdicts), W7 (`on_detach`/`on_attach`).*
1. Log watch verdicts (`tick`'s `WatchSummary` arm, 3373 — zero new LLM calls, the verdict is
   pre-computed) and command→outcome pairs to the session episode log (`session.rs` daemon-
   resident, flushed to `~/.local/state/mars/<name>.episodes.jsonl`).
2. Extend the W7 reattach briefing (`app.rs:on_attach`) from "since detach" to "across time":
   `? what was I doing last Tuesday` retrieves distilled episodes via the Phase-2 RAG path.
3. Redaction + the `episodic_memory` off switch (§5) proven in a real-terminal pass
   (per `AGENTS.md` — headless can't verify secret-shaped output from real PTYs).

**Cross-cutting, decided once (mirrors `workflows_eng.md` §4):** memory has **no push path** —
silent injection or on-pull only, the interruption budget made structural; **the live screen
always overrides a remembered fact** (a system-prompt line); **one write path**
(`App::remember`, three triggers) with dedup/decay on every write; **one `ContextSource` + one
`NeedKind`**, not a parallel retrieval stack; **a selfcheck per new seam**, agent replies
simulated via `agent_tx.send` + `tick`.

---

## Executive summary

Memory is Mars's **third axis** — sight (what's on screen) × persistence (survives the weekend)
× **memory (what it learned)** — and it's the one that *compounds*: every session starts warmer,
and the daemon that already sees every pane and persists frecency is the only place it can live.

**Top 3 delight experiences (delight × frequency × moat):**
1. **"It just knows" (adaptive).** The editor learns your build/test commands and reach-for
   files with *zero explicit save* — frecency (`app.rs:frecency`/`file_frecency`, already
   persisted to `state.json`) generalized from a ranking signal into a recall signal. So `?`
   already knows "the build" means `cargo build`.
2. **"I told it once" (explicit).** `? remember this repo deploys with make ship` → the agent
   emits a new `REMEMBER:` directive (a +4-point delta on the shipped `AgentDirective` seam) →
   stored per-project in `.mars/memory.json` → recalled, grounded, next week. The CLAUDE.md
   model, for the end user's workspace.
3. **"Remembers the story" (episodic).** *"What was I doing last Tuesday?"* — the daemon logged
   watch verdicts (`watches[].verdict`, already computed by `watch_summary`) and command
   outcomes while you were even detached. The deepest moat: only a present-and-persistent daemon
   holds it.

**Phase-1 build (almost free, ships now):** promote frecency into an adaptive-memory header in
`screen_context` (add a `cmd_frecency` counter to `PersistedState`); add the `REMEMBER:`
directive (enum arm + `match_directive` block + `system_prompt` line + `handle_bar_ask`
dispatch); retrieve with **keyword + frecency** (no embeddings, no API key, `key_design.md` §6's
own ladder says embeddings don't pay rent until ~1k facts), injected as a tiny always-on top-3
header plus a `NEED: memory` pull reusing the existing `reask_with_need` machinery. Embeddings/
RAG (Phase 2) and the episodic timeline (Phase 3) ride the same Context Bus + `NEED:` + Trigger
seams — **memory is a set of small deltas on shipped code, not a from-scratch build**, and it
never nags: silent injection or on-pull only, live screen always wins on conflict.
