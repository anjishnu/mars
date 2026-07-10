#!/usr/bin/env python3
"""Compute-cost accounting for the memory optimization AND the model-tier ring.

Two provider-agnostic-plus-concrete metrics, per the eval decision:
  1. FLOPs ≈ 2 · N_params · tokens   — model-agnostic inference cost (the ONLY way to
     compare a task run on an 8B tier vs a 70B tier; tokens alone are size-blind).
  2. Dollars — Groq pay-as-you-go published rates (per 1M tokens, in/out). Concrete,
     but provider-specific; edit RATES below if Groq changes them.

Reads the instrumentation log (results/runlogs/calls.jsonl by default) — the same log
the system writes during eval — and reports:
  A. per-model token/FLOPs/$ totals,
  B. the memory delta (none vs +memory: Δtokens → ΔFLOPs → Δ$),
  C. the tier-ring saving (routing a task to the cheapest tier that still answers it).

No LLM calls — pure log math. Usage: python eval/compute.py [calls.jsonl]
"""
import sys, json, collections
from pathlib import Path

# ── Editable measurement knobs ────────────────────────────────────────────────
# Parameter counts for the FLOPs estimate (open-weight models; proprietary → None,
# FLOPs not computable, tokens/$ still are).
PARAMS = {
    "qwen/qwen3-32b": 32e9,
    "llama-3.3-70b-versatile": 70e9,
    "llama-3.1-8b-instant": 8e9,
    "openai/gpt-oss-120b": 120e9,
    "gemini-3.1-flash-lite": None,   # proprietary; size undisclosed
}
# Groq published pay-as-you-go rates, $ per 1M tokens (input, output). As of 2026-01;
# edit if they change. Source: https://groq.com/pricing/
RATES = {
    "qwen/qwen3-32b":            (0.29, 0.59),
    "llama-3.3-70b-versatile":   (0.59, 0.79),
    "llama-3.1-8b-instant":      (0.05, 0.08),
    "openai/gpt-oss-120b":       (0.15, 0.75),
}
# The tier ring (cheapest→dearest). Mirrors the runtime tiers in tuning.json; used here
# only to price "what if this task ran on the tier its difficulty warrants."
TIER_ORDER = ["llama-3.1-8b-instant", "qwen/qwen3-32b", "llama-3.3-70b-versatile"]


def flops(model, tokens):
    n = PARAMS.get(model)
    return 2 * n * tokens if n else None


def dollars(model, pin, pout):
    r = RATES.get(model)
    return (pin * r[0] + pout * r[1]) / 1e6 if r else None


def fmt_flops(f):
    if f is None:
        return "    n/a"
    for u, s in [(1e15, "P"), (1e12, "T"), (1e9, "G")]:
        if f >= u:
            return f"{f/u:6.2f}{s}"
    return f"{f:6.0f} "


def main():
    log = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent / "results/runlogs/calls.jsonl"
    if not log.exists():
        raise SystemExit(f"no log at {log} — run the eval with MARS_LLM_DEBUG=1 first")
    calls = []
    for line in open(log):
        try:
            j = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "task" in j and "model" in j:
            calls.append(j)
    if not calls:
        raise SystemExit("log has no model calls (need 'model' + token fields)")

    pin = lambda c: c.get("prompt_tokens", 0)
    pout = lambda c: c.get("completion_tokens", 0)

    # A. Per-model totals
    bym = collections.defaultdict(lambda: [0, 0, 0])  # n, in, out
    for c in calls:
        m = bym[c["model"]]; m[0] += 1; m[1] += pin(c); m[2] += pout(c)
    print(f"{'PER MODEL':28} {'n':>4} {'in_tok':>8} {'out_tok':>8} {'FLOPs':>9} {'$':>9}")
    for model, (n, i, o) in sorted(bym.items(), key=lambda kv: -(kv[1][1] + kv[1][2])):
        f, d = flops(model, i + o), dollars(model, i, o)
        print(f"  {model:26} {n:>4} {i:>8} {o:>8} {fmt_flops(f)} {('$%.5f'%d) if d is not None else 'n/a':>9}")

    # B. Memory delta — tokens/FLOPs/$ per call, none vs +memory (same model)
    print("\nMEMORY DELTA (per-call avg, none → +memory):")
    bymm = collections.defaultdict(lambda: [0, 0, 0])  # (model,memory) → n,in,out
    for c in calls:
        k = (c["model"], c.get("memory", c.get("retrieval", "?")))
        v = bymm[k]; v[0] += 1; v[1] += pin(c); v[2] += pout(c)
    models = sorted({k[0] for k in bymm})
    for model in models:
        rows = {mem: v for (mo, mem), v in bymm.items() if mo == model}
        base = rows.get("none")
        for mem, (n, i, o) in sorted(rows.items()):
            if mem == "none" or not base or n == 0:
                continue
            bt = (base[1] + base[2]) / max(base[0], 1)
            mt = (i + o) / n
            f0, f1 = flops(model, bt), flops(model, mt)
            d0, d1 = dollars(model, base[1] / max(base[0], 1), base[2] / max(base[0], 1)), dollars(model, i / n, o / n)
            pct = (mt - bt) * 100 / max(bt, 1)
            fl = f"{(f1-f0)/f0*100:+.0f}% FLOPs" if f0 else ""
            dl = f"{(d1-d0)/d0*100:+.0f}% $" if d0 else ""
            print(f"  {model:26} none {bt:6.0f} tok → {mem} {mt:6.0f} tok  ({pct:+.0f}% tok, {fl}, {dl})")

    # C. Tier-ring framing — cost of the SAME token volume on each tier
    print("\nTIER RING (same total tokens priced on each tier — the routing lever):")
    tot_tok = sum(pin(c) + pout(c) for c in calls)
    tot_in = sum(pin(c) for c in calls); tot_out = sum(pout(c) for c in calls)
    for model in TIER_ORDER:
        f, d = flops(model, tot_tok), dollars(model, tot_in, tot_out)
        print(f"  {model:26} {fmt_flops(f)}  {('$%.5f'%d) if d is not None else 'n/a':>9}  (all {tot_tok:,} tok)")
    print("  → routing easy tasks down-tier trades a small accuracy risk for a large FLOPs/$ cut;")
    print("    the ring picks the cheapest tier whose accuracy on the task class clears threshold.")

    # D. Tiered routing on an ILLUSTRATIVE session mix. We only ever measure two task
    # classes in the eval (shell + ask); a real session is dominated by frequent trivial
    # labeling (auto_name on tab-content settle, name_session) that never appears here.
    # So this is a MODELED hour of active use — assumptions stated inline — not measured.
    # Per-call sizes: shell/ask taken from THIS log's measured averages; auto_name/
    # name_session estimated from their prompt shape (a short screen → a few-word label).
    avg = {}
    for c in calls:
        a = avg.setdefault(c["task"], [0, 0, 0]); a[0] += 1; a[1] += pin(c); a[2] += pout(c)
    tok_of = lambda t, dflt: ((avg[t][1] + avg[t][2]) / max(avg[t][0], 1)) if t in avg else dflt
    # (task, calls/hr, per-call tokens, tier). Frequencies from typical dogfooding.
    WORKLOAD = [
        ("auto_name",    40, tok_of("auto_name", 240),   "low"),
        ("name_session",  3, tok_of("name_session", 240), "low"),
        ("shell",        15, tok_of("shell", 360),        "mid"),
        ("ask",           8, tok_of("ask", 1100),         "high"),
    ]
    TIER_MODEL = {"low": "llama-3.1-8b-instant", "mid": "qwen/qwen3-32b",
                  "high": "llama-3.3-70b-versatile"}
    FLAT = "qwen/qwen3-32b"  # today's single-model policy
    fd = td = ff = tf = 0.0
    print("\nTIERED ROUTING — ILLUSTRATIVE hour of use (modeled mix, assumptions shown):")
    print(f"  {'task':14} {'n/hr':>4} {'tok/call':>8} {'tier':>5}  flat$    tiered$")
    for task, n, tk, tier in WORKLOAD:
        vol = n * tk; i = vol * 0.75; o = vol * 0.25  # ~3:1 in:out typical
        f0, f1 = dollars(FLAT, i, o), dollars(TIER_MODEL[tier], i, o)
        g0, g1 = flops(FLAT, vol), flops(TIER_MODEL[tier], vol)
        fd += f0; td += f1; ff += g0; tf += g1
        print(f"  {task:14} {n:>4} {tk:>8.0f} {tier:>5}  ${f0:.5f} ${f1:.5f}")
    print(f"  TOTAL/hr  flat ${fd:.5f} / {fmt_flops(ff).strip()}  →  "
          f"tiered ${td:.5f} / {fmt_flops(tf).strip()}  "
          f"({(td-fd)/fd*100:+.0f}% $, {(tf-ff)/ff*100:+.0f}% FLOPs)")
    print("  Reading: the ring pushes the two high-frequency labeling tasks to the 8B tier")
    print("  (where they're indistinguishable in quality) and reserves the 70B tier for `ask`.")


if __name__ == "__main__":
    main()
