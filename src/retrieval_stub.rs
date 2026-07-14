//! The no-`memory` build of [`crate::retrieval`]: same facade, neutral values.
//! This file is what makes the memory subsystem deletion-proof — every consumer
//! (agent prompt assembly, the accept hook, the management actions, the REC
//! chip) degrades gracefully through these stubs with zero `cfg` at call sites.
//! Keep it mirroring the facade section of `retrieval.rs`; both builds must
//! pass `--selfcheck` (see AGENTS.md).

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoryMode {
    None,
}

impl MemoryMode {
    pub fn from_env() -> Self {
        MemoryMode::None
    }
    pub fn as_str(self) -> &'static str {
        "none"
    }
}

#[allow(dead_code)]
pub struct CommandMemory {
    pub request: String,
    pub command: String,
    pub ts: u64,
    pub session: String,
    pub cwd: String,
}

pub fn command_memory_path() -> Option<PathBuf> {
    None
}

pub fn denylist_path() -> Option<PathBuf> {
    None
}

pub fn remember_command(_request: &str, _command: &str) {}

pub fn load_command_records() -> Vec<CommandMemory> {
    Vec::new()
}

/// Identity in the memory-free build: nothing is retrieved into prompts here,
/// so redaction's remaining consumers (worklog excerpts, persona) store/emit
/// text as-is — local files only.
pub fn redact(line: &str) -> String {
    line.to_string()
}

pub fn fewshot_for(_request: &str) -> String {
    String::new()
}

pub fn docs_context_for(_question: &str) -> Option<String> {
    None
}
