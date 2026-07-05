# Mars Memory — Design Alternatives (for review)

*Four+ genuinely different architectures for Mars's memory system, with honest tradeoffs, so the
approach can be chosen deliberately rather than defaulted into. This is a decision aid, not a
plan — nothing here is built. Companion to the working plan (the "signature-keyed pipeline",
which is Alternative A below).*

## What we're optimizing for (the axes that matter)

Mars's stated values pin the tradeoff space: **lean** (ships in days, no heavy deps), **empirical
& self-correcting** (memory tied to observed outcomes, decays when wrong), **inspectable** ("visible,
not spooky" — you can see and correct what it knows), **low-token** (the LLM shouldn't be in the
write loop), and **offline-capable** (works with no API key). The alternatives differ most on:
representation, who does distillation (code vs LLM), how causality is attributed, semantic vs
lexical recall, and where the cost lands.

---

## Alternative A — Signature-keyed structured store *(the current plan)*

**Essence.** A two-tier store: a bounded episodic ring (raw events) consolidated into a small
semantic `HashMap<Signature, Node>` where nodes carry `stats{success,fail,frecency}` and
**frequency-weighted causal edges** (`error --Resolves--> fix`, weight ↑ on repetition). Everything
keyed by a normalized content **signature** (command → argv[0]+flags; error → stable line). Pure
integer accounting; the LLM never touches the write or attribution path.

**Consolidation/causality.** The fix is the *delta of interventions* between a failing and a
succeeding attempt of the same goal; attribution is by **repetition** (the real fix's edge weight
grows, coincidences decay). Self-correction = relative edge-weight decay.

| Pros | Cons |
|---|---|
| Zero tokens to form memory; works offline / no key | Lexical recall — misses paraphrase ("kick off training" ≠ "launch run") unless worded close |
| Deterministic, trustworthy, low confidently-wrong risk | Signature normalization is a heuristic; can mis-group commands/errors |
| Self-correcting via decay; causality robust via repetition | Causal window can miss *slow* fixes (intervention far from the success) |
| Fully inspectable (the mirror shows real counted records) | Needs shell integration (OSC-133) for the good signal |
| Lean: flat JSON + a HashMap, no new deps | Cross-project semantic recall is weak without embeddings |

**Best when:** you want to ship a trustworthy, cheap, offline core fast — and treat semantic recall
as a later enrichment. (This is the recommended backbone.)

---

## Alternative B — Embedding / RAG memory *(vector store)*

**Essence.** Every fact/episode → an embedding; retrieval is semantic top-k over a vector index,
injected as RAG context. Facts stored as embedded chunks; optional LLM summarization of clusters.

**Consolidation/causality.** Weak on causality — you embed "error+fix" text and retrieve by
similarity; attribution is fuzzy (no counted edges). Consolidation = dedup by cosine threshold.

| Pros | Cons |
|---|---|
| **Semantic recall** — paraphrase-robust; finds the fix even when worded differently | Embedding calls = token/latency cost, or a bundled local model (binary weight) |
| Scales to thousands of facts | A vector-index dependency; opaque ("why did it recall this?") |
| Natural for free-text "how did I do X" and history-as-docs | Privacy: embedding terminal output externally unless a *local* model |
| | Poor causal attribution; harder to make self-correcting |
| | Overkill until ~1k facts (the repo's own retrieval ladder says so) |

**Best when:** memory is large and free-text-heavy and semantic recall is the dominant need. For a
solo dev's per-project memory, likely premature. Strong as a *bolt-on to A* once scale demands it.

---

## Alternative C — LLM-consolidated journal *(agentic reflection; CLAUDE.md-for-machines)*

**Essence.** A human-readable markdown memory file per scope, but **written by the agent** via
periodic reflection over the deterministic episodic log: the model reads recent events and updates
prose ("builds with `just build`; CUDA OOM fixed by `expandable_segments:True`"). The (small) file
is injected wholesale like CLAUDE.md.

**Consolidation/causality.** The LLM infers causality and summarizes in prose during reflection —
grounded in the real event log, but expressed as the model's judgment.

| Pros | Cons |
|---|---|
| Rich, nuanced, human-readable; captures causality heuristics miss | LLM in the consolidation loop = periodic token cost; needs a key |
| **User-editable** (it's just a file); natural team-sharing via git | **Highest confidently-wrong risk** — the model can hallucinate a fact |
| Leverages the model's judgment for attribution + summarization | Non-deterministic; summaries can drift from ground truth |
| Very "agentic memory" — the agent *reflects* and curates | Prose grows; size/decay harder to bound |
| Grounded if reflection reads the *deterministic* log | Closest to Claude Code's model → weakest on the "empirical vs prescriptive" differentiation |

**Best when:** you want the richest, most human-legible memory and accept LLM cost + a hallucination
guardrail. Compelling as an *enrichment layer over A* (deterministic outcomes → LLM writes the
readable summary + proposes links the counters missed), which keeps it grounded.

---

## Alternative D — Lazy / query-time memory *(no consolidation; compute on read)*

**Essence.** Keep only the append-only episodic log (Tier 1). No pre-built semantic store. At query
time, filter the log for relevant episodes (signature/keyword/recency) and return them raw or
LLM-distill on the spot. Causality ("find the fix") is computed on demand.

| Pros | Cons |
|---|---|
| Simplest write path — just log; no consolidation machinery | Read is expensive — scan the log every query |
| No decay logic; always reflects the full raw truth | Doesn't scale — log grows unbounded, or you lose history |
| No risk of a bad consolidation poisoning a store | No cheap always-on "it just knows" header (needs a scan/ask each time) |
| Cheap to prototype | Causal attribution recomputed each time (or LLM each time = tokens) |
| Offline if read-side is heuristic (no LLM) | Punts the hard problem to read-time; no durable distilled facts |

**Best when:** an early prototype, or when memory is small and queried rarely. Good scaffolding to
*collect data* before committing to a consolidation model — but not an endgame.

---

## Alternative E — Hybrid: deterministic skeleton + optional LLM/embedding enrichment

**Essence.** A (signature-keyed, counted, self-correcting) as the trustworthy backbone; **optionally**
add per-node embeddings for semantic recall (B) and periodic LLM reflection (C) to write readable
summaries and propose links the heuristics missed. Degrades gracefully offline to pure A.

| Pros | Cons |
|---|---|
| Deterministic/self-correcting core + semantic recall + rich summaries | Most complex — three mechanisms to maintain |
| Graceful degradation (offline → A; no scale → skip B) | Only worth it at scale; premature to build all at once |
| Each layer is optional and independently valuable | Requires discipline to keep the LLM off the hot path |

**Best when:** the natural *destination* — ship A, bolt on C's reflection, then B's embeddings only if
fact volume demands. Not a v1.

---

## Side-by-side

| Axis | A signature-store | B embeddings/RAG | C LLM-journal | D lazy/query-time | E hybrid |
|---|---|---|---|---|---|
| Tokens to form memory | **none** | medium (embed) | medium (reflect) | none / high (read) | medium |
| Determinism / trust | **high** | medium | low–med | high (raw) | high (core) |
| Semantic recall | low | **high** | high | low | **high** |
| Causal attribution | **good** (repetition) | weak | good-but-fallible | recomputed | **good** |
| Offline / no-key | **yes** | local-model only | no | yes | degrades |
| Inspectable / editable | high (records) | low | **high** (prose) | med (raw log) | high |
| Build complexity | low–med | med–high | med | low | high |
| Scales to many facts | med | **high** | low–med | poor | high |

## How to choose (framing, not a decision)

- Mars's values (lean, empirical, self-correcting, inspectable, offline, low-token) point hardest at
  **A as the backbone** — it's the only one that is trustworthy *and* cheap *and* offline *and*
  causally sound, and it makes the "empirical vs prescriptive" differentiation real.
- **C is the most tempting enrichment** (rich, agentic, editable) — best added *over* A's deterministic
  outcomes so it stays grounded, accepting a periodic token cost and a hallucination guardrail.
- **B** earns its complexity only once per-project fact volume is large and semantic recall is the
  bottleneck — a later bolt-on, not v1.
- **D** is a fine *data-collection prototype* if we want to observe real usage before committing to a
  consolidation model.
- **E** is the destination, reached by phases, not built at once.

Open questions worth deciding: (1) is shell-integration capture (OSC-133) acceptable, or do we stay
composer-scoped? (2) do we want *any* LLM in consolidation (C) for the branding "reflection" story, or
keep it purely deterministic (A) for trust? (3) how much does semantic/paraphrase recall actually
matter for a per-project command/fix memory vs. exact-signature match?
