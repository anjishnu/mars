//! Model-tier ring — route each agent task to the cheapest model tier that still
//! does the job well. The premise (see §compute in the eval): agent tasks are not
//! equally hard. Auto-naming a tab or a session is trivial; translating NL→shell
//! needs a competent coder; open-ended `ask` wants a reasoner. Sending every task
//! to one model over-pays on the easy ones and under-serves the hard ones.
//!
//! The ring names three tiers — `low`/`mid`/`high` — maps each *task class* to a
//! tier, and each tier to a concrete model *per provider*. It is fully
//! config-editable at `~/.config/mars/tiers.json` (written with annotated defaults
//! on first run), so an operator — or the agent asked to reconfigure itself — can
//! re-point a whole tier or move a task between tiers in one edit.
//!
//! Precedence: an explicit `MARS_LLM_MODEL` always wins (a deliberate model choice
//! is never second-guessed — this is also what keeps the eval pinned to one model).
//! Otherwise the ring resolves `task → tier → model` for the active provider; an
//! unmapped task or provider falls back to the provider default unchanged.
//!
//! Two runtime moves complete the cascade (both disabled by an explicit model pin):
//! *rotation for limits* — a rate-limited call retries on another keyed provider's
//! model for the same tier (`agent::rotation_candidates`) — and *escalation for
//! quality* — an `ask` reply whose RUN: directive fails the registry check is
//! retried once on the model one tier up (`model_above`).

use std::collections::BTreeMap;

/// task-class → tier name, and provider → (tier name → model).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Tiers {
    /// Which tier each agent task class runs on.
    pub task_tier: BTreeMap<String, String>,
    /// Concrete model per (provider, tier) — so the same tier map works whichever
    /// provider key is present.
    pub tiers: BTreeMap<String, BTreeMap<String, String>>,
}

fn m(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

impl Default for Tiers {
    fn default() -> Self {
        Tiers {
            // Trivial, high-frequency labeling → low; NL→shell → mid; open-ended
            // reasoning/knowledge → high.
            task_tier: m(&[
                ("auto_name", "low"),
                ("name_session", "low"),
                ("mission", "low"),
                ("watch", "mid"),
                // The batched shift-report polish: tier-0 heuristics already
                // classified every row; the model only phrases one-liners.
                ("shift_batch", "low"),
                ("translate", "mid"),
                ("ask", "high"),
            ]),
            tiers: [
                // Groq free/cheap open-weight ladder (8B → 32B → 70B).
                ("groq", m(&[
                    ("low", "llama-3.1-8b-instant"),
                    ("mid", "qwen/qwen3-32b"),
                    ("high", "llama-3.3-70b-versatile"),
                ])),
                ("anthropic", m(&[
                    ("low", "claude-haiku-4-5"),
                    ("mid", "claude-sonnet-5"),
                    ("high", "claude-opus-4-8"),
                ])),
                ("openai", m(&[
                    ("low", "gpt-4o-mini"),
                    ("mid", "gpt-4o-mini"),
                    ("high", "gpt-4o"),
                ])),
                ("gemini", m(&[
                    ("low", "gemini-3.1-flash-lite"),
                    ("mid", "gemini-3.1-flash-lite"),
                    ("high", "gemini-3.1-flash"),
                ])),
                // Bedrock cross-region inference profiles (modelIds are stable,
                // so a real ladder works). Azure has no default block: its
                // "models" are user-named deployments, so it falls through to the
                // single configured deployment unless the user adds tiers.json.
                ("bedrock", m(&[
                    ("low", "us.anthropic.claude-3-5-haiku-20241022-v1:0"),
                    ("mid", "us.anthropic.claude-sonnet-4-20250514-v1:0"),
                    ("high", "us.anthropic.claude-opus-4-20250514-v1:0"),
                ])),
            ]
            .into_iter()
            .map(|(p, t)| (p.to_string(), t))
            .collect(),
        }
    }
}

fn tiers_path() -> Option<std::path::PathBuf> {
    crate::config::state_path().map(|p| p.with_file_name("tiers.json"))
}

/// Load the ring, writing annotated defaults on first run so the file is
/// discoverable and editable. Malformed/partial files fall back to defaults.
pub fn load() -> Tiers {
    let Some(path) = tiers_path() else { return Tiers::default() };
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(t) = serde_json::from_str::<Tiers>(&s) {
            return t;
        }
    }
    let def = Tiers::default();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&def) {
            let _ = std::fs::write(&path, json);
        }
    }
    def
}

/// Resolve the model to use for `task` under `provider`. An explicit
/// `MARS_LLM_MODEL`/`ARES_LLM_MODEL` overrides the ring entirely; otherwise
/// `task → tier → model`, falling back to `default_model` when unmapped.
pub fn model_for(provider: &str, task: &str, default_model: &str) -> String {
    if std::env::var("MARS_LLM_MODEL").is_ok() || std::env::var("ARES_LLM_MODEL").is_ok() {
        return default_model.to_string();
    }
    let t = load();
    let Some(tier) = t.task_tier.get(task) else { return default_model.to_string() };
    t.tiers
        .get(provider)
        .and_then(|m| m.get(tier))
        .cloned()
        .unwrap_or_else(|| default_model.to_string())
}

const TIER_ORDER: [&str; 3] = ["low", "mid", "high"];

fn model_above_in(t: &Tiers, provider: &str, task: &str) -> Option<String> {
    let tier = t.task_tier.get(task)?;
    let models = t.tiers.get(provider)?;
    let current = models.get(tier)?;
    let start = TIER_ORDER.iter().position(|x| *x == tier.as_str())?;
    // Walk past tiers repointed to the same model — escalating to the model
    // that just failed would be a no-op (e.g. openai's low and mid coincide).
    for next in &TIER_ORDER[start + 1..] {
        if let Some(m) = models.get(*next) {
            if m != current {
                return Some(m.clone());
            }
        }
    }
    None
}

/// The model one tier above `task`'s tier for `provider` — the escalation
/// target when a cheap-tier reply fails validation. None when the task is
/// unmapped, already served by the top tier's model, or the user pinned a
/// model explicitly (a deliberate choice is never second-guessed).
pub fn model_above(provider: &str, task: &str) -> Option<String> {
    if std::env::var("MARS_LLM_MODEL").is_ok() || std::env::var("ARES_LLM_MODEL").is_ok() {
        return None;
    }
    model_above_in(&load(), provider, task)
}

/// Retrieval lines describing the ring, so the agent answers "which model runs X"
/// / "how do I change the model for translate" with the real file + tier, not a
/// hallucinated knob. Mirrors `tuning::knob_descriptions`.
#[cfg_attr(not(feature = "memory"), allow(dead_code))] // sole consumer is the docs corpus
pub fn tier_descriptions() -> Vec<String> {
    let t = Tiers::default();
    let mut out = vec![
        "The model tier ring routes each agent task to a tier (low/mid/high); edit \
         ~/.config/mars/tiers.json to move a task between tiers or re-point a tier to \
         a different model. An explicit MARS_LLM_MODEL overrides the ring."
            .to_string(),
    ];
    for (task, tier) in &t.task_tier {
        out.push(format!(
            "The `{task}` task runs on the `{tier}` tier by default; change it under \
             `task_tier` in ~/.config/mars/tiers.json."
        ));
    }
    out.push(
        "On a provider rate limit (HTTP 429) the agent rotates the call to another \
         configured provider's model for the same tier — set more than one API key \
         (e.g. GROQ_API_KEY and GEMINI_API_KEY) to enable rotation; an explicit \
         MARS_LLM_MODEL disables it."
            .to_string(),
    );
    out.push(
        "If an ask reply proposes an action that fails the registry check, the \
         question is retried once on the model one tier up (logged as task \
         `ask_escalated`); an explicit MARS_LLM_MODEL disables escalation."
            .to_string(),
    );
    out
}
