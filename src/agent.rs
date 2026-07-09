/// LLM agent integration. Most providers speak the OpenAI-compatible chat API;
/// Claude uses Anthropic's own Messages API (separate branch in `chat()`).
/// Env precedence (paid-first): MARS_LLM_* (any endpoint, e.g. local Ollama;
/// legacy ARES_LLM_* still honored) → ANTHROPIC_API_KEY → OPENAI_API_KEY →
/// GROQ_API_KEY → GEMINI_API_KEY / GOOGLE_API_KEY.

use std::sync::mpsc;

/// What the model asked the editor to do (always user-confirmed before firing).
#[derive(Clone, Debug, PartialEq)]
pub enum AgentDirective {
    /// `RUN: <ActionName>` — an editor action from the registry.
    Run(String),
    /// `TYPE: <command>` — type a shell command into the terminal pane.
    Type(String),
    /// `OPEN: path:line` — open a file at a line (cited from a stack trace).
    Open(String),
    /// `NEED: <what>` — a read-side request for more context (W4/W5). Never gated;
    /// Mars re-asks once with the extra source. Not shown to the user.
    Need(NeedKind),
}

/// What extra context the model asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum NeedKind {
    /// Full terminal scrollback of the focused pane (W5).
    Scrollback,
    /// Another tab's panes, by tab name (W4).
    Tab(String),
}

pub enum AgentEvent {
    Answer {
        text: String,
        directive: Option<AgentDirective>,
    },
    /// Background tab-naming reply (tab id, proposed name).
    AutoName { tab_id: usize, name: String },
    /// Background session-naming reply (proposed name).
    SessionName { name: String },
    /// W6: one-line verdict on a watched terminal (term id, verdict).
    WatchSummary { term_id: usize, verdict: String },
    /// A background agent thread finished — clears the `bg_busy` gate even if the
    /// call failed (so one failed request can't wedge all background work).
    BgDone,
    /// W3 shell translate: English → one shell command (fills the SH bar).
    ShellTranslation { command: String, call_id: u64 },
    Error(String),
}

/// Kebab-case, ≤16 chars, alnum+dash — shared by tab/session auto-naming.
fn kebab(text: &str) -> String {
    let s: String = text
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    s.chars().take(16).collect()
}

/// Match a single directive line, tolerating markdown noise (`- `, backticks,
/// bold) the model sometimes wraps it in.
fn match_directive(line: &str) -> Option<AgentDirective> {
    let l = line
        .trim()
        .trim_start_matches(['-', '*', '>', ' '])
        .trim_matches('`')
        .trim_matches('*')
        .trim();
    // RUN takes only the action token (models sometimes append prose); TYPE and
    // OPEN keep the full rest (commands and paths contain spaces).
    if let Some(rest) = l.strip_prefix("RUN:") {
        if let Some(name) = rest.trim().trim_matches('`').split_whitespace().next() {
            let name = name.trim_end_matches(['.', ',', ':']);
            if !name.is_empty() {
                return Some(AgentDirective::Run(name.to_string()));
            }
        }
    }
    // NEED: read-side context request (W4/W5).
    if let Some(rest) = l.strip_prefix("NEED:") {
        let arg = rest.trim().trim_matches('`').trim();
        let low = arg.to_lowercase();
        if low.starts_with("scrollback") || low.starts_with("history") {
            return Some(AgentDirective::Need(NeedKind::Scrollback));
        }
        if let Some(tab) = low.strip_prefix("tab") {
            let name = tab.trim().to_string();
            if !name.is_empty() {
                return Some(AgentDirective::Need(NeedKind::Tab(name)));
            }
        }
    }
    for (tag, make) in [
        ("TYPE:", AgentDirective::Type as fn(String) -> AgentDirective),
        ("OPEN:", AgentDirective::Open as fn(String) -> AgentDirective),
    ] {
        if let Some(rest) = l.strip_prefix(tag) {
            let arg = rest.trim().trim_matches('`').trim().to_string();
            if !arg.is_empty() {
                return Some(make(arg));
            }
        }
    }
    None
}

/// Split a model reply into display text and a trailing directive. Lenient: the
/// directive may sit on any of the last few non-empty lines (models sometimes
/// add a sign-off after it), and may be wrapped in backticks/list markers.
pub fn parse_directive(text: &str) -> (String, Option<AgentDirective>) {
    let lines: Vec<&str> = text.lines().collect();
    // Scan the last few non-empty lines from the bottom.
    let mut hit: Option<usize> = None;
    for (i, line) in lines.iter().enumerate().rev().take(4) {
        if line.trim().is_empty() {
            continue;
        }
        if match_directive(line).is_some() {
            hit = Some(i);
            break;
        }
    }
    match hit {
        Some(i) => {
            let directive = match_directive(lines[i]);
            let display = lines[..i].join("\n").trim_end().to_string();
            (display, directive)
        }
        None => (text.to_string(), None),
    }
}

pub struct AgentConfig {
    pub url: String,
    pub key: String,
    pub model: String,
    pub provider: &'static str,
    pub max_tokens: u32,
    pub temperature: f64,
    /// Set on a remote box behind an `ssh -R` auth socket: the agent proxies the
    /// LLM call home instead of holding a key. `None` = call the provider directly.
    pub broker_sock: Option<String>,
}

/// Read `MARS_<name>`, falling back to the pre-rename `ARES_<name>`.
fn env_var(name: &str) -> Result<String, std::env::VarError> {
    std::env::var(format!("MARS_{name}")).or_else(|_| std::env::var(format!("ARES_{name}")))
}

impl AgentConfig {
    pub fn from_env() -> Self {
        // Highest precedence: a forwarded auth socket (we're on a remote box).
        // Proxy the call home — the key never lands here. An explicit MARS_LLM_KEY
        // still wins over this, so a box you deliberately keyed keeps working.
        if std::env::var("MARS_LLM_KEY").is_err()
            && std::env::var("ARES_LLM_KEY").is_err()
        {
            if let Some(sock) = crate::broker::detect_broker_sock() {
                return AgentConfig {
                    url: String::new(),
                    key: String::new(),
                    // None-equivalent: empty → the broker picks its own model.
                    model: env_var("LLM_MODEL").unwrap_or_default(),
                    provider: "broker",
                    max_tokens: 512,
                    temperature: 0.3,
                    broker_sock: Some(sock),
                };
            }
        }
        // Provider detection, paid-first: explicit MARS_LLM_KEY wins, then a set
        // Claude/OpenAI key (you meant it), then the free Groq/Gemini tiers.
        // Cheap defaults per provider (right-size, don't reach for the biggest);
        // override any with MARS_LLM_MODEL.
        let (key, provider, default_url, default_model) =
            if let Ok(k) = env_var("LLM_KEY") {
                (k, "custom", "https://api.groq.com/openai/v1", "llama-3.1-8b-instant")
            } else if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
                // Claude — Anthropic's own Messages API (not OpenAI-compatible;
                // handled in chat()). Haiku is the cheap/fast default.
                (k, "anthropic", "https://api.anthropic.com", "claude-haiku-4-5")
            } else if let Ok(k) = std::env::var("OPENAI_API_KEY") {
                (k, "openai", "https://api.openai.com/v1", "gpt-4o-mini")
            } else if let Ok(k) = std::env::var("GROQ_API_KEY") {
                // Qwen3-32B: strong open model on Groq's fast free tier.
                (k, "groq", "https://api.groq.com/openai/v1", "qwen/qwen3-32b")
            } else if let Ok(k) =
                std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY"))
            {
                (
                    k,
                    "gemini",
                    "https://generativelanguage.googleapis.com/v1beta/openai",
                    // Flash-Lite: cheapest + highest free-tier limits. Override
                    // with MARS_LLM_MODEL. (Pinned dated versions age out of the
                    // free tier, so track the lite line.)
                    "gemini-3.1-flash-lite",
                )
            } else {
                (String::new(), "none", "https://api.groq.com/openai/v1", "llama-3.1-8b-instant")
            };

        // Explicit URL/model overrides apply to any provider.
        let url = env_var("LLM_URL").unwrap_or_else(|_| default_url.to_string());
        let model = env_var("LLM_MODEL").unwrap_or_else(|_| default_model.to_string());
        AgentConfig { url, key, model, provider, max_tokens: 512, temperature: 0.3, broker_sock: None }
    }

    pub fn is_configured(&self) -> bool {
        if self.provider == "broker" {
            // Honest on the remote: "configured" iff the tunnel is actually up.
            return self
                .broker_sock
                .as_deref()
                .map(|s| std::os::unix::net::UnixStream::connect(s).is_ok())
                .unwrap_or(false);
        }
        !self.key.is_empty()
    }
}

fn system_prompt(registry: &str, screen: &str) -> String {
    format!(
        "You are the assistant inside Mars, a terminal editor + multiplexer. \
         Be terse: 1-3 sentences, no preamble, no restating the question. When \
         triaging a failure, say what failed and why, then act — do NOT write an \
         essay. Always prefer ending with a concrete action over explaining.\n\
         You can act, always with user confirmation, by ending your reply with \
         EXACTLY ONE directive on its own final line:\n\
         RUN: <ActionName>      — run an editor action (e.g. RUN: SplitVertical)\n\
         TYPE: <shell command>  — type a command into the user's terminal pane \
         (e.g. TYPE: git status). Prefer TYPE for anything a shell does.\n\
         OPEN: path:line        — open a file at a line, e.g. OPEN: src/main.rs:42. \
         Use this to jump to the exact line a stack trace or error points at.\n\
         If the visible screen is not enough, ask for more instead of guessing, using \
         EXACTLY one of:\n\
         NEED: scrollback       — the focused terminal's full history (e.g. \"when did \
         this first fail?\").\n\
         NEED: tab <name>       — another tab's panes. You'll be re-asked automatically \
         with it; do not apologize, just request.\n\
         Available editor actions:\n{registry}\n\n\
         LIVE SCREEN (what the user is looking at right now — ground your answers \
         in it; you may reference file contents, terminal output, errors):\n{screen}"
    )
}

/// Build the chat messages: system + up to the last 12 conversation turns +
/// the new question. Extracted so tests can assert history is really sent.
pub fn build_messages(
    registry: &str,
    screen: &str,
    history: &[(String, String)],
    question: &str,
) -> Vec<serde_json::Value> {
    let mut messages = vec![serde_json::json!({
        "role": "system", "content": system_prompt(registry, screen)
    })];
    let start = history.len().saturating_sub(12);
    for (role, content) in &history[start..] {
        messages.push(serde_json::json!({ "role": role, "content": content }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": question }));
    messages
}

/// Spawn a background thread, POST to OpenAI-compatible chat endpoint, send result back.
pub fn ask(
    cfg: AgentConfig,
    question: String,
    registry: String,
    screen: String,
    history: Vec<(String, String)>,
    tx: mpsc::Sender<AgentEvent>,
) {
    std::thread::spawn(move || {
        // Memory (Axis B): retrieve from Mars's OWN knowledge (docs + tunable knobs)
        // so the agent can answer "how do I…" and propose self-reconfiguration
        // accurately. The action registry is already in the base prompt.
        let mode = crate::retrieval::MemoryMode::from_env();
        let mut messages = build_messages(&registry, &screen, &history, &question);
        if mode.includes_docs() {
            if let Some(ctx) = docs_context(&question) {
                messages.insert(1, serde_json::json!({ "role": "system", "content": ctx }));
            }
        }
        match chat_with_id(&cfg, messages, "ask", mode.as_str()) {
            Ok((text, _call_id)) => {
                let (display, directive) = parse_directive(&text);
                let _ = tx.send(AgentEvent::Answer { text: display, directive });
            }
            Err(e) => {
                let _ = tx.send(AgentEvent::Error(e.to_string()));
            }
        }
    });
}

/// Retrieve the most relevant Mars docs + tuning-knob descriptions for `question`,
/// formatted as a system-context block, or None if nothing is relevant.
fn docs_context(question: &str) -> Option<String> {
    use crate::retrieval;
    let mut corpus = retrieval::doc_chunks();
    corpus.extend(crate::tuning::knob_descriptions());
    corpus.extend(retrieval::env_var_reference());
    let hits = retrieval::rank(question, &corpus, 5);
    if hits.is_empty() {
        return None;
    }
    let body = hits.iter().map(|&i| format!("- {}", corpus[i])).collect::<Vec<_>>().join("\n");
    // Frame it: for a how-to / configuration question, ANSWER by explaining the
    // exact setting/keybinding/variable and where to set it — do not emit a RUN
    // directive. This is what fixes the "[would run: X]" non-answers.
    Some(format!(
        "Use this Mars reference to answer the user's how-to/configuration question directly — \
         name the exact keybinding, setting (with its file), or environment variable, and how to \
         change it. Do not propose a RUN action for a how-to question.\n{body}"
    ))
}

/// Background tab-naming: tiny prompt, no registry, quiet failure.
pub fn auto_name(cfg: AgentConfig, tab_id: usize, screen: String, tx: mpsc::Sender<AgentEvent>) {
    std::thread::spawn(move || {
        let messages = vec![
            serde_json::json!({ "role": "system", "content":
                "Name this terminal workspace tab from its visible content. Reply with \
                 ONLY a 1-3 word kebab-case label (e.g. rust-build, api-notes, logs). \
                 No punctuation, no explanation." }),
            serde_json::json!({ "role": "user", "content": screen }),
        ];
        if let Ok(text) = chat(&cfg, messages, "auto-name") {
            let name = kebab(&text);
            if !name.is_empty() {
                let _ = tx.send(AgentEvent::AutoName { tab_id, name });
            }
        }
        let _ = tx.send(AgentEvent::BgDone); // release the gate even on failure
    });
}

/// W6: one-line verdict on a watched terminal that went quiet or exited. Runs on
/// a background thread (even inside the detached daemon) and delivers a Notice.
pub fn watch_summary(
    cfg: AgentConfig,
    term_id: usize,
    reason: crate::app::WatchReason,
    tail: String,
    tx: mpsc::Sender<AgentEvent>,
) {
    std::thread::spawn(move || {
        let hint = match reason {
            crate::app::WatchReason::Exit => "The process just exited.",
            crate::app::WatchReason::Quiet => "The output has gone quiet (it may still be running).",
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": format!(
                "You watch a terminal for the user. {hint} In ONE short line, say whether it \
                 succeeded or failed and the single most important reason. Start with a verb \
                 or 'failed:'/'done:'. No preamble, no markdown.") }),
            serde_json::json!({ "role": "user", "content": tail }),
        ];
        match chat(&cfg, messages, "watch") {
            Ok(text) => {
                let verdict = text.trim().lines().next().unwrap_or("").trim().to_string();
                if !verdict.is_empty() {
                    let _ = tx.send(AgentEvent::WatchSummary { term_id, verdict });
                }
            }
            // Surface the failure instead of going silent (and clear the gate).
            Err(e) => {
                let _ = tx.send(AgentEvent::WatchSummary {
                    term_id,
                    verdict: format!("⚠ watch couldn't summarize — {e}"),
                });
            }
        }
        let _ = tx.send(AgentEvent::BgDone); // always release the gate
    });
}

/// Background session-naming — like tab naming but for the whole session.
pub fn name_session(cfg: AgentConfig, screen: String, tx: mpsc::Sender<AgentEvent>) {
    std::thread::spawn(move || {
        let messages = vec![
            serde_json::json!({ "role": "system", "content":
                "Name this terminal session from what the user is doing. Reply with \
                 ONLY a 1-2 word kebab-case label (e.g. mars-dev, deploy, db-migrate). \
                 No punctuation, no explanation." }),
            serde_json::json!({ "role": "user", "content": screen }),
        ];
        if let Ok(text) = chat(&cfg, messages, "session-name") {
            let name = kebab(&text);
            if !name.is_empty() {
                let _ = tx.send(AgentEvent::SessionName { name });
            }
        }
        let _ = tx.send(AgentEvent::BgDone); // release the gate even on failure
    });
}

/// W3: turn an English request into ONE shell command (no prose, no fences).
/// ALWAYS sends exactly one event so the caller's spinner can never wedge.
/// Synchronous shell translation with memory retrieval + logging. Returns the
/// extracted command and its `call_id`. Shared by the async composer and the
/// headless `mars translate` primitive the Python eval harness drives.
///
/// Memory (Axis A): when the active [`crate::retrieval::MemoryMode`] includes
/// history, the user's own past `(request → command)` pairs + recent shell history
/// are retrieved and shown as few-shot examples — the "sits at the terminal"
/// advantage a standalone translator can't have. The variant is logged.
pub fn translate_once(cfg: &AgentConfig, request: &str, screen: &str) -> anyhow::Result<(String, u64)> {
    let mode = crate::retrieval::MemoryMode::from_env();
    let examples = if mode.includes_history() { command_fewshot(request) } else { String::new() };
    let system = format!(
        "You convert an English request into ONE shell command. Output the command \
         and nothing else — no explanation, no markdown, no backticks, no leading $. \
         Use the visible screen for context (cwd, filenames) when relevant. If the \
         request is already a shell command, return it unchanged. Keep any internal \
         reasoning to at most 50 tokens, then output only the command.{}",
        if examples.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nThis user's own commands (prefer their conventions — e.g. a \
                 project alias/script over a generic equivalent):\n{examples}"
            )
        }
    );
    let messages = vec![
        serde_json::json!({ "role": "system", "content": system }),
        serde_json::json!({ "role": "user", "content": format!("SCREEN:\n{screen}\n\nREQUEST: {request}") }),
    ];
    let (text, call_id) = chat_with_id(cfg, messages, "shell", mode.as_str())?;
    let command = text
        .trim()
        .trim_matches('`')
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .trim_start_matches("$ ")
        .to_string();
    Ok((command, call_id))
}

pub fn translate_shell(cfg: AgentConfig, request: String, screen: String, tx: mpsc::Sender<AgentEvent>) {
    std::thread::spawn(move || {
        let ev = match translate_once(&cfg, &request, &screen) {
            Ok((command, _)) if command.is_empty() => {
                AgentEvent::Error("couldn't translate that — rephrase and retry".into())
            }
            Ok((command, call_id)) => AgentEvent::ShellTranslation { command, call_id },
            Err(e) => AgentEvent::Error(e.to_string()),
        };
        let _ = tx.send(ev);
    });
}

/// Format the top few retrieved command-memory / shell-history entries for `request`
/// as few-shot lines, or "" if memory is empty. Command-memory pairs (request →
/// command) rank ahead of bare history commands.
fn command_fewshot(request: &str) -> String {
    use crate::retrieval;
    let mem = retrieval::load_command_memory(); // (request, command)
    let hist = retrieval::shell_history(500); // recent commands
    // Rank memory pairs by their request text; history by the command text.
    let mem_docs: Vec<String> = mem.iter().map(|(r, _)| r.clone()).collect();
    let mut lines: Vec<String> = retrieval::rank(request, &mem_docs, 3)
        .into_iter()
        .map(|i| format!("- {} → {}", mem[i].0, mem[i].1))
        .collect();
    for i in retrieval::rank(request, &hist, 5) {
        let cmd = &hist[i];
        if !lines.iter().any(|l| l.contains(cmd.as_str())) {
            lines.push(format!("- {cmd}"));
        }
    }
    lines.truncate(6);
    lines.join("\n")
}

/// Extract "retry in 14.89s"-style hints from a 429 message → whole seconds.
pub fn retry_secs(msg: &str) -> Option<u64> {
    let after = msg.split("retry in ").nth(1)?;
    let num: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    num.parse::<f64>().ok().map(|s| s.ceil() as u64)
}

/// Strip `<think>…</think>` reasoning blocks (Qwen3, DeepSeek-R1, etc.) so only
/// the user-facing answer + directive remain.
fn strip_reasoning(text: &str) -> String {
    let mut out = text.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if a < b {
            out.replace_range(a..b + "</think>".len(), "");
        } else {
            break;
        }
    }
    // A dangling, unclosed <think> → drop everything from it.
    if let Some(a) = out.find("<think>") {
        out.truncate(a);
    }
    out.trim().to_string()
}

/// Single choke point for every LLM call. `task` tags the call; `retrieval` names
/// the memory variant that shaped the prompt ("n/a" when the path doesn't retrieve).
/// Times the call, captures real token usage, logs it under a fresh `call_id` when
/// MARS_LLM_DEBUG is on, and returns `(reasoning-stripped reply, call_id)`. The
/// call_id lets a later behavioral outcome (accept/edit/reject) be correlated.
pub fn chat_with_id(
    cfg: &AgentConfig,
    messages: Vec<serde_json::Value>,
    task: &str,
    retrieval: &str,
) -> anyhow::Result<(String, u64)> {
    // Remote box: proxy the whole call home over the forwarded socket. No key,
    // no Authorization header, ever constructed here. (Logged home-side.)
    if cfg.provider == "broker" {
        let sock = cfg
            .broker_sock
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("broker mode with no socket"))?;
        return crate::broker::chat_via_broker(sock, cfg, messages).map(|t| (t, 0));
    }

    let call_id = crate::llm_log::next_call_id();
    let start = std::time::Instant::now();
    let result = if cfg.provider == "anthropic" {
        chat_anthropic(cfg, &messages)
    } else {
        chat_openai(cfg, &messages)
    };
    let latency_ms = start.elapsed().as_millis() as u64;

    match &result {
        Ok((text, pt, ct)) => {
            crate::llm_log::record(&crate::llm_log::CallRecord {
                call_id, task, provider: cfg.provider, model: &cfg.model, retrieval,
                prompt_tokens: *pt, completion_tokens: *ct, latency_ms,
                ok: true, error: None, input: &messages, output: text,
            });
            Ok((strip_reasoning(text), call_id))
        }
        Err(e) => {
            let msg = e.to_string();
            crate::llm_log::record(&crate::llm_log::CallRecord {
                call_id, task, provider: cfg.provider, model: &cfg.model, retrieval,
                prompt_tokens: 0, completion_tokens: 0, latency_ms,
                ok: false, error: Some(&msg), input: &messages, output: "",
            });
            Err(anyhow::anyhow!("{msg}"))
        }
    }
}

/// Convenience wrapper for callers that don't need the `call_id` correlation
/// (watch / auto-name / session-name / remote) and don't retrieve.
pub fn chat(cfg: &AgentConfig, messages: Vec<serde_json::Value>, task: &str) -> anyhow::Result<String> {
    chat_with_id(cfg, messages, task, "n/a").map(|(text, _)| text)
}

/// OpenAI-compatible providers (OpenAI, Groq, Gemini's OpenAI shim, custom,
/// Ollama). Returns (raw_text, prompt_tokens, completion_tokens).
fn chat_openai(cfg: &AgentConfig, messages: &[serde_json::Value]) -> anyhow::Result<(String, u64, u64)> {
    let url = format!("{}/chat/completions", cfg.url);
    let body = serde_json::json!({
        "model": cfg.model,
        "messages": messages,
        "max_tokens": cfg.max_tokens,
        "temperature": cfg.temperature
    });

    // Bound the call so a stalled connection surfaces as an error instead of
    // hanging the agent (and the spinner) forever.
    let resp = match ureq::post(&url)
        .timeout(std::time::Duration::from_secs(30))
        .set("Authorization", &format!("Bearer {}", cfg.key))
        .set("Content-Type", "application/json")
        .send_json(body)
    {
        Ok(r) => r,
        // Pull the real message out of the error body (bad key, quota, etc.).
        // Gemini wraps it in a JSON array: [{"error":{"message": …}}].
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let api_msg = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|j| {
                let node = if j.is_array() { j[0].clone() } else { j };
                node["error"]["message"].as_str().map(str::to_string)
            });
            let msg = match code {
                429 => match api_msg.as_deref().and_then(retry_secs) {
                    Some(s) => format!(
                        "rate limit reached — wait ~{s}s and retry (free tier). \
                         Tip: raise limits, switch model with MARS_LLM_MODEL, or use \
                         GROQ_API_KEY / a local Ollama via MARS_LLM_URL."
                    ),
                    None => "rate limit reached (free tier) — wait ~30s and retry, or \
                             switch model/provider (MARS_LLM_MODEL / GROQ_API_KEY / \
                             MARS_LLM_URL for local Ollama)."
                        .to_string(),
                },
                401 | 403 => format!(
                    "auth failed — check your API key. ({})",
                    api_msg.as_deref().unwrap_or("invalid credentials")
                ),
                _ => api_msg.unwrap_or_else(|| format!("HTTP {code}")),
            };
            anyhow::bail!("{msg}");
        }
        Err(e) => anyhow::bail!("{e}"),
    };

    let json: serde_json::Value = resp.into_json()?;
    if let Some(msg) = json["error"]["message"].as_str() {
        anyhow::bail!("{msg}");
    }
    let text = json["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    let pt = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let ct = json["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    Ok((text, pt, ct))
}

/// Anthropic Messages API — NOT OpenAI-compatible: system is a top-level field
/// (not a message role), auth is `x-api-key` + `anthropic-version`, the reply is
/// an array of content blocks, and usage is input/output_tokens.
fn chat_anthropic(cfg: &AgentConfig, messages: &[serde_json::Value]) -> anyhow::Result<(String, u64, u64)> {
    // Split the system message(s) out of the OpenAI-style array.
    let mut system = String::new();
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    for m in messages {
        if m["role"].as_str() == Some("system") {
            if !system.is_empty() {
                system.push('\n');
            }
            system.push_str(m["content"].as_str().unwrap_or(""));
        } else {
            msgs.push(m.clone());
        }
    }
    let url = format!("{}/v1/messages", cfg.url);
    let body = serde_json::json!({
        "model": cfg.model,
        "max_tokens": cfg.max_tokens,
        "system": system,
        "messages": msgs,
        "temperature": cfg.temperature
    });
    let resp = match ureq::post(&url)
        .timeout(std::time::Duration::from_secs(30))
        .set("x-api-key", &cfg.key)
        .set("anthropic-version", "2023-06-01")
        .set("Content-Type", "application/json")
        .send_json(body)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let api_msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|j| j["error"]["message"].as_str().map(str::to_string));
            let msg = match code {
                429 => "rate limit reached (Anthropic) — wait and retry, or switch model \
                        with MARS_LLM_MODEL."
                    .to_string(),
                401 | 403 => format!(
                    "auth failed — check ANTHROPIC_API_KEY. ({})",
                    api_msg.as_deref().unwrap_or("invalid credentials")
                ),
                _ => api_msg.unwrap_or_else(|| format!("HTTP {code}")),
            };
            anyhow::bail!("{msg}");
        }
        Err(e) => anyhow::bail!("{e}"),
    };
    let json: serde_json::Value = resp.into_json()?;
    if let Some(msg) = json["error"]["message"].as_str() {
        anyhow::bail!("{msg}");
    }
    // content is an array of blocks; concatenate the text ones.
    let text = json["content"]
        .as_array()
        .map(|blocks| blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""))
        .unwrap_or_default();
    let pt = json["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let ct = json["usage"]["output_tokens"].as_u64().unwrap_or(0);
    Ok((text, pt, ct))
}
