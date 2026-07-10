//! Every instruction the binary sends to a model, as editable Markdown under
//! `src/prompts/` — embedded at compile time (`include_str!`) so the
//! single-binary install still ships everything. Editing a prompt is editing
//! its `.md` file; no prompt text lives in code. `{name}` substrings are
//! placeholders the call sites substitute with `.replace()` (substitute
//! user/screen-derived content LAST, so injected text is never re-scanned for
//! placeholders). The selfcheck asserts each template still carries its
//! placeholders, so a stray edit can't silently break assembly.

pub const ASK_SYSTEM: &str = include_str!("prompts/ask_system.md");
pub const TRANSLATE_SYSTEM: &str = include_str!("prompts/translate_system.md");
pub const TRANSLATE_REASONING_CAP: &str = include_str!("prompts/translate_reasoning_cap.md");
pub const TRANSLATE_EXAMPLES: &str = include_str!("prompts/translate_examples.md");
pub const WATCH_SYSTEM: &str = include_str!("prompts/watch_system.md");
pub const WATCH_HINT_EXIT: &str = include_str!("prompts/watch_hint_exit.md");
pub const WATCH_HINT_QUIET: &str = include_str!("prompts/watch_hint_quiet.md");
pub const MISSION_SYSTEM: &str = include_str!("prompts/mission_system.md");
pub const AUTO_NAME_SYSTEM: &str = include_str!("prompts/auto_name_system.md");
pub const NAME_SESSION_SYSTEM: &str = include_str!("prompts/name_session_system.md");
#[cfg_attr(not(feature = "memory"), allow(dead_code))] // consumer is the docs corpus
pub const DOCS_CONTEXT_PREAMBLE: &str = include_str!("prompts/docs_context_preamble.md");
pub const EXPLAIN_THIS: &str = include_str!("prompts/explain_this.md");
pub const EXPLAIN_FAILURE: &str = include_str!("prompts/explain_failure.md");
