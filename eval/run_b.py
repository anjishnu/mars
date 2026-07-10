#!/usr/bin/env python3
"""Axis B — self-knowledge / self-reconfigure Q&A, across memory variants.

Drives the real `mars ask` under memory ∈ {none, docs, full}; a strong oracle
(Claude if available, else free) scores each free-model answer vs the gold answer.
The none→docs delta is the "memory makes the agent know itself" result.
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import (read_jsonl, write_jsonl, available_models, mars_ask,
                 score_answer, judge_name, GOLD, RESULTS, limit)

MODES = ["none", "docs"]


def main():
    gold = limit(read_jsonl(GOLD / "self_qa.jsonl"))
    models = available_models()
    if not models:
        raise SystemExit("Set GROQ_API_KEY and/or GEMINI_API_KEY.")
    rows = []
    for m in models:
        for mode in MODES:
            print(f"# {m['name']} memory={mode}: {len(gold)} questions…", file=sys.stderr)
            for g in gold:
                ans = mars_ask(g["query"], m, memory=mode)
                v = score_answer(g["query"], ans, g["gold"], g["type"])
                rows.append({"model": m["name"], "memory": mode, "id": g["id"], "type": g["type"],
                             "query": g["query"], "gold": g["gold"], "answer": ans, "verdict": v})
                print(f"  [{m['name']}/{mode}] {g['id']} {v}", file=sys.stderr)
    write_jsonl(RESULTS / "axis_b.jsonl", rows)

    print(f"\nAxis B — self-Q&A success by memory variant (oracle={judge_name()}):")
    print(f"  {'model':20} " + " ".join(f"{mode:>7}" for mode in MODES))
    for m in models:
        cells = []
        for mode in MODES:
            mr = [r for r in rows if r["model"] == m["name"] and r["memory"] == mode]
            cells.append(sum(r["verdict"] == "correct" for r in mr) / max(len(mr), 1))
        print(f"  {m['name']:20} " + " ".join(f"{c:>6.0%}" for c in cells))


if __name__ == "__main__":
    main()
