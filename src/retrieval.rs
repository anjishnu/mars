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
    crate::sys::paths::home_dir().map(|h| h.join(".mars").join("cmd_memory.jsonl"))
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

/// Recent, de-duplicated shell history commands (newest first), read from the
/// user's `$HISTFILE` / `~/.zsh_history` / `~/.bash_history`. Zsh extended-history
/// lines look like `: 1699999999:0;git status` — we strip that prefix.
fn shell_history(limit: usize) -> Vec<String> {
    let home = crate::sys::paths::home_dir();
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

// ── The prompt-context facade (what agent.rs calls) ──────────────────────────
//
// These two functions are the whole memory surface the rest of Mars sees: the
// mode gate lives inside them, so a caller needs no memory knowledge at all —
// and `retrieval_stub.rs` can mirror them with neutral values in a build
// compiled without the `memory` feature.

/// Few-shot exemplar block for NL→shell translation, or "" when history memory
/// is off or nothing relevant is stored. Command-memory pairs (request →
/// command) rank ahead of bare history commands; ties among pairs break toward
/// the current project (cwd) and the recent past; every line passes through
/// the redaction pass before it can enter a prompt.
pub fn fewshot_for(request: &str) -> String {
    if !MemoryMode::from_env().includes_history() {
        return String::new();
    }
    let mem = load_command_records();
    let hist = shell_history(500); // recent commands
    let t = crate::tuning::load();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut lines: Vec<String> = rank_memories(
        &mem, request, 3, &cwd, now,
        t.memory_cwd_boost, t.memory_recency_boost, t.memory_recency_halflife_days,
    )
    .into_iter()
    .map(|i| format!("- {} → {}", mem[i].request, mem[i].command))
    .collect();
    for i in rank(request, &hist, 5) {
        let cmd = &hist[i];
        if !lines.iter().any(|l| l.contains(cmd.as_str())) {
            lines.push(format!("- {cmd}"));
        }
    }
    lines.truncate(6);
    lines.iter().map(|l| redact(l)).collect::<Vec<_>>().join("\n")
}

/// The most relevant Mars self-knowledge (docs + knob/tier/env references) for
/// `question`, formatted as a system-context block — or None when docs memory
/// is off or nothing is relevant.
pub fn docs_context_for(question: &str) -> Option<String> {
    if !MemoryMode::from_env().includes_docs() {
        return None;
    }
    let mut corpus = doc_chunks();
    corpus.extend(crate::tuning::knob_descriptions());
    corpus.extend(env_var_reference());
    corpus.extend(memory_reference());
    corpus.extend(crate::tiers::tier_descriptions());
    let hits = rank(question, &corpus, 5);
    if hits.is_empty() {
        return None;
    }
    let body = hits.iter().map(|&i| format!("- {}", corpus[i])).collect::<Vec<_>>().join("\n");
    // Frame it: for a how-to / configuration question, ANSWER by explaining the
    // exact setting/keybinding/variable and where to set it — do not emit a RUN
    // directive. This is what fixes the "[would run: X]" non-answers.
    Some(crate::prompts::DOCS_CONTEXT_PREAMBLE.trim_end().replace("{body}", &body))
}

// ── Redaction: nothing secret-shaped enters a prompt ─────────────────────────
//
// The stores stay intact on disk — they are the user's own local files. What is
// guarded is the wire: every memory/history line is passed through [`redact`]
// before it can reach a (possibly cloud-bound) prompt. Shell history is exactly
// where pasted tokens, `password=` flags, and credentialed URLs end up.

const REDACTED: &str = "[REDACTED]";

/// `~/.mars/denylist` — literal strings that must never enter a prompt, one per
/// line (`#` comments). `MARS_DENYLIST` overrides the path (tests/eval).
pub fn denylist_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MARS_DENYLIST") {
        return Some(PathBuf::from(p));
    }
    crate::sys::paths::home_dir().map(|h| h.join(".mars").join("denylist"))
}

fn denylist() -> Vec<String> {
    let Some(path) = denylist_path() else { return Vec::new() };
    let Ok(s) = std::fs::read_to_string(path) else { return Vec::new() };
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Machine-generated credential prefixes (provider keys, PATs, Slack tokens,
/// AWS key ids, JWTs). A hit redacts the whole token run when it is long
/// enough to be a credential rather than a word.
const TOKEN_PREFIXES: [&str; 12] =
    ["sk-", "gsk_", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "xoxs-", "AKIA", "AIza", "eyJ",
     "ABSKQmVk"]; // AWS Bedrock API keys base64-encode to an "ABSKQmVk…" prefix

/// Names whose `=`/`:` values are secrets by construction. Matched as a
/// suffix-insensitive scan (so `GITHUB_TOKEN=` and `--token=` both hit);
/// over-redacting a value is harmless, leaking one is not.
const SECRET_NAMES: [&str; 11] =
    ["password", "passwd", "pwd", "token", "secret", "api_key", "apikey", "access_key", "authorization",
     "api-key", "aws_bearer_token_bedrock"];

fn token_char(c: char, jwt: bool) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || (jwt && c == '.')
}

fn redact_token_prefixes(mut s: String) -> String {
    for pre in TOKEN_PREFIXES {
        let mut from = 0;
        while let Some(pos) = s[from..].find(pre).map(|p| p + from) {
            let boundary_ok = s[..pos]
                .chars()
                .next_back()
                .map(|c| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(true);
            let jwt = pre == "eyJ";
            let end = s[pos..]
                .char_indices()
                .find(|(_, c)| !token_char(*c, jwt))
                .map(|(o, _)| pos + o)
                .unwrap_or(s.len());
            if boundary_ok && end - pos >= 20 {
                s.replace_range(pos..end, REDACTED);
                from = pos + REDACTED.len();
            } else {
                from = pos + pre.len();
            }
        }
    }
    s
}

fn redact_assignments(mut s: String) -> String {
    for name in SECRET_NAMES {
        let mut from = 0;
        loop {
            let lower = s.to_ascii_lowercase();
            let Some(pos) = lower[from..].find(name).map(|p| p + from) else { break };
            let after = pos + name.len();
            let Some(sep) = s[after..].chars().next() else { break };
            if sep != '=' && sep != ':' {
                from = after;
                continue;
            }
            let mut val_start = after + 1;
            while s[val_start..].starts_with(' ') {
                val_start += 1;
            }
            // `Authorization: Bearer <tok>` — keep the scheme, redact the token.
            if s[val_start..].to_ascii_lowercase().starts_with("bearer ") {
                val_start += "bearer ".len();
            }
            let val_end = s[val_start..]
                .char_indices()
                .find(|(_, c)| c.is_whitespace())
                .map(|(o, _)| val_start + o)
                .unwrap_or(s.len());
            if val_end > val_start && &s[val_start..val_end] != REDACTED {
                s.replace_range(val_start..val_end, REDACTED);
                from = val_start + REDACTED.len();
            } else {
                from = val_end.max(after);
            }
        }
    }
    s
}

fn redact_url_credentials(mut s: String) -> String {
    let mut from = 0;
    while let Some(pos) = s[from..].find("://").map(|p| p + from) {
        let auth_start = pos + 3;
        let end = s[auth_start..]
            .char_indices()
            .find(|(_, c)| c.is_whitespace() || *c == '/')
            .map(|(o, _)| auth_start + o)
            .unwrap_or(s.len());
        let authority = &s[auth_start..end];
        if let Some(at) = authority.rfind('@') {
            if let Some(colon) = authority[..at].find(':') {
                let pw_start = auth_start + colon + 1;
                let pw_end = auth_start + at;
                if &s[pw_start..pw_end] != REDACTED {
                    s.replace_range(pw_start..pw_end, REDACTED);
                    from = pw_start + REDACTED.len();
                    continue;
                }
            }
        }
        from = auth_start;
    }
    s
}

/// Scrub one memory/history line before prompt injection: denylist literals,
/// known credential prefixes, `password=`/`token:`-style values, and
/// `user:pass@host` URL credentials. Always on — there is deliberately no
/// knob to disable it.
pub fn redact(line: &str) -> String {
    let mut s = line.to_string();
    for entry in denylist() {
        s = s.replace(&entry, REDACTED);
    }
    redact_url_credentials(redact_assignments(redact_token_prefixes(s)))
}

// ── System knowledge corpus (Axis B) ──────────────────────────────────────────

/// Read Mars's own docs from the working directory (present when run from the
/// repo, e.g. during eval) and split into retrievable chunks (by blank line).
/// Missing files are simply skipped — the registry/tuning corpus is always present.
fn doc_chunks() -> Vec<String> {
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
fn env_var_reference() -> Vec<String> {
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
    rank_scored(query, docs, k).into_iter().map(|(i, _)| i).collect()
}

/// [`rank`], keeping the BM25 scores — the substrate for metadata re-weighting.
pub fn rank_scored(query: &str, docs: &[String], k: usize) -> Vec<(usize, f64)> {
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
    scored.truncate(k);
    scored
}

/// Rank command memories: BM25 over the request text, then a gentle boost for
/// the user's *recent* and *same-directory* (same-project) commands — so that
/// when several stored exemplars tie lexically, the one from this project wins.
/// Records missing metadata (legacy or seeded eval stores: `ts`=0, empty `cwd`)
/// get no boost, so the eval's leakage-controlled stores rank purely lexically.
/// Lexical relevance still gates everything: a zero-score record never appears.
pub fn rank_memories(
    records: &[CommandMemory],
    query: &str,
    k: usize,
    cwd: &str,
    now: u64,
    cwd_boost: f64,
    recency_boost: f64,
    halflife_days: f64,
) -> Vec<usize> {
    let docs: Vec<String> = records.iter().map(|r| r.request.clone()).collect();
    let mut scored = rank_scored(query, &docs, docs.len());
    for (i, score) in scored.iter_mut() {
        let r = &records[*i];
        let mut boost = 1.0;
        if !cwd.is_empty() && r.cwd == cwd {
            boost += cwd_boost;
        }
        if r.ts > 0 && now >= r.ts && halflife_days > 0.0 {
            let age_days = (now - r.ts) as f64 / 86_400.0;
            boost += recency_boost * 0.5f64.powf(age_days / halflife_days);
        }
        *score *= boost;
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}

/// Memory-management reference lines for the system-knowledge corpus, so "what
/// do you remember about me / how do I make you forget" gets the real actions
/// and paths instead of a guess.
fn memory_reference() -> Vec<String> {
    [
        "To see or edit everything the agent remembers: run the 'open command memory' \
         action from the command bar — it opens ~/.mars/cmd_memory.jsonl in the editor; \
         delete lines to forget individual commands.",
        "To make the agent forget all remembered commands at once: run the 'forget all \
         commands' action (asks for confirmation), or delete ~/.mars/cmd_memory.jsonl.",
        "Memory and shell-history lines are redacted before they enter any LLM prompt \
         (API-key shapes, password=/token= values, URL credentials). To force-redact \
         additional strings, add them to ~/.mars/denylist (one per line) — the 'open \
         redaction denylist' action edits it.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
