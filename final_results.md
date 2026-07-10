# MARS — Final Experimental Results

**Model under test:** `claude-haiku-4-5` (a small, inexpensive model).
**Judge / oracle:** `claude-sonnet-5`.
**Gold sets:** frozen, a-priori. Translation 112 (88 general + 24 personalized); self-Q&A 82
(52 knowledge + 30 reconfigure). All runs: **0 empty completions**.

> Why a paid small model and not a free one? Both free tiers we targeted (Qwen3-32B on Groq,
> Gemini Flash-Lite) exhausted their **daily** token quotas under a ~350-call batch — the
> free-tier throughput wall the paper describes (§4.4), hit for real. We ran the final,
> complete measurement on a small paid model for reliability. The thesis needs the model to be
> *small*, not *free*; Haiku is small.

## Axis A — NL → shell translation (base, no memory)
| Slice | Accuracy |
|---|---|
| General ($n=88$) | **90%** (79/88) |
| Personalized ($n=24$) | 42% (10/24) |
| Overall ($n=112$) | 79% (89/112) |

The model is a competent general shell author (89%) but cannot know project-specific
conventions (41%), dragging the overall number down — the gap memory closes.

## Corrective memory — leakage-controlled (the headline)
Three conditions, all **tested on the original request**:
| Condition | Accuracy | What it measures |
|---|---|---|
| cold (no memory) | 38% (9/24) | base guess |
| **paraphrase memory** | **96% (23/24)** | **generalization** — a *reworded* prior request is in the store (mean word-Jaccard 0.36 to the query); retrieval must bridge the gap |
| verbatim memory | 100% (24/24) | lookup ceiling — the exact prior request is in the store |

15/24 cold→paraphrase flips, and one regression (p19: `make benchmark`, judged correct cold, became `cargo bench` with memory) — 9 + 15 − 1 = 23. The paraphrase number is the honest result: simple BM25 memory
lifts a small model **38%→96%** by generalizing across a paraphrase gap, not by looking up a
planted answer. (This design controls the leakage a naive verbatim-seed protocol would have.)

## Axis B — self-knowledge / self-reconfiguration Q&A
| Question type | none | +docs memory |
|---|---|---|
| Knowledge (how-do-I) | 58% (30/52) | 65% (34/52) |
| Reconfigure (change setting X) | 17% (5/30) | **93% (28/30)** |
| Overall ($n=82$) | 43% (35/82) | **76% (62/82)** |

Docs memory lifts overall 43%→76%. The reconfigure gain (17%→93%) is the payoff of the
corpus fix: actionable knob lines (name + file + default), an env-var reference, and an
explain-don't-act instruction — so the agent answers with the exact `knob = value in
tuning.json` / `MARS_LLM_MODEL` instead of hallucinating a file or emitting a bare directive.

## Token cost (reported, not headlined)
Memory's effect on token cost is **model-dependent** — an interesting aside, not a headline:
| Model | cold tokens/call | +memory | Δ |
|---|---|---|---|
| claude-haiku-4-5 (non-reasoning) | 95 | 161 | +66 tok (+70%) |
| qwen3-32b (reasoning) | 385 | 217 | −168 tok (−44%) |

On a **non-reasoning** model, memory adds a small few-shot input cost (≈66 tokens) for a large
accuracy gain. On a **reasoning** model, few-shot grounding *suppresses* verbose chain-of-thought,
so memory actually *cuts* tokens ~44%. We report the modest Haiku cost honestly and note the
reasoning-model effect as a phenomenon, rather than claiming a universal token reduction.

## Product bugs found and fixed en route
- The NL→shell prompt's "cap your reasoning" clause (added for reasoning models) made
  **non-reasoning models return empty completions** — now conditional on `is_reasoning_model()`.
- `temperature` is deprecated on the newest Claude models — removed from the Anthropic call path.
- The eval silently used a stale `target/release/mars` — pinned `MARS_BIN`.
