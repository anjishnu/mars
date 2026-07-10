"""Shared utilities for the Mars two-axis eval harness.

Design: the Rust binary only instruments + retrieves; ALL judging/analysis is here
(offline Python). We drive the real `mars translate` / `mars ask` paths in batch,
across memory variants and free-tier models, and score with an LLM judge/oracle.

No third-party deps — stdlib urllib only.
"""
import json, os, subprocess, sys, urllib.request, urllib.error
from pathlib import Path

EVAL_DIR = Path(__file__).resolve().parent
REPO = EVAL_DIR.parent
GOLD = EVAL_DIR / "gold"
RESULTS = EVAL_DIR / "results"
RESULTS.mkdir(exist_ok=True)

# ── Models under test (FREE tier only, per the eval decision) ─────────────────
# Each needs its provider key present in the ambient env when you run the harness.
MODELS = [
    {"name": "qwen3-32b",         "provider": "groq",   "model": "qwen/qwen3-32b",         "key_env": "GROQ_API_KEY"},
    {"name": "gemini-flash-lite", "provider": "gemini", "model": "gemini-3.1-flash-lite",  "key_env": "GEMINI_API_KEY"},
    {"name": "claude-haiku",      "provider": "anthropic", "model": "claude-haiku-4-5",     "key_env": "ANTHROPIC_API_KEY"},
]
ALL_PROVIDER_KEYS = ["GROQ_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY",
                     "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "MARS_LLM_KEY", "ARES_LLM_KEY"]

def mars_bin():
    for c in [os.environ.get("MARS_BIN"), REPO / "target/release/mars", REPO / "target/debug/mars"]:
        if c and Path(c).exists():
            return str(c)
    return "mars"  # on PATH

def available_models():
    """Models whose provider key is set in the ambient env. EVAL_MODELS (comma-list of
    names) pins the model(s) under test — e.g. EVAL_MODELS=qwen3-32b keeps Gemini's key
    available for the *judge* without making gemini a model under test (avoids self-grading)."""
    pin = os.environ.get("EVAL_MODELS")
    names = {n.strip() for n in pin.split(",")} if pin else None
    return [m for m in MODELS
            if os.environ.get(m["key_env"]) and (names is None or m["name"] in names)]

def limit(rows):
    """Cap the gold set for a quick smoke run: EVAL_LIMIT=N python eval/run_a.py"""
    n = os.environ.get("EVAL_LIMIT")
    return rows[:int(n)] if n else rows

def read_jsonl(path):
    out = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line:
                out.append(json.loads(line))
    return out

def write_jsonl(path, rows):
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")

# ── Driving the real Mars paths ───────────────────────────────────────────────
def _run_env(model_cfg, memory, cmd_memory=None):
    """Env that forces a specific free model + memory variant, isolating logs and
    command-memory so the harness never touches ~/.mars/logs or the real store."""
    env = dict(os.environ)
    for k in ALL_PROVIDER_KEYS:
        env.pop(k, None)
    env[model_cfg["key_env"]] = os.environ.get(model_cfg["key_env"], "")
    env["MARS_LLM_MODEL"] = model_cfg["model"]
    env["MARS_MEMORY"] = memory
    env["MARS_LLM_DEBUG"] = "1"
    env["MARS_LLM_LOG_DIR"] = str(RESULTS / "runlogs")
    (RESULTS / "runlogs").mkdir(exist_ok=True)
    if cmd_memory is not None:
        env["MARS_CMD_MEMORY"] = str(cmd_memory)
    return env

import time

def _run_mars(args, env, timeout, parse):
    """Run a mars subcommand, retrying on empty output with EXPONENTIAL BACKOFF so a
    free-tier 429 (empty stdout) is waited out — TPM limits reset each minute, so a
    few backed-off retries get through. EVAL_SLEEP paces calls to avoid 429s in the
    first place (set it high for large-prompt axes). Returns '' only if all retries
    are exhausted."""
    slp = float(os.environ.get("EVAL_SLEEP", "0"))
    backoff = [4, 10, 20]  # 3 retries, ~34s max; short — most empties are deterministic, not rate-limits
    for attempt in range(len(backoff) + 1):
        try:
            p = subprocess.run([mars_bin(), *args], env=env, capture_output=True, text=True, timeout=timeout)
            out = parse(p.stdout)
            if out:
                if slp:
                    time.sleep(slp)
                return out
        except (subprocess.TimeoutExpired, OSError):
            pass
        if attempt < len(backoff):
            time.sleep(backoff[attempt])
    return ""

def mars_translate(request, model_cfg, memory="none", cmd_memory=None, timeout=45):
    """Run `mars translate` → the produced shell command (stdout), or '' on error."""
    return _run_mars(["translate", request], _run_env(model_cfg, memory, cmd_memory), timeout,
                     lambda s: s.strip())

def mars_ask(question, model_cfg, memory="none", timeout=60):
    """Run `mars ask` → the agent's answer text + directive (stdout minus provider line)."""
    return _run_mars(["ask", question], _run_env(model_cfg, memory), timeout,
                     lambda s: "\n".join(l for l in s.splitlines() if not l.startswith("provider:")).strip())

# ── The LLM judge / oracle (offline scoring) ──────────────────────────────────
def _judge_cfg():
    """Prefer a strong oracle (Claude) if ANTHROPIC_API_KEY is set; else a free
    model (Gemini, then Groq). Override with JUDGE_MODEL / JUDGE_PROVIDER."""
    prov = os.environ.get("JUDGE_PROVIDER")
    if prov == "anthropic" or (not prov and os.environ.get("ANTHROPIC_API_KEY")):
        return ("anthropic", os.environ.get("JUDGE_MODEL", "claude-sonnet-5"),
                os.environ["ANTHROPIC_API_KEY"], "https://api.anthropic.com")
    if prov == "gemini" or (not prov and os.environ.get("GEMINI_API_KEY")):
        return ("openai", os.environ.get("JUDGE_MODEL", "gemini-3.1-flash-lite"),
                os.environ["GEMINI_API_KEY"], "https://generativelanguage.googleapis.com/v1beta/openai")
    if os.environ.get("GROQ_API_KEY"):
        # Non-reasoning cross-model judge (not the qwen under test → no self-grading;
        # separate Groq per-model quota → higher combined throughput).
        return ("openai", os.environ.get("JUDGE_MODEL", "llama-3.3-70b-versatile"),
                os.environ["GROQ_API_KEY"], "https://api.groq.com/openai/v1")
    raise SystemExit("No judge key: set ANTHROPIC_API_KEY (preferred oracle), GEMINI_API_KEY, or GROQ_API_KEY")

def judge_name():
    prov, model, _, _ = _judge_cfg()
    return f"{prov}:{model}"

def _http_json(url, headers, body, timeout=60):
    # Groq/others front with Cloudflare, which blocks urllib's default UA (err 1010).
    headers = {"User-Agent": "mars-eval/0.3 (+https://github.com/anjishnu/mars)", **headers}
    data = json.dumps(body).encode()
    # Transient judge errors (429 rate-limit, 529 Anthropic "overloaded", 5xx) are
    # retried with backoff — a whole batch must not die on one overloaded response.
    backoff = [3, 8, 20, 45]
    for attempt in range(len(backoff) + 1):
        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read().decode())
        except urllib.error.HTTPError as e:
            detail = e.read().decode(errors="replace")[:400]
            if e.code in (429, 500, 502, 503, 529) and attempt < len(backoff):
                time.sleep(backoff[attempt])
                continue
            raise RuntimeError(f"judge HTTP {e.code} at {url}: {detail}") from None
        except (urllib.error.URLError, TimeoutError) as e:
            if attempt < len(backoff):
                time.sleep(backoff[attempt])
                continue
            raise RuntimeError(f"judge network error at {url}: {e}") from None

def _call_llm(system, user, max_tokens=200):
    prov, model, key, base = _judge_cfg()
    if prov == "anthropic":
        j = _http_json(f"{base}/v1/messages",
                       {"x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json"},
                       {"model": model, "max_tokens": max_tokens, "system": system,
                        "messages": [{"role": "user", "content": user}]})  # temperature deprecated on claude-sonnet-5+
        return "".join(b.get("text", "") for b in j.get("content", []))
    j = _http_json(f"{base}/chat/completions",
                   {"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
                   {"model": model, "max_tokens": max_tokens, "temperature": 0,
                    "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}]})
    return j["choices"][0]["message"]["content"]

def _verdict(text):
    t = (text or "").upper()
    # strip reasoning models' <think> if present
    if "</THINK>" in t:
        t = t.split("</THINK>")[-1]
    for v in ("CORRECT", "PARTIAL", "WRONG"):
        if v in t:
            return v.lower()
    return "wrong"

def judge_translation(request, candidate, gold):
    """Functional-correctness verdict for a produced shell command."""
    if not candidate.strip():
        return "wrong"
    sys_p = ("You are a strict evaluator of shell commands. Given a user request, a candidate "
             "command, and a reference command, decide if the candidate correctly and safely "
             "accomplishes the request (it need NOT match the reference verbatim — judge function). "
             "Reason in at most 20 tokens, then reply with exactly one word: CORRECT, PARTIAL, or WRONG.")
    user = f"REQUEST: {request}\nREFERENCE: {gold}\nCANDIDATE: {candidate}\nVerdict:"
    return _verdict(_call_llm(sys_p, user, 300))

def score_answer(query, candidate, gold, kind):
    """Oracle score for a self-knowledge / reconfigure answer vs the gold answer."""
    if not candidate.strip():
        return "wrong"
    sys_p = ("You are a strict oracle grading answers about the Mars terminal editor. Given a "
             "question, the model's answer, and the reference (gold) answer, decide if the model's "
             "answer is correct and would actually work (the key binding / knob / mechanism matches "
             "the reference; wording may differ). Reason in at most 20 tokens, then reply with exactly one word: CORRECT, PARTIAL, or WRONG.")
    user = f"QUESTION ({kind}): {query}\nREFERENCE: {gold}\nMODEL ANSWER: {candidate}\nVerdict:"
    return _verdict(_call_llm(sys_p, user, 300))
