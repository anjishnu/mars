#!/usr/bin/env python3
"""E4 — token/cost profile from a day's dogfooding log (no LLM calls, pure log math).
Usage: python eval/cost.py [~/.mars/logs/calls.jsonl]
Reports tokens per feature, per session, per session-hour, and the tail (worst 5%).
Cost is in TOKENS (provider-neutral), per the eval decision.
"""
import sys, json, collections
from pathlib import Path


def main():
    log = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.home() / ".mars/logs/calls.jsonl"
    if not log.exists():
        raise SystemExit(f"no log at {log} — dogfood with MARS_LLM_DEBUG=1 first")
    calls, sstart, send = [], {}, {}
    for line in open(log):
        try:
            j = json.loads(line)
        except json.JSONDecodeError:
            continue
        if j.get("kind") == "session_start":
            sstart[j["session_id"]] = j["ts"]
        elif j.get("kind") == "session_end":
            send[j["session_id"]] = j["ts"]
        elif "task" in j:
            calls.append(j)
    if not calls:
        raise SystemExit("log has no calls")

    tok = lambda c: c.get("prompt_tokens", 0) + c.get("completion_tokens", 0)
    grand = sum(tok(c) for c in calls)

    byf = collections.defaultdict(lambda: [0, 0, 0])
    for c in calls:
        f = byf[c["task"]]; f[0] += 1; f[1] += c.get("prompt_tokens", 0); f[2] += c.get("completion_tokens", 0)
    print(f"PER FEATURE (task)   {'n':>4} {'avg_in':>7} {'avg_out':>7} {'tot_tok':>9} {'%':>4}")
    for t, (n, i, o) in sorted(byf.items(), key=lambda kv: -(kv[1][1] + kv[1][2])):
        print(f"  {t:16} {n:>4} {i // max(n,1):>7} {o // max(n,1):>7} {i+o:>9} {(i+o)*100//max(grand,1):>3}%")

    bys = collections.defaultdict(int)
    for c in calls:
        bys[c.get("session_id", "?")] += tok(c)
    print(f"\nPER SESSION (tokens, and tokens/hour when the session was >1min):")
    for sid, t in sorted(bys.items(), key=lambda kv: -kv[1]):
        dur = send.get(sid, 0) - sstart.get(sid, 0)
        rate = f"{t / (dur/3600):,.0f} tok/h" if dur > 60 else "n/a"
        print(f"  {sid:24} {t:>8}  {rate}")

    tot = sorted((tok(c) for c in calls), reverse=True)
    k = max(1, len(tot) // 20)
    print(f"\nTAIL: worst 5% ({k} of {len(tot)} calls) = {sum(tot[:k]):,} tok "
          f"({sum(tot[:k])*100//max(grand,1)}% of the total {grand:,})")


if __name__ == "__main__":
    main()
