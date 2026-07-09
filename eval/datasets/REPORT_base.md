# Mars memory-augmentation evaluation — baseline vs. optimized

*Judge/oracle: `openai:llama-3.3-70b-versatile`. Models under test: qwen3-32b. Free-tier only. Correctness is functional (judge decides; the command/answer need not match the reference verbatim).*

## Axis A — natural-language → shell command

**Base accuracy (no memory):**

| Model | Overall | General | Personalized |
|---|---|---|---|
| qwen3-32b | 75% | 83% | 46% |

**Corrective-memory lift (project-specific tasks): baseline vs. +memory**

| Model | Cold (baseline) | +Memory (optimized) | Wrong→Right |
|---|---|---|---|
| qwen3-32b | 38% | 100% | 15/24 |

**Compute (avg tokens per translate call):**

| Variant | avg in | avg out | avg total |
|---|---|---|---|
| baseline (no memory) | 99 | 295 | 395 |
| +memory (history) | 160 | 79 | 240 |

Memory adds ~-155 tokens/call (-40%) for the retrieved few-shot examples — the accuracy-vs-compute trade.


## Axis B — self-knowledge / self-reconfigure Q&A

**Success rate by memory variant (baseline `none` vs. optimized `docs`):**

| Model | docs | none |
|---|---|---|
| qwen3-32b | 59% | 44% |

**By question type (optimized `docs`):**

| Model | knowledge | reconfigure |
|---|---|---|
| qwen3-32b | 75% | 30% |

**Compute (avg tokens per ask call):**

| Variant | avg in | avg out | avg total |
|---|---|---|---|
| baseline (no docs) | 1230 | 296 | 1527 |
| +docs memory | 1830 | 273 | 2104 |

## Takeaways

- Memory retrieval over the agent's own context is the optimization; it trades a modest token increase for accuracy, most visibly on project-specific commands (the wrong→right lift) and self-knowledge questions the base model cannot know.
- All numbers are free-tier models; a stronger oracle (Claude) would tighten the scoring.
