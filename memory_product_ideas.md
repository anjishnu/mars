# Mars — The Terminal That Travels With You

*The synthesis of two designs into one product: [`ssh_strategy.md`](./ssh_strategy.md) (your
LLM key never leaves home — a key-never-leaves-home SSH-tunnel proxy) and
[`memory_ideas.md`](./memory_ideas.md) (the three kinds of memory — adaptive, explicit, episodic —
retrieved as a `ContextSource`). Read those first; this doc does not re-derive them. It makes
one argument they each imply but neither states: **the tunnel that carries your key is the same
tunnel that carries your memory, and together they make your terminal a thing you carry — one
identity, everywhere, zero footprint.** Sibling to [`strategy.md`](./strategy.md) (the
sight × persistence thesis, scenario #2), [`key_design.md`](./key_design.md),
[`workflows_eng.md`](./workflows_eng.md), and [`agentic_inline.md`](./agentic_inline.md).*

---

## 0. The one-liner

> **Your terminal forgets you the instant you SSH into a box. Mars doesn't. You hop onto a
> fresh prod host and the agent already knows this repo ships with `make ship`, already
> remembers last week's incident, already has your model access — and not one byte of your
> key or your memory ever touches that box's disk.**

Everyone else's AI terminal is a program installed *on a machine*. Mars is a thing that lives
*on you* and reaches down to whatever machine you happen to be standing on. That is the whole
product in one sentence, and the rest of this document is the argument that it's buildable —
lean, local-first, on machinery Mars has already shipped — and defensible.

---

## 1. The killer feature set — lead with agentic memory

`strategy.md` §1 names the two load-bearing properties no competitor has together: **sight**
(the agent reads every pane) and **persistence** (the workspace lives in a daemon that
outlives the client). `memory_ideas.md` §1 adds the **third axis** — memory — the one that
*compounds* the other two over time. This document adds the fourth, orthogonal move that makes
all three worth ten times more: **portability**. Sight, persistence, and memory that are
locked to one machine are a great single-box editor. The same three, carried securely to every
host you touch, are a category nobody else can enter — because to follow, a competitor would
need a daemon (tmux has it, no AI), a memory store (Claude Code has it, dies with the window),
*and* a secure cross-host identity channel (nobody has it), all at once.

The through-line: **a fact about you should follow you.** Your key is a fact about you
(`ssh_strategy.md` §1). Your build command is a fact about you. Last Tuesday's debugging
discovery is a fact about you. None of them is a fact about the machine you're currently typing
on — so none of them should be stranded there, and none should have to be re-supplied every
time you hop.

Seven customer stories, ranked by delight × frequency × how-uniquely-ownable. The first is the
flagship because it fuses both source designs into one moment.

### Story 1 — "SSH in and it already knows you" *(the flagship)*

> **Before.** It's 2am, prod is down, PagerDuty hands you a jump host you've touched twice this
> year. You `ssh incident-box-7`. Fresh shell. No history. You paste your `GEMINI_API_KEY` out
> of 1Password (now it's in this box's `~/.bash_history`, on a box you don't own). You start an
> agent. It knows *nothing* — not that this service deploys with `make ship`, not that the same
> alert fired three weeks ago and the cause was a stale migration lock. You re-derive all of it,
> under pressure, at 2am. Twenty minutes gone before you've typed a real command.
>
> **After.** You `mars ssh incident-box-7`. The pane comes up and `?` already answers *"this is
> the `payments` service; it deploys with `make ship`; three weeks ago this same p99 alert was
> a stale advisory lock — the fix was `make unlock-migrations`. Want me to check the lock
> table?"* — because the moment the agent assembled that answer, the remote daemon reached back
> through the SSH tunnel to your home broker, which holds both your key *and* your memory store,
> retrieved the top project facts and the matching past episode, and injected them. **The box
> never saw your key, never saw your memory store, only the assembled prompt in flight and the
> answer coming back.** You are three lines into a fix before you'd previously have finished
> exporting a variable.

This is the intersection of `ssh_strategy.md` (key never leaves home) and `memory_ideas.md`
(episodic recall of the past incident, `watches[].verdict` logged by a daemon that was present
for it) — and it is *only* possible because both ride the same channel. Ownability: maximal.
No incumbent can produce this moment without simultaneously owning a persistent daemon, a
memory store, and a zero-footprint identity tunnel.

### Story 2 — "Told it once on my laptop, it knows on the server"

> **Before.** Sam figures out on his laptop that this repo's flaky `test_auth` only fails under
> parallelism — use `-j1`. Tuesday he's on the CI box debugging the same suite, has completely
> forgotten, and burns fifteen minutes rediscovering it. The knowledge was in his head on
> Monday and on the wrong machine on Tuesday.
>
> **After.** Monday, on his laptop: `? remember test_auth is flaky under parallelism, use -j1`.
> The agent emits a `REMEMBER:` directive (`memory_ideas.md` §4.2); the fact lands in the home
> broker's project-scoped store. Tuesday, on the CI box, he asks *"why is test_auth flaky?"* and
> the answer is instant and grounded — because the retrieval happened at home and the fact came
> down the tunnel. **A `REMEMBER:` recorded on any host is available on every host**, because
> the store is per-identity, not per-machine.

`memory_ideas.md` §2.2's "I told it once" — made portable. The `REMEMBER:` write path
(`agent.rs:AgentDirective` +1 arm) writes *through the broker*, not to local disk, so the fact
is instantly global to your whole fleet.

### Story 3 — "It learned my muscle memory across all my boxes"

> **Before.** Adaptive/frecency memory (`app.rs:frecency`/`file_frecency`, already persisted to
> `state.json` per `memory_ideas.md` §2.1) is *per machine*. Priya's laptop knows "the build" is
> `cargo build --release`; the staging box she SSHes into daily learned nothing and ranks her
> reach-for files cold. Her motor habits reset on every host.
>
> **After.** Command frecency (`cmd_frecency`, `memory_ideas.md` §4.2) is a fact about *Priya*,
> so it lives in the home store and is retrieved over the tunnel. The staging box's agent's
> context header already reads *"usual build: cargo build --release · frequently-edited:
> src/app.rs"* on the first ask of the session — never taught, earned by watching, and earned
> **once for all her machines.** The practice curve (`key_design.md` §1.3, power law) stops
> resetting at every host boundary.

This is the subtle one, and the most defensible: adaptive memory silently becomes
*cross-machine* the instant its store lives at the broker. Zero new UX; enormous compounding.

### Story 4 — On-call: the running narrative that hops with you

> **Before.** An incident walks you across five hosts — the LB, two app boxes, the DB primary, a
> log aggregator. Each `ssh` is a cold start; you carry the thread of the investigation in your
> head and in scattered scrollback you can't cross-reference.
>
> **After.** The episodic log (`memory_ideas.md` §2.3, daemon-resident, fed by watch verdicts and
> command→outcome pairs) is written to the broker as you go. Host 4's agent can answer *"what
> have we ruled out so far?"* — the LB was healthy, app-box-1's error rate was normal, the
> primary showed lock contention — because the running narrative is a property of *your session
> across hosts*, reconstructed from the broker, not trapped in five disconnected shells. The
> war-room (`strategy.md` scenario #6) finally spans the actual war.

### Story 5 — The warm start on any machine (W7, extended across hosts)

> **Before.** The reattach briefing (`app.rs:on_attach`, `strategy.md` scenario #4) tells you
> what changed since you detached — but only for the one daemon on the one box. Attach to a
> different host and you're cold.
>
> **After.** "Where was I?" is answered *across hosts*: `mars attach` on box B can brief you on
> the build that failed on box A while you were gone, because episode boundaries and verdicts
> flushed to the broker are queryable from anywhere you attach. The W7 diff extends from
> "since you detached from *this* daemon" to "since you last touched *this project*, on any
> box."

### Story 6 — Detach from the laptop, reattach from your phone

> **Before.** You close the laptop mid-task. The context is gone until you reopen that exact
> machine.
>
> **After.** Identity, memory, and key all live at home, not in the client. A thin client on
> your phone attaches to the remote daemon (`session.rs` is already a thin-client protocol) and
> the agent is *still you* — same memory, same access (via the leased-token path, `ssh_strategy.md`
> §4 phase 2, for the detached window). The client is disposable; you are not.

### Story 7 — Onboard a box in zero seconds, offboard in zero seconds

> **Before.** New host = export the key, wait to accrue local frecency, re-teach project facts.
> Compromised host = scrub N machines for the leaked key and hope you got them all
> (`ssh_strategy.md` §3).
>
> **After.** New host = nothing to configure; the agent arrives fully-loaded over the tunnel.
> Compromised host = nothing to clean up, because nothing was ever there — no key at rest, no
> memory at rest (`ssh_strategy.md` §5, extended to memory). Onboarding and offboarding both
> collapse to zero.

**The learning model, borrowed and reframed.** Stories 2, 3, and 5 are Claude Code's own
learning hierarchy — the `CLAUDE.md` cascade (global → project → local), the `.claude/memory/`
discovered-facts store, the ad-hoc → memory → skill promotion — turned outward for the
*end user's* workspace and, crucially, **made portable**. Claude Code's hierarchy is per-repo,
per-machine, on local disk; Mars's is per-identity, retrieved from a broker, applied on
whatever host you're standing on. Same taxonomy, different physics:

| Claude Code (for the agent, local) | Mars (for the end user, portable) |
|---|---|
| `~/.claude/CLAUDE.md` — global rules | Global-scope memory — prefs, cross-project habits (`memory_ideas.md` §4.1) |
| `<repo>/CLAUDE.md` — project rules | Project-scope memory — deploy commands, conventions, `.mars/memory.json` |
| `.claude/memory/*.md` — discovered facts | Episodic + explicit facts, keyword-indexed, frecency-ranked |
| ad-hoc → memory → skill promotion | ad-hoc frecency → `REMEMBER:` fact → (later) parameterized macro |
| lives on one disk | lives at the broker; retrieved to every host over the tunnel |

---

## 2. The cross-machine experience — a customer's day

The mental model to sell: **"my terminal is a thing I carry, not a program on each host."**
Walk one day.

**08:30 — home laptop.** Priya opens Mars. Local daemon, local panes. She asks `?` about a
failing build; the agent's context header already has her build command and reach-for files
(adaptive memory, `state.json` — but now the *canonical* copy lives in the broker, and the
laptop is just another attached host). She discovers a fix and types `? remember the staging
deploy needs DATABASE_URL exported`. That fact is now global.

**10:00 — SSH box A (staging).** `mars ssh staging`. No key export. The agent already knows the
deploy needs `DATABASE_URL` — the fact she recorded ninety minutes ago on her laptop came down
the tunnel. She works. Every command she runs feeds `cmd_frecency` *at the broker*, so it
counts toward her cross-machine muscle memory, not a throwaway local counter.

**11:30 — SSH box B (a colleague's debug host).** `mars ssh box-b`. A box she does not own and
will not harden. The agent works perfectly — her key never lands here (`ssh_strategy.md` §3,
option b), her memory store never lands here, only the assembled prompt in flight. When she
detaches, box B retains *nothing* about her.

**13:00 — detach, lunch.** She closes the laptop. The remote daemons keep ticking
(`session.rs:server_main` ticks whether or not a client is attached); watches keep watching. The
broker leases short-lived tokens (`ssh_strategy.md` §4 phase 2) so detached watchers can still
summarize a finished build — bounded, self-healing, key still home.

**14:00 — reattach from her phone.** Thin client, remote daemon. "Where was I?" spans both
boxes. Same identity, same memory. The client is a window; she is the room.

**What is consistent across every hop** (travels with the identity): her key access, her memory
(adaptive + explicit + episodic), her habits, her project facts, the agent's very personality.
**What is local to each host** (stays put): the actual shell processes, the cwd, the files on
that disk, the PTYs. This is the correct seam — you carry *knowledge and identity*, not *state
that belongs to a machine*. Mars never pretends box B's filesystem is box A's; it pretends
*you* are the same person on both, which you are.

**The commands.** `mars ssh <host>` is a thin wrapper over `ssh` that sets up the forwarded
socket (`ssh_strategy.md` §4: `RemoteForward` + `SetEnv=MARS_AUTH_SOCK`) — the user types one
thing and the identity channel is just *there*. `mars attach <host:session>` reattaches to a
named remote session. `mars ls` already lists sessions with attached/detached state; extended,
it lists them across your known hosts.

**The friction removed, quantified.** Status quo per new host: ~30s to fish and export a key,
plus a fresh secret at rest to eventually scrub, plus a cold agent that re-derives your build
command (`strategy.md` §3 costs it at minutes/day) and knows none of your project facts. For an
SSH-heavy engineer touching 5–10 boxes a week, that's minutes of paper-cut friction per hop and
a genuinely unbounded blast radius (`ssh_strategy.md` §3: the key on *every* box you ever
configured). Mars takes all of it to zero: **supply once, works everywhere, leaks nowhere**
(`ssh_strategy.md` §5) — now not just for the key, but for everything the agent knows about you.

---

## 3. Secure transmission of memory + keys across hosts

The elegant claim, stated plainly: **the SSH-tunnel proxy that carries your KEY is the SAME
channel that carries your MEMORY.** `ssh_strategy.md` §2 option (b) already relocates the LLM
*call* to a home broker so the key never leaves home. This design adds one observation — the
broker is also the natural owner of the memory store (`memory_ideas.md` §4.1 already homes the
canonical store at `~/.config/mars/`, which *is* the home machine) — so **both LLM requests and
memory reads/writes proxy back through the one tunnel to the one broker.** The remote host is a
pure execution surface with zero secrets and zero memory at rest.

### 3.1 The unified secure channel

The remote Mars daemon, when assembling a grounded agent answer, makes **two** kinds of call
back home over the forwarded socket:

1. **`LlmRequest`** — the fully-formed prompt (messages, model, params). The broker injects
   `Authorization: Bearer …`, runs today's `chat()` body verbatim (`agent.rs:396`, POSTs to
   `{cfg.url}/chat/completions`), and streams the completion home→remote. *Key never leaves home*
   (`ssh_strategy.md` §2b).
2. **`MemoryQuery` / `MemoryWrite`** — retrieval and record. The broker runs keyword+frecency
   retrieval *against the store it holds* (§3.5 below) and returns only the top-k facts the
   remote is allowed to see; a `REMEMBER:` or an implicit frecency bump comes up as a
   `MemoryWrite` and is deduped/decayed *at the broker* (`memory_ideas.md` §4.3). *Store never
   leaves home.*

Both are the exact same `session.rs` frame machinery pointed at a second socket
(`ssh_strategy.md` §2 makes this point for the key; it generalizes for free to memory). One
`0700` socket at `$HOME/.mars/auth.sock`, one accept loop, one version handshake.

### 3.2 The protocol — extend `session.rs`'s frame types

`ssh_strategy.md` §4 proposes `BrokerFrame::{ChatRequest, ChatResponse, ChatError}`. Extend that
one enum with the memory variants — same JSON-lines, same `write_frame` (`session.rs:61`) +
`BufReader::read_line` pair:

```rust
// broker.rs — sibling of session.rs's ClientFrame/ServerFrame, one socket
pub enum BrokerRequest {
    Llm { model: String, messages: Vec<Value>, max_tokens: u32, temperature: f64 },
    MemoryQuery { scope: Scope, query: String, k: usize },   // retrieval at home
    MemoryWrite { record: MemoryWriteReq },                  // REMEMBER: / frecency bump
}
pub enum BrokerResponse {
    Llm { text: String },
    Memory { facts: Vec<String> },   // only the top-k the remote may see
    Ack,
    Error { message: String },
}
```

This is `ssh_strategy.md`'s three-variant broker with two more variants. Nothing structural is
new: the broker is `session.rs:server_main`'s shape (accept loop, per-connection thread,
`Hello`-style version guard at `session.rs:432/497`), and the remote agent is a thin pump exactly
as `ssh_strategy.md` §4 step 4 describes for the LLM call — only now it also pumps memory frames.

### 3.3 Detection and dispatch — the real seams

- **`AgentConfig::from_env()`** (`agent.rs:152`) gains its highest-precedence branch, above the
  `LLM_KEY`/`GROQ`/`GEMINI` ladder: if `MARS_AUTH_SOCK` is set, `provider = "broker"` and
  `broker_sock = Some(path)` (`ssh_strategy.md` §4 step 3). One branch serves *both* key and
  memory — the socket is the identity channel, not merely the key channel.
- **`chat()`** (`agent.rs:396`) forks on `provider == "broker"`: instead of `ureq::post`, it
  writes a `BrokerRequest::Llm` frame to the socket and blocks on the response (`ssh_strategy.md`
  §4 step 4).
- **The `MemorySource` `ContextSource`** (`memory_ideas.md` §4.5) forks the same way: on a broker
  provider, `retrieve_memories()` doesn't scan a local `Vec<Memory>` — it sends
  `BrokerRequest::MemoryQuery` and returns the facts the broker hands back. `expand_context`
  (`app.rs:2978`), satisfying a `NEED: memory <query>`, does the same at full depth.
- **`is_configured()`** (`agent.rs:182`) becomes true when `MARS_AUTH_SOCK` resolves to a
  reachable socket — so the remote UI's "agent unavailable" hint is honest.

### 3.4 Threat model

The ranking question, from `ssh_strategy.md` §3, now asked of memory too: **when a remote box is
fully compromised, what does the attacker get, and for how long?**

| Asset | At rest on remote? | In remote memory? | Blast radius if remote owned |
|---|---|---|---|
| **LLM key** | **No** (`ssh_strategy.md` §2b) | **No** | Ride the *live* tunnel to spend quota; **cannot obtain the key**; loses all access on detach. |
| **Memory store** | **No** — lives at broker | **No** — only top-k in-flight facts | See only the facts *this project's* retrieval surfaced during the live session; the store, the other projects, the raw episodic log — all unreachable. |
| **Assembled prompt** | No — in flight only | Transiently | The one real exposure: the prompt the remote assembled and the answer it got. Same surface `screen_context` already sends; gated identically. |

- **MITM on the tunnel.** The transport is OpenSSH's already-audited encrypted channel
  (`ssh_strategy.md` §2a) — a Unix-socket remote-forward, same shape as `SSH_AUTH_SOCK`. There is
  no plaintext memory or key on the wire outside SSH's encryption.
- **The trust boundary is your home machine, and only your home machine** (`ssh_strategy.md` §3).
  Memory moves the boundary nowhere new: the broker that holds the key already runs at home and
  is already trusted with the store (`memory_ideas.md` §4.1 homes it at `~/.config/mars/`).
- **Per-project isolation is a security control, not just tidiness.** `Scope::Project` memory
  (`memory_ideas.md` §4.1, §5) is loaded and retrieved *only* for the requesting project's
  `project_root`. A `MemoryQuery` from a compromised host for project A can never surface project
  B's facts — the broker scopes the retrieval before it ever leaves home. A client's secrets
  never leak into another client's session.
- **Redaction before storage.** Episodic logs may capture secrets echoed to a terminal
  (`memory_ideas.md` §5). Redaction (`KEY=…`, `Bearer …`, high-entropy tokens) runs **at the
  broker, on write**, before anything is stored — so even the home store never holds a raw
  secret, and a `MemoryQuery` can't surface one that was never written.
- **Consent gate, identical to `screen_context`.** Memory is written and retrieved under
  *exactly* the consent the Context Bus applies to screen content (`memory_ideas.md` §5,
  `workflows_eng.md` §2.1 per-source `default_consent`) — never a wider gate. If a pane isn't
  consented for sight, its content is neither remembered nor sent.

### 3.5 Retrieval at home is the elegant move (and it doubles as security)

Doing retrieval **at the broker**, not on the remote, is the design's keystone — it is right on
three axes at once:

1. **The store never leaves home.** The remote sends a query string and gets back k facts. The
   corpus — every project's memory, the full episodic log — stays put. Least-privilege by
   construction: the remote learns only what it asked about and is allowed to see.
2. **One round-trip.** Retrieval is local *to the broker* (a keyword+frecency scan over the
   in-memory `Vec<Memory>`, `memory_ideas.md` §4.5 phase 1 — no model call, no latency). The
   remote pays one socket round-trip, invisible against a multi-second LLM completion
   (`ssh_strategy.md` §2b makes the identical latency argument for the key).
3. **The security control and the performance win are the same decision.** Because retrieval
   happens where the data lives, "the store never leaks" and "retrieval is cheap" are not a
   tradeoff — they're the same architectural choice.

---

## 3.5 Agentic memory read/write, efficiently (RAG over the channel)

How the agent reads and writes memory across the tunnel without blowing latency or tokens —
reusing `memory_ideas.md` §4.5's read path, now with the broker as the retrieval site.

**The read path — a tiny always-on header + `NEED:` for depth** (`memory_ideas.md` §4.5's
decided ruling, "both, layered"):

1. **Always-on header.** On each ask, the remote sends `BrokerRequest::MemoryQuery { query =
   last_question_or_screen_topic, k = 3 }`. The broker runs keyword+frecency retrieval
   (`memory_ideas.md` §4.5 phase 1 — significant-token overlap × frecency, the same
   subsequence-scoring the finder already uses on `file_frecency`) and returns the top 3 facts,
   ~200 chars. The remote injects them into `build_messages` (`agent.rs:214`) as a
   `### source:memory ###` block — trivially within `screen_context`'s 6 KB cap (`app.rs:3041`).
   Constant "it just knows," one cheap round-trip folded into the LLM call it's grounding.
2. **`NEED: memory <query>` for the long tail.** When the model needs a specific old discovery,
   it emits `NEED: memory <query>` (a new `NeedKind::Memory(String)` arm, `agent.rs:23`), and the
   existing re-ask machinery (`app.rs:reask_with_need` at 2961, `expand_context` at 2978,
   `need_depth` capped at 1) pulls it at full depth — via the broker, not local disk. Memory adds
   **one `NeedKind` variant and one `ContextSource`**, no new retrieval control flow
   (`memory_ideas.md` §4.5).

**The write path** (`memory_ideas.md` §4.2, §4.3):

- **Explicit** — `REMEMBER: <fact>` (parsed in `agent.rs:match_directive` at 66, one new
  `AgentDirective::Remember` arm at `agent.rs:9`) sends `BrokerRequest::MemoryWrite`. The broker
  runs dedup/decay/contradiction resolution (`memory_ideas.md` §4.3 — "edit, don't duplicate")
  before storing. Shown-before-write (`▶ Enter to remember: "…"`, `memory_ideas.md` §4.2) so
  memory is never written behind the user's back.
- **Implicit** — `cmd_frecency` bumps and episodic verdicts flow up as `MemoryWrite` frames from
  `tick` (`app.rs:3427`), deduped and decayed at the broker. The self-improving loop
  (`memory_ideas.md` §4.4) closes at home, so it improves across *all* your hosts at once.

**Embeddings, later** (`memory_ideas.md` §4.5 phase 2): `agent::embed` is `chat`'s twin against
`{cfg.url}/embeddings` — and because the broker is the only thing that calls the provider, the
broker computes embeddings once at write time and does the cosine scan locally. The remote never
embeds, never holds a vector, never sees the corpus. The same channel, the same key, one more
frame type.

---

## 4. Lean deployment — borrow the learning, skip the infra

This is the load-bearing product decision, and it is *lean*: **no hosted cloud, no account
system, no server to run for v1.**

**The "broker" is just `mars keyd` on your own home machine.** It is `session.rs:server_main`
pointed at a second socket (`ssh_strategy.md` §2, §4 — this equivalence is the whole reason the
build is cheap), now also holding the memory store. **The memory store is local files**
(`memory_ideas.md` §4.1 — `~/.config/mars/memory.json` global, `.mars/memory.json` per project,
reusing `config.rs:app_config_dir`/`state_path` at 310/305). There is no database, no cloud, no
tenant. Your home machine is the entire backend, and you already own it.

**Distribution is one static Rust binary.** `mars` *is* the client, the daemon, *and* the broker
— chosen by subcommand (`mars` attaches, `mars --server` is the daemon, `mars keyd` is the
broker). `cargo install mars` or a prebuilt via cargo-dist; no runtime dependencies, no service
to stand up, nothing to sign up for. The same binary on your laptop is the broker; the same
binary on the prod box is the thin remote. This is the local-first identity Mars already has
(`strategy.md` §6: "runs on your box") extended to "runs on *your* box, reaches every *other*
box."

**Contrast with the heavyweight path — and why lean wins.** `ssh_strategy.md` §2 option (d) is
the SaaS relay: a Mars-hosted service holds your key, routes every prompt (and every retrieved
memory, and every line of code context) through Anthropic-the-company's servers, needs OAuth,
needs an infra bill and a privacy policy. It has the best ergonomics (zero SSH, works from a
phone with no home box) — but it moves the trust boundary onto *our* servers and turns a
local-first terminal into a data-hoarding service. **For a developer tool whose entire adoption
thesis is trust — "your key never leaves your machine, your memory never leaves your machine" —
the lean/local-first path is not a compromise, it's the product.** A developer will `cargo
install` a binary that provably keeps their secrets at home in an afternoon; they will not pipe
their terminal history through a startup's cloud. Lean is the go-to-market.

**The phased rollout** (fusing `ssh_strategy.md` §4 phasing and `memory_ideas.md` §6 phasing into
one order):

1. **Phase 1 — local memory + SSH-proxy key.** Ship `memory_ideas.md` Phase 1 (adaptive
   `cmd_frecency`, the `REMEMBER:` directive, keyword+frecency retrieval as a `MemorySource`) and
   `ssh_strategy.md` Phase 1 (`mars keyd` + `MARS_AUTH_SOCK` + the `chat()` broker fork) as two
   independent, individually-valuable features. Each stands alone.
2. **Phase 2 — portable memory over the tunnel.** The fusion: extend `BrokerFrame` with
   `MemoryQuery`/`MemoryWrite`; point the `MemorySource` and the `REMEMBER:` write at the broker
   when `provider == "broker"`. This is the flagship — Story 1 lights up — and it is a *small
   delta* because both halves already exist. Add leased tokens (`ssh_strategy.md` §4 phase 2) for
   the detached watcher.
3. **Phase 3 — embeddings.** `agent::embed` at the broker; cosine + frecency retrieval; sharper
   contradiction dedup (`memory_ideas.md` §4.5 phase 2, §6 phase 2). Still local, still no cloud.
4. **Phase 4 (optional, deliberate, later) — an opt-in hosted relay** for users without an
   always-on home box (`ssh_strategy.md` §2d, §4 phase 3). A funded product decision, named so it
   isn't stumbled into — not the first move, not the default, and always opt-in.

---

## 5. The architecture — one diagram in prose

```
        YOUR HOME MACHINE                          A REMOTE HOST (prod / CI / jump box)
  ┌──────────────────────────────┐          ┌────────────────────────────────────────┐
  │  mars keyd  (the broker)      │          │  mars --server  (remote daemon)         │
  │  ─ holds the LLM key          │          │  ─ owns the panes + scrollback          │
  │  ─ owns the memory store       │◄────────┤  ─ assembles the grounded prompt        │
  │    (memory.json, episodes)     │  SSH    │  ─ NO key, NO memory store at rest       │
  │  ─ runs keyword+frecency       │ tunnel  │                                          │
  │    RETRIEVAL locally           │ (unix   │  mars  (thin client)  ⇄  or from a phone │
  │  ─ injects Bearer, calls LLM   │  sock   └────────────────────────────────────────┘
  └──────────────┬────────────────┘  fwd,
                 │                    MARS_AUTH_SOCK
                 ▼
        ┌──────────────────┐
        │   LLM provider    │   (Gemini / Groq / OpenAI-compat)
        └──────────────────┘
```

**Data flow for one memory-grounded agent query issued from a remote host:**

1. User on the remote host presses `?` and asks *"how do I ship this?"*
2. The remote daemon builds the local `screen_context` (`app.rs:3041`) — the panes it can see —
   and, because `AgentConfig::from_env` (`agent.rs:152`) detected `MARS_AUTH_SOCK`
   (`provider = "broker"`), its `MemorySource` sends `BrokerRequest::MemoryQuery { scope:
   Project, query: "how do I ship", k: 3 }` up the forwarded socket.
3. The **broker** runs keyword+frecency retrieval over its local store, scoped to this project,
   returns `["deploys with `make ship`", …]` — the store never leaves home.
4. The remote injects those facts into `build_messages` (`agent.rs:214`) and sends the assembled
   prompt as `BrokerRequest::Llm` up the *same* socket.
5. The broker injects `Authorization: Bearer …`, runs `chat()`'s body (`agent.rs:396`), gets the
   completion, streams it back — the key never left home.
6. The remote renders the answer: *"`make ship` — want me to run it?"* with a `TYPE: make ship`
   directive behind the confirm gate. If the model instead needs an old discovery, it emits
   `NEED: memory <query>` and `reask_with_need` (`app.rs:2961`) pulls it via the broker.
7. If the user later says `? remember staging needs DATABASE_URL`, a `BrokerRequest::MemoryWrite`
   goes up; the broker dedups/decays and stores it — instantly global to every host.

**The named seams** (every one already exists or is a small delta):

- **`session.rs` frame protocol** (`ClientFrame`/`ServerFrame` at 37/52, `write_frame` at 61) →
  extended into `BrokerRequest`/`BrokerResponse` with `Llm` + `MemoryQuery` + `MemoryWrite`.
- **`AgentConfig::from_env`** (`agent.rs:152`) → one `MARS_AUTH_SOCK` branch, serving both key
  and memory.
- **`agent::chat`** (`agent.rs:396`) → forks to the socket in broker mode.
- **Memory as a `ContextSource`** (`memory_ideas.md` §4.5) → its `retrieve` forks to the broker.
- **The `REMEMBER:` / `NEED: memory` directives** (`agent.rs:AgentDirective` at 9, `NeedKind` at
  23, `match_directive` at 66) → write/read over the broker.
- **`app.rs:tick`** (3427) → implicit `cmd_frecency`/episodic writes flow up as `MemoryWrite`.

**Build order — almost nothing is from scratch:**

1. `mars keyd` broker (= `server_main` + a 3-variant protocol) and `MARS_AUTH_SOCK` detection —
   *this is `ssh_strategy.md` §4, verbatim.*
2. Local memory (adaptive `cmd_frecency`, `REMEMBER:`, keyword+frecency `MemorySource`) — *this
   is `memory_ideas.md` §6 Phase 1, verbatim.*
3. **The fusion:** two frame variants (`MemoryQuery`/`MemoryWrite`) + forking the `MemorySource`
   and `REMEMBER:` write to the broker when remote. *This is the only genuinely new code, and
   it's small, because both endpoints already exist.*
4. Leased tokens for detached watchers; then embeddings; then (maybe) the relay.

Verify per `AGENTS.md`: extend `--selfcheck` — assert a `MemoryQuery` frame round-trips through a
real broker socket and the retrieved fact lands in `screen_context`; assert a `REMEMBER:` over
the socket stores at the broker; drive it with `agent_tx.send(...)` + `tick()`, no mocks, exactly
as the watch test does. The cross-host `setsid`/tunnel behavior needs the real-terminal pass
`ssh_strategy.md` and `AGENTS.md` §9 both call for — headless can't verify SSH socket forwarding.

---

## 6. Risks & the anti-vision

**Privacy — the sharpest edge, and where taste shows.** Terminal output feeding episodic memory
may hold secrets (`memory_ideas.md` §5). The defenses stack, and the cross-host design *tightens*
them rather than loosening them: redaction on write **at the broker** before storage; the same
consent gate as `screen_context` (never wider); per-project isolation enforced at retrieval so a
compromised host can't cross scopes; and the structural guarantee that **memory never leaves
home** — the remote holds only the top-k facts it asked for and was allowed to see. The thing
that makes this safe is the same thing that makes it work: retrieval at the broker.

**Staleness / confidently-wrong recall** (`memory_ideas.md` §3, §5). A stale fact recalled with
the authority of the live screen is worse than no fact. Mitigated by edit-don't-duplicate on
contradiction, frecency decay of unused facts (both run at the broker, `memory_ideas.md` §4.3),
and the system-prompt backstop: **the live screen always overrides a remembered fact.** Across
hosts this matters more, not less — a fact true on box A may be stale on box B — so `Scope`
isolation and "sight overrides memory" are load-bearing, not polish.

**The offline case.** When the tunnel is down (laptop closed, network gone), the remote has no
key and no memory — by design (`ssh_strategy.md` §2b's one real limitation). The fix is precise,
not broad: leased short-lived tokens (`ssh_strategy.md` §4 phase 2) keep detached watchers alive
for a bounded window, and a small **local memory cache** of the last-retrieved facts (write-through,
read-only when offline, per `memory_ideas.md`'s phasing) keeps the agent warm for the session.
Degrade gracefully; never strand secrets on the box to buy offline convenience.

**What NOT to build — taste is what you refuse:**

- **Do not become a SaaS that hoards terminal data.** The relay (`ssh_strategy.md` §2d) is
  opt-in, late, and never the default. The moment memory routes through our servers by default,
  we've become the thing the product exists to avoid. *Every byte of memory stays on the user's
  machine unless they explicitly choose otherwise.*
- **Do not require an account.** v1 is `cargo install` and your own home box. No signup, no
  tenant, no login. The lean stance (§4) is a values stance.
- **Do not send memory to the LLM without the same gate as the screen** (`memory_ideas.md` §5).
  Memory of terminal content gets *exactly* the consent `screen_context` gets — never a wider
  one. A memory system with a looser gate than sight would be a betrayal of the sight doctrine.
- **Do not give memory a push path to the screen** (`memory_ideas.md` §3 invariant 4). Silent
  injection or on-pull retrieval only. No "I remembered something!" toast, on any host. The
  interruption budget is structural: the memory subsystem has no `push_to_screen` method.
- **Do not let the key or the store land on a remote disk, ever, for any convenience.** Config-sync
  (`ssh_strategy.md` §2c) is an explicit per-host opt-in for a box you fully own — never the
  default, because the default must be *structurally incapable* of leaving a secret behind.

The anti-vision in one line: **a cloud service that reads your terminal, learns your secrets,
and calls it a feature.** Mars is the opposite by construction — the knowledge is yours, it lives
on your machine, and it reaches down to every host you touch without ever landing there. Taste is
the refusal to trade that for ergonomics we could get the cheap way.
