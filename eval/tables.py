#!/usr/bin/env python3
"""Emit LaTeX table bodies from eval/results/*.jsonl to paste into paper/main.tex
(replacing the \\tbd stubs). Prints to stdout; run after run_a / lift / run_b."""
import os, sys, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import read_jsonl, RESULTS


def load(name):
    p = RESULTS / name
    return read_jsonl(p) if p.exists() else []


def pct(xs, pred):
    xs = list(xs)
    return f"{100*sum(pred(r) for r in xs)/max(len(xs),1):.0f}\\%"


def main():
    a = load("axis_a.jsonl")
    lift = load("lift.jsonl")
    b = load("axis_b.jsonl")
    models = sorted({r["model"] for r in a + lift + b})

    print("% ── Table: Axis A base translation accuracy ──")
    print("\\begin{tabular}{lccc}\\toprule")
    print("Model & Overall & General & Personalized \\\\ \\midrule")
    for m in models:
        mr = [r for r in a if r["model"] == m]
        if not mr:
            continue
        gen = [r for r in mr if not r["personalized"]]
        per = [r for r in mr if r["personalized"]]
        ok = lambda r: r["verdict"] == "correct"
        print(f"{m} & {pct(mr,ok)} & {pct(gen,ok)} & {pct(per,ok)} \\\\")
    print("\\bottomrule\\end{tabular}\n")

    print("% ── Table: corrective-memory lift (personalized tasks) ──")
    print("\\begin{tabular}{lccc}\\toprule")
    print("Model & Cold & +Memory & Wrong$\\to$Right \\\\ \\midrule")
    for m in models:
        mr = [r for r in lift if r["model"] == m]
        if not mr:
            continue
        cold = pct(mr, lambda r: r["cold_verdict"] == "correct")
        warm = pct(mr, lambda r: r["warm_verdict"] == "correct")
        w2r = sum(r["cold_verdict"] != "correct" and r["warm_verdict"] == "correct" for r in mr)
        print(f"{m} & {cold} & {warm} & {w2r}/{len(mr)} \\\\")
    print("\\bottomrule\\end{tabular}\n")

    print("% ── Table: Axis B self-Q&A success by memory variant ──")
    modes = ["none", "docs", "full"]
    print("\\begin{tabular}{l" + "c" * len(modes) + "}\\toprule")
    print("Model & " + " & ".join(f"mem={m}" for m in modes) + " \\\\ \\midrule")
    for m in models:
        cells = []
        for mode in modes:
            mr = [r for r in b if r["model"] == m and r["memory"] == mode]
            if not mr:
                cells.append("--"); continue
            cells.append(pct(mr, lambda r: r["verdict"] == "correct"))
        if any(c != "--" for c in cells):
            print(f"{m} & " + " & ".join(cells) + " \\\\")
    print("\\bottomrule\\end{tabular}")


if __name__ == "__main__":
    main()
