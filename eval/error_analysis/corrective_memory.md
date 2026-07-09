# Error analysis — Corrective memory (project-specific tasks, cold vs. +memory)

**Data:** `eval/results/lift.jsonl` (24 project-specific tasks; cold = empty memory, warm = the
user's own commands seeded, retrieved by BM25). **Result:** cold 38% → +memory 100%
(controlled), 15/24 wrong→right on the base run.

## What memory fixes (the mechanism, made visible)
Cold, the model guesses the *wrong ecosystem*; warm, BM25 surfaces the user's stored
`(request → command)` and the model reproduces it. Every one of these flipped wrong/partial→correct:
| Request | Cold (guess) | +Memory (the user's own) |
|---|---|---|
| run the unit tests | `python -m unittest discover` | **`cargo test`** (it's a Rust project) |
| build the release binary | `npm run build` | **`cargo build --release`** |
| format the code | `black example.py` | **`cargo fmt`** |
| run the training job | `./train.sh` | **`python train.py --config configs/base.yaml`** |
| deploy the model | `./deploy_model.sh` | **`./scripts/deploy.sh --prod`** |
| sync the dataset | `aws s3 sync s3://bucket ./data` | **`aws s3 sync s3://our-bucket/data ./data`** |

Two sub-patterns: (a) **ecosystem correction** (Rust project answered with Python/npm defaults →
corrected), and (b) **exact-argument recall** (right utility, wrong bucket/flag/path → the precise
string recalled, e.g. `s3://bucket`→`s3://our-bucket`, `venv`→`.venv`). Both are cases where the
answer is *unknowable from the request text* and *knowable from the user's history* — exactly the
regime the paper claims.

## Where memory does *not* help (the residual)
15/24 flipped on the base run; the rest were already correct cold or stayed wrong. Residual failures
are retrieval misses — the request paraphrases far enough from the stored one that BM25 does not
surface the right exemplar (lexical, not semantic, matching). This is the honest boundary of "simple
memory" and the motivation for the future semantic-retrieval rung. **Recency/temporal weighting**
(now that memories carry `ts`/`session`/`cwd`) is the cheapest next lever: prefer the user's *recent*
and *same-project* commands when multiple exemplars compete.
