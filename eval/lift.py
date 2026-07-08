#!/usr/bin/env python3
"""Corrective-memory experiment (the headline memory result).

For each PROJECT-SPECIFIC request the model gets wrong cold, seed the user's own
commands into an isolated memory and re-run: does retrieval fix it? Memory is seeded
with ALL personalized (request→command) pairs at once, so retrieval must surface the
RIGHT one per request (not a trivial one-entry lookup). Isolated via MARS_CMD_MEMORY —
never touches the user's real ~/.mars/cmd_memory.jsonl.
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import (read_jsonl, write_jsonl, available_models, mars_translate,
                 judge_translation, judge_name, GOLD, RESULTS, limit)


def main():
    gold = limit([g for g in read_jsonl(GOLD / "translate.jsonl") if g["personalized"]])
    models = available_models()
    if not models:
        raise SystemExit("Set GROQ_API_KEY and/or GEMINI_API_KEY.")

    seeded = RESULTS / "lift_memory.jsonl"
    write_jsonl(seeded, [{"request": g["request"], "command": g["gold"]} for g in gold])
    empty = RESULTS / "lift_empty.jsonl"
    write_jsonl(empty, [])

    rows = []
    for m in models:
        print(f"# {m['name']}: {len(gold)} personalized tasks, cold vs seeded-memory…", file=sys.stderr)
        for g in gold:
            cold = mars_translate(g["request"], m, memory="none", cmd_memory=empty)
            cv = judge_translation(g["request"], cold, g["gold"])
            warm = mars_translate(g["request"], m, memory="history", cmd_memory=seeded)
            wv = judge_translation(g["request"], warm, g["gold"])
            rows.append({"model": m["name"], "id": g["id"], "request": g["request"], "gold": g["gold"],
                         "cold": cold, "cold_verdict": cv, "warm": warm, "warm_verdict": wv})
            print(f"  [{m['name']}] {g['id']}: cold {cv} → warm {wv}", file=sys.stderr)
    write_jsonl(RESULTS / "lift.jsonl", rows)

    print(f"\nCorrective-memory lift on personalized tasks (judge={judge_name()}):")
    print(f"  {'model':20} {'cold':>6} {'warm':>6} {'wrong→right':>12}")
    for m in models:
        mr = [r for r in rows if r["model"] == m["name"]]
        ca = sum(r["cold_verdict"] == "correct" for r in mr) / max(len(mr), 1)
        wa = sum(r["warm_verdict"] == "correct" for r in mr) / max(len(mr), 1)
        w2r = sum(r["cold_verdict"] != "correct" and r["warm_verdict"] == "correct" for r in mr)
        print(f"  {m['name']:20} {ca:>6.0%} {wa:>6.0%} {w2r:>9}/{len(mr)}")


if __name__ == "__main__":
    main()
