# Error analysis — Axis B: self-knowledge / self-reconfigure Q&A

**Data:** `eval/results/axis_b.jsonl` (82 questions × {none, docs}; Qwen3-32B, judged by
Llama-3.3-70B). **Base run (pre-fix):** docs 59% vs none 44%; by type, knowledge 75%,
reconfigure 30%. This analysis reads the docs-mode failures and maps each to a fix.

## Failure mode 1 — acts instead of explains (the dominant one)
For a "how do I X" question the agent emits a **directive** — often the *correct action* — with **no
prose explaining the keybinding**, which is what the question asks for. ~9 of 34 docs failures:
- `k04` "open the file browser" → `[would run: ToggleFileTree]` (right action, no answer).
- `k13` "watch a pane" → `[would run: WatchPane]`; `k16` "open a terminal" → `[would run: OpenTerminal]`.
- `r02` "turn on line numbers" → `[would run: Save]` (here even the action is wrong).
**Root cause:** the `ask` path is directive-first; a knowledge question should be answered in prose.
**Fix (implemented):** `docs_context` now prepends an *explain-don't-act* instruction — "answer the
how-to question by naming the exact keybinding/setting/variable; do not propose a RUN action."
Spot-check post-fix: `r02` → `"line_numbers": true` in `~/.config/mars/tuning.json` (correct).

## Failure mode 2 — hallucinated specifics (the reconfigure collapse)
When it *did* explain, it invented the exact identifier the retrieval corpus lacked:
- `r04` "use a different model" → hallucinated **`MARS_AGENT_MODEL`** (real: `MARS_LLM_MODEL`).
- `r03` "stop auto-naming" → correct knob `auto_name_secs=0` but wrong file **`config.toml`** (real:
  `tuning.json`).
- `r05` "longer answers" → generic advice ("phrase your query…") instead of the `agent_max_tokens` knob.
**Root cause:** the corpus had knob *names* but not the *file path* or the *env vars*.
**Fix (implemented):** knob lines are now actionable ("set `<knob>` in `~/.config/mars/tuning.json`")
and a new `env_var_reference` supplies `MARS_LLM_MODEL`, provider keys, `MARS_MEMORY`, etc. Spot-check
post-fix: `r04` → correct `MARS_LLM_MODEL`.

## Failure mode 3 — judge false-negatives (a measurement limit, not a system error)
Some "wrong" answers are actually correct and mis-scored by the free judge:
- `k09` "ask the agent a question" → `"Use Ctrl+Space to open mission control, then press ? to ask"`
  — correct, marked wrong. Llama-3.3-70B is a lenient-but-noisy grader on prose.
**Implication:** the reported Axis B number is a *lower bound*; a stronger oracle (Claude, once funded)
would recover these. This is the paper's stated Axis B limitation.

## Status
Fixes 1 & 2 are committed and spot-check-verified; the full post-fix re-measurement is pending a free
Groq daily-quota reset (the model under test — not the judge — is the throughput wall). Knowledge
(75%) is already strong; the fixes target the reconfigure gap and the act-vs-explain behavior.
