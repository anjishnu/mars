#!/usr/bin/env python3
"""One-shot: run the whole eval and emit the LaTeX tables.
  python eval/run_all.py            # runs cost (if a dogfooding log exists) + A + lift + B + tables
Needs GROQ_API_KEY and/or GEMINI_API_KEY (models under test) and a judge key
(ANTHROPIC_API_KEY preferred, else the free keys). Cost step needs no key.
"""
import os, sys, runpy
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))


def run(mod):
    print(f"\n{'='*70}\n# {mod}\n{'='*70}", file=sys.stderr)
    sys.argv = [mod]
    runpy.run_path(os.path.join(HERE, mod), run_name="__main__")


def main():
    log = Path.home() / ".mars/logs/calls.jsonl"
    if log.exists():
        try:
            run("cost.py")
        except SystemExit as e:
            print(f"(cost skipped: {e})", file=sys.stderr)
    else:
        print("(no dogfooding log yet → skipping cost/E4; run it after a day of --llm-debug)", file=sys.stderr)
    run("run_a.py")
    run("lift.py")
    run("run_b.py")
    print(f"\n{'='*70}\n# LaTeX tables (paste into paper/main.tex)\n{'='*70}")
    run("tables.py")


if __name__ == "__main__":
    main()
