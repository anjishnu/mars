//! Lightweight memory / retrieval over Mars's OWN context — the substrate for the
//! two eval axes. Deliberately simple (lexical BM25, no embeddings): the claim is
//! that *sitting at the terminal* and retrieving the user's own commands / Mars's
//! own docs beats a generic model, not that the retriever is fancy.
//!
//! Two corpora, both ranked by [`rank`] and injected as prompt context by `agent.rs`:
//!   (A) command memory — `(request → accepted_command)` pairs the user actually ran,
//!       plus recent shell history; injected as few-shot into shell-translation.
//!   (B) system knowledge — Mars's docs + action registry + tuning descriptions;
//!       injected into the `ask` prompt so the agent can answer/​reconfigure itself.
//!
//! The active variant is chosen by [`MemoryMode`] (env `MARS_MEMORY`, set by the
//! `--memory` flag) so the eval can ablate implementations.

use std::path::PathBuf;

/// Which memory implementation is active for this run (the ablation knob).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoryMode {
    None,
    History, // command memory + shell history → shell-translation
    Docs,    // system knowledge → ask / self-reconfigure
    Full,    // both
}

impl MemoryMode {
    pub fn from_env() -> Self {
        match std::env::var("MARS_MEMORY").as_deref().map(str::trim) {
            Ok("history") | Ok("commands") => MemoryMode::History,
            Ok("docs") | Ok("system") => MemoryMode::Docs,
            Ok("full") | Ok("both") | Ok("all") => MemoryMode::Full,
            _ => MemoryMode::None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryMode::None => "none",
            MemoryMode::History => "history",
            MemoryMode::Docs => "docs",
            MemoryMode::Full => "full",
        }
    }
    pub fn includes_history(self) -> bool {
        matches!(self, MemoryMode::History | MemoryMode::Full)
    }
    pub fn includes_docs(self) -> bool {
        matches!(self, MemoryMode::Docs | MemoryMode::Full)
    }
}

// ── Command memory: the corrective-memory store ───────────────────────────────

/// `~/.mars/cmd_memory.jsonl` — one `{request, command}` per accepted translation.
/// `MARS_CMD_MEMORY` overrides the path so the eval harness can seed a controlled
/// memory without touching the user's real store.
pub fn command_memory_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MARS_CMD_MEMORY") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".mars").join("cmd_memory.jsonl"))
}

/// One temporally-situated command memory. We store atomic events with time,
/// session, and working directory; sequences (prev/next), recency, and per-project
/// scoping are *derived* at read time by grouping on `session` and ordering by `ts`
/// — the substrate for temporal reasoning and procedure/skill mining (§future work).
#[derive(Clone)]
#[allow(dead_code)] // ts/session/cwd stored for temporal reasoning + procedure mining
pub struct CommandMemory {
    pub request: String,
    pub command: String,
    pub ts: u64,           // unix seconds
    pub session: String,   // which session it was run in
    pub cwd: String,       // working directory (project scope)
}

/// Persist a `(request → accepted command)` event with temporal context. Best-effort;
/// called from the accept-outcome hook so the *next* similar request can be answered
/// correctly — and so later work can reason over *when* and *where* commands were run.
pub fn remember_command(request: &str, command: &str) {
    let req = request.trim();
    let cmd = command.trim();
    if req.is_empty() || cmd.is_empty() {
        return;
    }
    let Some(path) = command_memory_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let line = serde_json::json!({
        "request": req,
        "command": cmd,
        "ts": ts,
        "session": crate::llm_log::session_id(),
        "cwd": cwd,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Load the full temporal memory records (chronological as written). Tolerant of the
/// legacy `{request, command}`-only format (missing fields default).
pub fn load_command_records() -> Vec<CommandMemory> {
    let Some(path) = command_memory_path() else { return Vec::new() };
    let Ok(content) = std::fs::read_to_string(&path) else { return Vec::new() };
    content
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|j| {
            Some(CommandMemory {
                request: j["request"].as_str()?.to_string(),
                command: j["command"].as_str()?.to_string(),
                ts: j["ts"].as_u64().unwrap_or(0),
                session: j["session"].as_str().unwrap_or("").to_string(),
                cwd: j["cwd"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Load the stored `(request, command)` pairs (most-recent last) — the view the
/// few-shot retrieval uses. Thin wrapper over [`load_command_records`].
pub fn load_command_memory() -> Vec<(String, String)> {
    load_command_records().into_iter().map(|m| (m.request, m.command)).collect()
}

/// Recent, de-duplicated shell history commands (newest first), read from the
/// user's `$HISTFILE` / `~/.zsh_history` / `~/.bash_history`. Zsh extended-history
/// lines look like `: 1699999999:0;git status` — we strip that prefix.
pub fn shell_history(limit: usize) -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(hf) = std::env::var_os("HISTFILE") {
        candidates.push(PathBuf::from(hf));
    }
    if let Some(h) = &home {
        candidates.push(h.join(".zsh_history"));
        candidates.push(h.join(".bash_history"));
    }
    let Some(path) = candidates.into_iter().find(|p| p.exists()) else { return Vec::new() };
    let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
    let content = String::from_utf8_lossy(&bytes);

    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for raw in content.lines().rev() {
        // Strip zsh extended-history metadata prefix `: <ts>:<dur>;`.
        let cmd = match raw.strip_prefix(':') {
            Some(rest) => rest.splitn(2, ';').nth(1).unwrap_or(rest),
            None => raw,
        }
        .trim();
        if cmd.is_empty() || !seen.insert(cmd.to_string()) {
            continue;
        }
        out.push(cmd.to_string());
        if out.len() >= limit {
            break;
        }
    }
    out
}

// ── System knowledge corpus (Axis B) ──────────────────────────────────────────

/// Read Mars's own docs from the working directory (present when run from the
/// repo, e.g. during eval) and split into retrievable chunks (by blank line).
/// Missing files are simply skipped — the registry/tuning corpus is always present.
pub fn doc_chunks() -> Vec<String> {
    let mut chunks = Vec::new();
    for name in ["README.md", "DESIGN.md", "key_design.md"] {
        let Ok(text) = std::fs::read_to_string(name) else { continue };
        for para in text.split("\n\n") {
            let p = para.trim();
            if p.len() >= 40 {
                chunks.push(p.to_string());
            }
        }
    }
    chunks
}

/// Environment-variable reference for the system-knowledge corpus (Axis B). These
/// are the runtime knobs that live in the env, not tuning.json — the agent tends to
/// hallucinate their names ("MARS_AGENT_MODEL") without this.
pub fn env_var_reference() -> Vec<String> {
    [
        "To use a different LLM model: set the MARS_LLM_MODEL environment variable (e.g. export MARS_LLM_MODEL=gpt-4o-mini).",
        "To point the agent at a custom or local OpenAI-compatible endpoint (e.g. Ollama): set MARS_LLM_KEY and MARS_LLM_URL.",
        "Provider keys (paid-first detection): ANTHROPIC_API_KEY, OPENAI_API_KEY, GROQ_API_KEY, GEMINI_API_KEY. Export one to select that provider.",
        "To turn on memory retrieval: set MARS_MEMORY to history (your commands), docs (system knowledge), or full (both).",
        "To log every LLM call (tokens, latency) for debugging: run mars --llm-debug or export MARS_LLM_DEBUG=1; the log lands in ~/.mars/logs/ and mars llm-stats profiles it.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// ── BM25 ranking ──────────────────────────────────────────────────────────────

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Rank `docs` against `query` with BM25 (k1=1.5, b=0.75); return the indices of
/// the top-`k` (best first). Empty query or docs → empty.
pub fn rank(query: &str, docs: &[String], k: usize) -> Vec<usize> {
    let q = tokenize(query);
    if q.is_empty() || docs.is_empty() {
        return Vec::new();
    }
    let toks: Vec<Vec<String>> = docs.iter().map(|d| tokenize(d)).collect();
    let n = docs.len() as f64;
    let avgdl = toks.iter().map(|t| t.len()).sum::<usize>() as f64 / n.max(1.0);

    // Document frequency per query term.
    let mut df: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    for qt in q.iter().collect::<std::collections::HashSet<_>>() {
        let c = toks.iter().filter(|t| t.iter().any(|w| w == qt)).count() as f64;
        df.insert(qt.as_str(), c);
    }

    let (k1, b) = (1.5f64, 0.75f64);
    let mut scored: Vec<(usize, f64)> = toks
        .iter()
        .enumerate()
        .map(|(i, doc)| {
            let dl = doc.len() as f64;
            let mut score = 0.0;
            for qt in &q {
                let f = doc.iter().filter(|w| *w == qt).count() as f64;
                if f == 0.0 {
                    continue;
                }
                let dfi = *df.get(qt.as_str()).unwrap_or(&0.0);
                let idf = (((n - dfi + 0.5) / (dfi + 0.5)) + 1.0).ln();
                score += idf * (f * (k1 + 1.0)) / (f + k1 * (1.0 - b + b * dl / avgdl.max(1.0)));
            }
            (i, score)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}
