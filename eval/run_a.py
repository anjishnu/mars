#!/usr/bin/env python3
"""Axis A — base shell-translation accuracy per free model (memory=none).
Drives the real `mars translate` over the frozen gold set; scores with the judge.
Writes eval/results/axis_a.jsonl and prints a summary (overall / general / personalized).
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import (read_jsonl, write_jsonl, available_models, mars_translate,
                 judge_translation, judge_name, GOLD, RESULTS, limit)


def main():
    gold = limit(read_jsonl(GOLD / "translate.jsonl"))
    models = available_models()
    if not models:
        raise SystemExit("Set GROQ_API_KEY and/or GEMINI_API_KEY to eval the free models.")
    rows = []
    for m in models:
        print(f"# {m['name']}: translating {len(gold)} tasks (memory=none)…", file=sys.stderr)
        for g in gold:
            cmd = mars_translate(g["request"], m, memory="none")
            v = judge_translation(g["request"], cmd, g["gold"])
            rows.append({"model": m["name"], "id": g["id"], "personalized": g["personalized"],
                         "request": g["request"], "gold": g["gold"], "candidate": cmd, "verdict": v})
            print(f"  [{m['name']}] {g['id']} {v}: {cmd[:55]}", file=sys.stderr)
    write_jsonl(RESULTS / "axis_a.jsonl", rows)

    print(f"\nAxis A — base translation accuracy (judge={judge_name()}):")
    print(f"  {'model':20} {'overall':>8} {'general':>8} {'personal':>9}")
    for m in models:
        mr = [r for r in rows if r["model"] == m["name"]]
        gen = [r for r in mr if not r["personalized"]]
        per = [r for r in mr if r["personalized"]]
        acc = lambda xs: sum(r["verdict"] == "correct" for r in xs) / max(len(xs), 1)
        print(f"  {m['name']:20} {acc(mr):>7.0%} {acc(gen):>8.0%} {acc(per):>9.0%}")
    print("\n(personalized tasks are project-specific — expected LOW cold; see lift.py for the memory effect)")


if __name__ == "__main__":
    main()
