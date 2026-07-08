# Mars eval harness — runbook

Two-axis evaluation of the memory-augmented agent. **The Rust binary only instruments +
retrieves; all judging/analysis is here (offline Python, stdlib only).** The harness drives
the real `mars translate` / `mars ask` paths in batch and scores with an LLM judge/oracle.

## Prerequisites
- A built `mars` binary (`cargo build --release`; the harness finds `target/release/mars`,
  or set `MARS_BIN`).
- **Models under test (free tier):** export `GROQ_API_KEY` (for `qwen3-32b`) and/or
  `GEMINI_API_KEY` (for `gemini-flash-lite`). Only the ones you set are evaluated.
- **Judge / oracle:** `ANTHROPIC_API_KEY` is used as the oracle if set (recommended — a
  stronger, independent grader); otherwise it falls back to Gemini, then Groq. Override with
  `JUDGE_PROVIDER` / `JUDGE_MODEL`.
- No `pip install` — stdlib only. Python 3.8+.

## Gold sets (frozen, committed a priori)
- `eval/gold/translate.jsonl` — 37 NL→command tasks (25 general + 12 project-specific,
  marked `personalized`, used for the corrective-memory experiment).
- `eval/gold/self_qa.jsonl` — 30 self-knowledge / reconfigure Q&A, grounded in Mars's real
  keybindings and tuning knobs.

## Run everything
```bash
export GROQ_API_KEY=...   GEMINI_API_KEY=...   ANTHROPIC_API_KEY=...   # what you have
python eval/run_all.py                      # cost (if a log exists) + A + lift + B + LaTeX tables
```
Smoke-test first (2 gold items): `EVAL_LIMIT=2 python eval/run_all.py`.

## Or step by step
| Step | Command | Produces |
|---|---|---|
| Cost / tokens (E4) | `python eval/cost.py ~/.mars/logs/calls.jsonl` | per-feature / per-session / per-session-hour tokens + tail. **No LLM calls.** Needs a day of `MARS_LLM_DEBUG=1` dogfooding. |
| Axis A accuracy | `python eval/run_a.py` | `results/axis_a.jsonl` + overall/general/personalized accuracy per free model |
| Corrective-memory lift | `python eval/lift.py` | `results/lift.jsonl` + cold→warm accuracy + wrong→right count (the headline memory result) |
| Axis B self-Q&A | `python eval/run_b.py` | `results/axis_b.jsonl` + success rate per memory variant (none/docs/full) |
| LaTeX tables | `python eval/tables.py` | 3 `tabular` bodies → paste into `paper/main.tex` (replace the `\tbd` stubs) |

## Notes
- Runs are isolated: eval `mars` calls log to `eval/results/runlogs/` (not `~/.mars/logs/`),
  and the lift experiment seeds an isolated command-memory via `MARS_CMD_MEMORY` — your real
  `~/.mars/cmd_memory.jsonl` and dogfooding log are never touched.
- `results/` is git-ignored; re-runnable and deterministic given the same gold + models.
- Free-tier rate limits: if you hit 429s, re-run (the harness records whatever completed) or
  add a sleep. Cost step (E4) never calls an API.
