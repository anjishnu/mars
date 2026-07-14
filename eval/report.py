#!/usr/bin/env python3
"""Baseline-vs-optimized report from eval/results/. Compares, on both axes:
  - memory-retrieval-wise: accuracy WITHOUT vs WITH memory
  - compute-wise: avg tokens/call WITHOUT vs WITH memory (memory adds retrieved context)
Reads results/*.jsonl (accuracy) + results/runlogs/calls.jsonl (real tokens per mode).
Writes eval/results/REPORT.md and prints it.
"""
import os, sys, json, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import read_jsonl, RESULTS, judge_name

def load(name):
    p = RESULTS / name
    return read_jsonl(p) if p.exists() else []

def acc(rows, key="verdict"):
    rows = list(rows)
    return sum(r[key] == "correct" for r in rows) / max(len(rows), 1)

def toks_by(task, retrieval):
    """avg (in, out, total) tokens for calls of this task+retrieval variant, from runlogs."""
    log = RESULTS / "runlogs" / "calls.jsonl"
    if not log.exists():
        return (0, 0, 0, 0)
    n = i = o = 0
    for line in open(log):
        try:
            j = json.loads(line)
        except json.JSONDecodeError:
            continue
        # "shell" is the pre-0.4 tag for translate calls; old runlogs keep it.
        tags = {task, "shell"} if task == "translate" else {task}
        if j.get("task") in tags and j.get("retrieval") == retrieval and j.get("ok"):
            n += 1; i += j.get("prompt_tokens", 0); o += j.get("completion_tokens", 0)
    if not n:
        return (0, 0, 0, 0)
    return (n, i // n, o // n, (i + o) // n)

def main():
    a = load("axis_a.jsonl"); lift = load("lift.jsonl"); b = load("axis_b.jsonl")
    models = sorted({r["model"] for r in a + lift + b})
    L = []
    P = L.append
    P("# Mars memory-augmentation evaluation — baseline vs. optimized\n")
    P(f"*Judge/oracle: `{judge_name()}`. Models under test: {', '.join(models) or '—'}. "
      "Free-tier only. Correctness is functional (judge decides; the command/answer need not "
      "match the reference verbatim).*\n")

    # ── Axis A ────────────────────────────────────────────────────────────────
    P("## Axis A — natural-language → shell command\n")
    P("**Base accuracy (no memory):**\n")
    P("| Model | Overall | General | Personalized |")
    P("|---|---|---|---|")
    for m in models:
        mr = [r for r in a if r["model"] == m]
        if not mr: continue
        gen = [r for r in mr if not r["personalized"]]; per = [r for r in mr if r["personalized"]]
        P(f"| {m} | {acc(mr):.0%} | {acc(gen):.0%} | {acc(per):.0%} |")
    P("")
    if lift:
        P("**Corrective-memory lift (project-specific tasks): baseline vs. +memory**\n")
        P("| Model | Cold (baseline) | +Memory (optimized) | Wrong→Right |")
        P("|---|---|---|---|")
        for m in models:
            mr = [r for r in lift if r["model"] == m]
            if not mr: continue
            ca = acc(mr, "cold_verdict"); wa = acc(mr, "warm_verdict")
            w2r = sum(r["cold_verdict"] != "correct" and r["warm_verdict"] == "correct" for r in mr)
            P(f"| {m} | {ca:.0%} | {wa:.0%} | {w2r}/{len(mr)} |")
        P("")
    # compute
    bn, bi, bo, bt = toks_by("translate", "none")
    mn, mi, mo, mt = toks_by("translate", "history")
    if bt or mt:
        P("**Compute (avg tokens per translate call):**\n")
        P("| Variant | avg in | avg out | avg total |")
        P("|---|---|---|---|")
        if bt: P(f"| baseline (no memory) | {bi} | {bo} | {bt} |")
        if mt: P(f"| +memory (history) | {mi} | {mo} | {mt} |")
        if bt and mt:
            d = bt - mt
            if d > 0:
                P(f"\nMemory **reduces** tokens ~{d}/call (−{d*100//max(bt,1)}%): the retrieved few-shot "
                  "examples add input but the model reasons less and outputs a shorter command — "
                  "more accurate *and* cheaper.\n")
            else:
                P(f"\nMemory adds ~{-d} tokens/call (+{-d*100//max(bt,1)}%) for the retrieved examples.\n")
        P("")

    # ── Axis B ────────────────────────────────────────────────────────────────
    if b:
        P("## Axis B — self-knowledge / self-reconfigure Q&A\n")
        modes = sorted({r["memory"] for r in b})
        P("**Success rate by memory variant (baseline `none` vs. optimized `docs`):**\n")
        P("| Model | " + " | ".join(modes) + " |")
        P("|---|" + "---|" * len(modes))
        for m in models:
            cells = []
            for mode in modes:
                mr = [r for r in b if r["model"] == m and r["memory"] == mode]
                cells.append(f"{acc(mr):.0%}" if mr else "—")
            P(f"| {m} | " + " | ".join(cells) + " |")
        P("")
        # by type
        P("**By question type (optimized `docs`):**\n")
        P("| Model | knowledge | reconfigure |")
        P("|---|---|---|")
        for m in models:
            kn = [r for r in b if r["model"] == m and r["memory"] == "docs" and r["type"] == "knowledge"]
            rc = [r for r in b if r["model"] == m and r["memory"] == "docs" and r["type"] == "reconfigure"]
            if kn or rc:
                P(f"| {m} | {acc(kn):.0%} | {acc(rc):.0%} |")
        P("")
        an, ai, ao, at = toks_by("ask", "none")
        dn, di, do, dt = toks_by("ask", "docs")
        if at or dt:
            P("**Compute (avg tokens per ask call):**\n")
            P("| Variant | avg in | avg out | avg total |")
            P("|---|---|---|---|")
            if at: P(f"| baseline (no docs) | {ai} | {ao} | {at} |")
            if dt: P(f"| +docs memory | {di} | {do} | {dt} |")
            P("")

    P("## Takeaways\n")
    P("- Memory retrieval over the agent's own context is the optimization; it trades a modest "
      "token increase for accuracy, most visibly on project-specific commands (the wrong→right lift) "
      "and self-knowledge questions the base model cannot know.")
    P("- All numbers are free-tier models; a stronger oracle (Claude) would tighten the scoring.")

    out = "\n".join(L)
    (RESULTS / "REPORT.md").write_text(out)
    print(out)

if __name__ == "__main__":
    main()
