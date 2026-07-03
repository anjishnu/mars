# Design ideas — proposals & product visions

**These docs describe what Mars *could* become, not what it *is*.** They are
forward-looking proposals, product strategy, and engineering designs written to be
reviewed before (or while) building. A doc living here may be unbuilt, partially built,
or since superseded — do **not** read them as a description of the shipped system.

For the system **as it actually exists**, see the root-level docs instead:

- [`../README.md`](../README.md) — what Mars is, for users.
- [`../DESIGN.md`](../DESIGN.md) — architecture, tradeoffs, engineering philosophy.
- [`../architecture_overview.md`](../architecture_overview.md) — file-by-file tour of the code.
- [`../key_design.md`](../key_design.md) — the living UX doctrine + decision log.

## What's here

| Doc | What it proposes |
|---|---|
| `strategy.md` | AI product strategy — scenarios Mars owns, the six primitives, build recommendation. |
| `agentic_inline.md` | The "AI that can see" thesis and what to build next. |
| `workflows_design.md` | Product spec for the first 7 agentic workflows (W1–W7). |
| `workflows_eng.md` | Engineering companion to the above — the Context Bus + `NEED:` machinery. |
| `delighters_design.md` | Pre-launch navigation & polish delighters. |
| `speed_design.md` | Laser-fast movement + the anchored query. |
| `ssh_strategy.md` | Key-never-leaves-home SSH-tunnel proxy for the LLM key. |
| `memory_ideas.md` | Augmenting Mars with memory (adaptive / explicit / episodic + RAG). |
| `memory_product_ideas.md` | The synthesis — a memory-augmented terminal that travels securely across hosts. |

When a proposal here ships, fold its durable rationale into the root design docs and let
the proposal stand as the historical "why we built it this way."
