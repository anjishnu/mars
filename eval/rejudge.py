#!/usr/bin/env python3
"""Re-judge the stored eval candidates with the Claude oracle (offline; no model-under-
test calls — the candidates are fixed, only the grader changes). Produces the oracle
verdicts for the paper AND a free judge-agreement cross-check (free Gemini/Groq judge vs
Claude), which is itself evidence for how far a free judge can be trusted.

Run with the oracle key in the env:
  ANTHROPIC_API_KEY=... JUDGE_PROVIDER=anthropic JUDGE_MODEL=claude-sonnet-5 python eval/rejudge.py
Writes results/*_oracle.jsonl and prints oracle accuracy + agreement per experiment.
"""
import os, sys, collections
import lib

R = lib.RESULTS

def pct(n, d): return f"{100*n//max(d,1)}%"

def agree(rows, old, new):
    a = sum(1 for r in rows if r.get(old) == r.get(new))
    return f"{pct(a,len(rows))} ({a}/{len(rows)})"

def rejudge_axis_a():
    p = R / "axis_a.jsonl"
    if not p.exists(): return
    rows = lib.read_jsonl(p)
    by = collections.Counter(); tot = collections.Counter()
    for i, r in enumerate(rows):
        r["verdict_oracle"] = lib.judge_translation(r["request"], r.get("candidate", ""), r["gold"])
        kind = "personal" if str(r["id"]).startswith("p") else "general"
        tot[kind] += 1; tot["all"] += 1
        if r["verdict_oracle"] == "correct":
            by[kind] += 1; by["all"] += 1
        print(f"\r  axis_a {i+1}/{len(rows)}", end="", flush=True)
    lib.write_jsonl(R / "axis_a_oracle.jsonl", rows)
    print(f"\nAxis A (oracle={lib.judge_name()}): overall {pct(by['all'],tot['all'])}, "
          f"general {pct(by['general'],tot['general'])}, personal {pct(by['personal'],tot['personal'])}")
    print(f"  agreement with free judge: {agree(rows,'verdict','verdict_oracle')}")

def rejudge_lift():
    p = R / "lift.jsonl"
    if not p.exists(): return
    rows = lib.read_jsonl(p)
    cold = verb = para = 0
    for i, r in enumerate(rows):
        r["cold_oracle"] = lib.judge_translation(r["request"], r.get("cold", ""), r["gold"])
        r["warm_oracle"] = lib.judge_translation(r["request"], r.get("warm", ""), r["gold"])   # verbatim
        r["para_oracle"] = lib.judge_translation(r["request"], r.get("para", ""), r["gold"])   # paraphrase
        cold += r["cold_oracle"] == "correct"; verb += r["warm_oracle"] == "correct"
        para += r["para_oracle"] == "correct"
        print(f"\r  lift {i+1}/{len(rows)}", end="", flush=True)
    lib.write_jsonl(R / "lift_oracle.jsonl", rows)
    n = len(rows)
    c2p = sum(1 for r in rows if r["cold_oracle"] != "correct" and r["para_oracle"] == "correct")
    print(f"\nCorrective memory (oracle): cold {pct(cold,n)} → paraphrase-memory {pct(para,n)} "
          f"(verbatim ceiling {pct(verb,n)}); {c2p}/{n} wrong→right on paraphrase")

def rejudge_axis_b():
    p = R / "axis_b.jsonl"
    if not p.exists(): return
    rows = lib.read_jsonl(p)
    by = collections.defaultdict(lambda: [0, 0])  # (memory,type) → correct,total
    for i, r in enumerate(rows):
        r["verdict_oracle"] = lib.score_answer(r["query"], r.get("answer", ""), r["gold"], r.get("type", ""))
        for k in [(r["memory"], "all"), (r["memory"], r.get("type", ""))]:
            by[k][1] += 1; by[k][0] += r["verdict_oracle"] == "correct"
        print(f"\r  axis_b {i+1}/{len(rows)}", end="", flush=True)
    lib.write_jsonl(R / "axis_b_oracle.jsonl", rows)
    print(f"\nAxis B (oracle):")
    for mem in sorted({k[0] for k in by}):
        cells = " ".join(f"{t}={pct(by[(mem,t)][0],by[(mem,t)][1])}"
                         for t in ["all", "knowledge", "reconfigure"] if (mem, t) in by)
        print(f"  memory={mem:5} {cells}")
    print(f"  agreement with free judge: {agree(rows,'verdict','verdict_oracle')}")

if __name__ == "__main__":
    which = sys.argv[1:] or ["a", "lift", "b"]
    if "a" in which: rejudge_axis_a()
    if "lift" in which: rejudge_lift()
    if "b" in which: rejudge_axis_b()
