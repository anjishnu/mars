# Error analysis — Axis A: NL → shell translation (base, no memory)

**Data:** `eval/results/axis_a.jsonl` (112 tasks; Qwen3-32B, judged by Llama-3.3-70B).
**Base accuracy:** 75% overall (83% general, 46% personalized). Below are the failure modes
from a read of the wrong/partial cases.

## Failure mode 1 — near-miss on general commands (dominant "partial")
The model produces a functionally-adjacent command that omits a flag or a step the request
implied. These are judged *partial*, and they are the bulk of non-correct general-command cases.
- `a08` "show running processes sorted by memory" → `ps aux` (correct utility, **dropped the
  `--sort=-%mem | head`** the request asked for).
- `a20` "ten largest files" → `du -h --max-depth=0 | sort -hr | head` (a *valid alternative* to the
  reference `find … -printf`, arguably correct — a judge-strictness artifact as much as a model error).
- `a25` "count jsonl records" → `wc -l log` (right idea, **guessed the filename** `log`).
**Takeaway:** general-command errors are precision misses (a missing flag, a guessed filename), not
comprehension failures — consistent with 83% base and suggesting a light prompt tweak (or partial
credit) would lift the general number further.

## Failure mode 2 — placeholder literalness
- `a24` "download the file from the url" → `curl -O URL` (left `URL` as a literal token rather than
  a placeholder path). The model knows the command; it mishandles the unspecified argument.

## Failure mode 3 — ecosystem/project blindness (the personalized collapse)
On project-specific requests the model defaults to the *most common* ecosystem, which is wrong
whenever the user's project differs. This is the 46% personalized number, and it is the entire
motivation for Axis A memory:
- `p03` "deploy the model" → `python app.py` (generic guess; the user's real command is
  `./scripts/deploy.sh --prod`).
- `p02` "run the training job" → `./train.sh` vs. the user's `python train.py --config …`.
These are not knowledge failures — the command is unknowable from the text alone. **Only local
memory of what this user runs can fix them** (see `corrective_memory.md`).

## What would move the number
- General: partial→correct is mostly flag/argument precision — a one-line "prefer the full flags the
  request implies" nudge, and/or reporting partial credit.
- Personalized: not fixable by prompting or scale — it is an information-access gap, closed by memory.
