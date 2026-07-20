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

/// task-class → tier name, and provider → (tier name → ordered model candidates).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Tiers {
    /// Which tier each agent task class runs on.
    pub task_tier: BTreeMap<String, String>,
    /// Ordered model candidates per (provider, tier). Listing more than one gives
    /// in-tier fallback: a retired or rate-limited model falls through to the next
    /// in its OWN tier before the call leaves the provider. A bare string is still
    /// accepted (back-compat with single-model files).
    #[serde(deserialize_with = "de_model_lists")]
    pub tiers: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

/// Accept either `"model"` or `["model", "fallback", …]` for a tier's value, so a
/// tiers.json written before the list format still loads.
fn de_model_lists<'de, D>(
    d: D,
) -> std::result::Result<BTreeMap<String, BTreeMap<String, Vec<String>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let raw: BTreeMap<String, BTreeMap<String, OneOrMany>> =
        serde::Deserialize::deserialize(d)?;
    Ok(raw
        .into_iter()
        .map(|(provider, tier_map)| {
            let tier_map = tier_map
                .into_iter()
                .map(|(tier, v)| {
                    (tier, match v {
                        OneOrMany::One(s) => vec![s],
                        OneOrMany::Many(list) => list,
                    })
                })
                .collect();
            (provider, tier_map)
        })
        .collect())
}

/// The shipped ring. Lives as DATA in `tiers_default.json` (edit that, not Rust)
/// and is embedded at compile time — the same pattern as the model prompts. The
/// runtime override at `~/.config/mars/tiers.json` is merged over this on load.
/// Refresh the model lists when a provider churns models, e.g. Groq:
///   curl -sH "Authorization: Bearer $GROQ_API_KEY" https://api.groq.com/openai/v1/models
const DEFAULT_RING_JSON: &str = include_str!("tiers_default.json");

impl Default for Tiers {
    fn default() -> Self {
        // Parse the embedded data file. It ships with the binary, so a parse
        // failure is an authoring bug in tiers_default.json — pinned by the
        // selfcheck ("tier ring" block), which fails loudly if this ever breaks.
        serde_json::from_str(DEFAULT_RING_JSON)
            .expect("tiers_default.json is malformed — fix the embedded ring")
    }
}

fn tiers_path() -> Option<std::path::PathBuf> {
    crate::config::state_path().map(|p| p.with_file_name("tiers.json"))
}

/// Load the ring, writing annotated defaults on first run so the file is
/// discoverable and editable. Malformed/partial files fall back to defaults.
pub fn load() -> Tiers {
    let mut merged = Tiers::default();
    let Some(path) = tiers_path() else { return merged };
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(t) = serde_json::from_str::<Tiers>(&s) {
            // Overlay the file so an operator's edits win — but a partial or stale
            // file must never DROP a mapping the defaults added. A task missing
            // from an old file would otherwise fall through to the provider default
            // model, which is exactly how a retired default silently broke every
            // unmapped task (shift_brief / mission / capture_goals) for days.
            for (task, tier) in t.task_tier {
                merged.task_tier.insert(task, tier);
            }
            for (provider, tier_map) in t.tiers {
                let entry = merged.tiers.entry(provider).or_default();
                for (tier, models) in tier_map {
                    entry.insert(tier, models);
                }
            }
            return merged;
        }
    }
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&merged) {
            let _ = std::fs::write(&path, json);
        }
    }
    merged
}

/// Resolve the model to use for `task` under `provider`. An explicit
/// `MARS_LLM_MODEL`/`ARES_LLM_MODEL` overrides the ring entirely; otherwise
/// `task → tier → model`, falling back to `default_model` when unmapped.
pub fn models_for(provider: &str, task: &str, default_model: &str) -> Vec<String> {
    if std::env::var("MARS_LLM_MODEL").is_ok() || std::env::var("ARES_LLM_MODEL").is_ok() {
        return vec![default_model.to_string()];
    }
    let t = load();
    let Some(tier) = t.task_tier.get(task) else { return vec![default_model.to_string()] };
    t.tiers
        .get(provider)
        .and_then(|m| m.get(tier))
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| vec![default_model.to_string()])
}

/// The single best model for `task` — the head of [`models_for`]. Kept for
/// callers/tests that want one string; the runtime uses the whole list so a
/// retired head falls through to the next candidate in the same tier.
pub fn model_for(provider: &str, task: &str, default_model: &str) -> String {
    models_for(provider, task, default_model)
        .into_iter()
        .next()
        .unwrap_or_else(|| default_model.to_string())
}

const TIER_ORDER: [&str; 3] = ["low", "mid", "high"];

fn model_above_in(t: &Tiers, provider: &str, task: &str) -> Option<String> {
    let tier = t.task_tier.get(task)?;
    let models = t.tiers.get(provider)?;
    let current = models.get(tier).and_then(|v| v.first())?;
    let start = TIER_ORDER.iter().position(|x| *x == tier.as_str())?;
    // Walk past tiers whose head model matches this one — escalating to the model
    // that just failed would be a no-op (e.g. openai's low and mid coincide).
    for next in &TIER_ORDER[start + 1..] {
        if let Some(m) = models.get(*next).and_then(|v| v.first()) {
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
        "Each tier lists several models in priority order: if the first is retired \
         (HTTP 404 / 'model does not exist') or rate-limited (HTTP 429), the agent \
         falls through to the next model in the SAME tier, then rotates to another \
         configured provider's tier — set more than one API key (e.g. GROQ_API_KEY \
         and GEMINI_API_KEY) to enable cross-provider rotation. An explicit \
         MARS_LLM_MODEL disables both."
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
