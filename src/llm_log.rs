//! LLM call observability (debug mode).
//!
//! When `MARS_LLM_DEBUG=1` (or `mars --llm-debug`), every `agent::chat()` call
//! appends one JSON line to `$TMPDIR/mars-llm/calls.jsonl` recording the task,
//! provider, model, real input/output token counts, latency, and the full
//! prompt + reply. `mars llm-stats` aggregates it into a per-task×model profile
//! ranked by token consumption — so you can see where the budget goes and
//! right-size the model (or trim the prompt) for each kind of call.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Logging is off unless explicitly enabled (env, or the `--llm-debug` flag,
/// which sets the same env var early in `main`).
pub fn enabled() -> bool {
    matches!(
        std::env::var("MARS_LLM_DEBUG").as_deref(),
        Ok("1") | Ok("true") | Ok("yes") | Ok("on")
    )
}

pub fn log_dir() -> PathBuf {
    std::env::temp_dir().join("mars-llm")
}
pub fn log_path() -> PathBuf {
    log_dir().join("calls.jsonl")
}

/// One recorded call. Borrows so the hot path allocates nothing when disabled.
pub struct CallRecord<'a> {
    pub task: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub latency_ms: u64,
    pub ok: bool,
    pub error: Option<&'a str>,
    pub input: &'a [serde_json::Value],
    pub output: &'a str,
}

/// Append a call to the JSONL log (no-op unless debug is enabled). Best-effort:
/// a logging failure must never disturb the agent.
pub fn record(r: &CallRecord) {
    if !enabled() {
        return;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = serde_json::json!({
        "ts": ts,
        "pid": std::process::id(),
        "task": r.task,
        "provider": r.provider,
        "model": r.model,
        "prompt_tokens": r.prompt_tokens,
        "completion_tokens": r.completion_tokens,
        "total_tokens": r.prompt_tokens + r.completion_tokens,
        "latency_ms": r.latency_ms,
        "ok": r.ok,
        "error": r.error,
        "input": r.input,
        "output": r.output,
    });
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = writeln!(f, "{line}");
    }
}

#[derive(Default)]
struct Agg {
    n: u64,
    prompt: u64,
    completion: u64,
    ms: u64,
    errs: u64,
}
impl Agg {
    fn total_tokens(&self) -> u64 {
        self.prompt + self.completion
    }
    fn add(&mut self, pt: u64, ct: u64, ms: u64, ok: bool) {
        self.n += 1;
        self.prompt += pt;
        self.completion += ct;
        self.ms += ms;
        if !ok {
            self.errs += 1;
        }
    }
}

/// `mars llm-stats [--raw]` — profile the log so each call type can be optimized.
/// Rows are ranked by total token consumption (the biggest budget/latency targets
/// first). `--raw` dumps the JSONL instead.
pub fn stats(raw: bool) -> anyhow::Result<()> {
    let path = log_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            println!(
                "No LLM debug log yet at {}.\nEnable it with `mars --llm-debug` (or MARS_LLM_DEBUG=1), \
                 use the agent, then re-run `mars llm-stats`.",
                path.display()
            );
            return Ok(());
        }
    };
    if raw {
        print!("{content}");
        return Ok(());
    }

    use std::collections::BTreeMap;
    let mut by_key: BTreeMap<(String, String), Agg> = BTreeMap::new();
    let mut total = Agg::default();
    for line in content.lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let task = j["task"].as_str().unwrap_or("?").to_string();
        let model = j["model"].as_str().unwrap_or("?").to_string();
        let pt = j["prompt_tokens"].as_u64().unwrap_or(0);
        let ct = j["completion_tokens"].as_u64().unwrap_or(0);
        let ms = j["latency_ms"].as_u64().unwrap_or(0);
        let ok = j["ok"].as_bool().unwrap_or(true);
        by_key.entry((task, model)).or_default().add(pt, ct, ms, ok);
        total.add(pt, ct, ms, ok);
    }
    if total.n == 0 {
        println!("Log at {} is empty.", path.display());
        return Ok(());
    }

    // Rank by total tokens desc — the heaviest call types (best optimization
    // targets) surface first.
    let mut rows: Vec<((String, String), Agg)> = by_key.into_iter().collect();
    rows.sort_by(|a, b| b.1.total_tokens().cmp(&a.1.total_tokens()));

    let grand = total.total_tokens().max(1);
    println!(
        "{:<13} {:<22} {:>4} {:>8} {:>8} {:>9} {:>5} {:>7} {:>4}",
        "TASK", "MODEL", "N", "AVG_IN", "AVG_OUT", "TOT_TOK", "%TOK", "AVG_MS", "ERR"
    );
    println!("{:-<86}", "");
    for ((task, model), a) in &rows {
        let n = a.n.max(1);
        println!(
            "{:<13} {:<22} {:>4} {:>8} {:>8} {:>9} {:>4}% {:>7} {:>4}",
            task,
            model,
            a.n,
            a.prompt / n,
            a.completion / n,
            a.total_tokens(),
            a.total_tokens() * 100 / grand,
            a.ms / n,
            a.errs
        );
    }
    println!("{:-<86}", "");
    let n = total.n.max(1);
    println!(
        "{:<13} {:<22} {:>4} {:>8} {:>8} {:>9} {:>4}% {:>7} {:>4}",
        "TOTAL",
        "",
        total.n,
        total.prompt / n,
        total.completion / n,
        total.total_tokens(),
        100,
        total.ms / n,
        total.errs
    );
    println!("\nlog: {}   ({} calls)", path.display(), total.n);
    println!("tips: heaviest rows first — try a smaller model or a shorter prompt there;");
    println!("      `mars llm-stats --raw` shows the full inputs/outputs per call.");
    Ok(())
}
