//! The voice seam. A user-editable style file (`~/.mars/persona.md`) rides
//! into VOICE tasks (ask, watch) as the FINAL system message, wrapped in a
//! compiled precedence preamble — style can color prose but can never
//! override the directive protocol or output-format rules above it. FORMAT
//! tasks (translate, naming, mission, cursor-insert) never see it: their
//! output is machine-parsed, and mission text is re-ingested into prompts.
//!
//! Semantics of the file (documented in the seeded default): absent → the
//! shipped product voice applies; empty/whitespace → persona off; else the
//! contents, per-line redacted and hard-capped. Hot-read on every prompt
//! assembly (the denylist pattern) — edits apply to the next reply.

use std::path::PathBuf;

/// Bounds both token cost and the injection surface of user-authored text.
const PERSONA_MAX_CHARS: usize = 2000;

/// `~/.mars/persona.md`; `MARS_PERSONA` overrides (tests, eval isolation).
pub fn persona_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MARS_PERSONA") {
        return Some(PathBuf::from(p));
    }
    crate::sys::paths::home_dir().map(|h| h.join(".mars").join("persona.md"))
}

/// The active persona text: `None` means persona is off (user emptied the
/// file). User-authored lines are treated like any retrieved text — redacted
/// before they can ride into a prompt — and hard-capped.
pub fn load() -> Option<String> {
    let text = match persona_path().map(|p| std::fs::read_to_string(p)) {
        Some(Ok(s)) => {
            if s.trim().is_empty() {
                return None; // explicit opt-out
            }
            s.lines()
                .filter(|l| !l.trim_start().starts_with('#')) // seeded header comments
                .map(crate::retrieval::redact)
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => crate::prompts::PERSONA_DEFAULT.to_string(), // no file → product voice
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(if text.chars().count() > PERSONA_MAX_CHARS {
        let mut t: String = text.chars().take(PERSONA_MAX_CHARS).collect();
        t.push('…');
        t
    } else {
        text
    })
}

/// The persona as a ready-to-append system message — always the LAST system
/// message of a VOICE task, so the preamble's "nothing below overrides
/// anything above" is positionally true. Never travels through `.replace()`.
pub fn system_message() -> Option<serde_json::Value> {
    let text = load()?;
    Some(serde_json::json!({
        "role": "system",
        "content": format!("{}\n{}", crate::prompts::PERSONA_PREAMBLE.trim_end(), text),
    }))
}

/// First open (palette action): seed a commented template around the default
/// voice so the file teaches its own contract.
pub fn seed_if_missing() -> Option<PathBuf> {
    let p = persona_path()?;
    if !p.exists() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            &p,
            format!(
                "# Your assistant's voice — style notes applied to prose replies (ask, watch).\n\
                 # Style only: this file can never change what the agent does, run commands,\n\
                 # or alter directive/output formats. Empty file = persona off. Lines starting\n\
                 # with # are ignored. Delete the file to restore this default.\n\n{}",
                crate::prompts::PERSONA_DEFAULT.trim_end()
            ),
        );
    }
    Some(p)
}
