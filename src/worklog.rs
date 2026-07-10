//! The work journal — watch-mode verdicts persisted as a stream of "what was
//! happening" snapshots. Deliberately separate from `llm_log` (that log is
//! about the cost and behavior of LLM calls; this one is about the user's
//! work). Consumers today: the mission inference (a one-line "what is this
//! person working on", refreshed in the background and shown by `mars ls`) and
//! the expand-all notices digest. The stream is the substrate for standup
//! digests, deviation alerts, and procedure mining (§future work).

use std::path::PathBuf;

pub struct WorkEntry {
    pub ts: u64,
    pub session: String,
    pub tab: String,
    pub verdict: String,
    pub failed: bool,
    pub dur_secs: Option<u64>,
}

/// `~/.mars/worklog.jsonl`; `MARS_WORKLOG` overrides (tests, eval isolation).
pub fn worklog_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MARS_WORKLOG") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".mars").join("worklog.jsonl"))
}

fn mission_path() -> Option<PathBuf> {
    worklog_path().map(|p| p.with_file_name("mission.json"))
}

/// Append one snapshot. Best-effort — never fails the caller.
pub fn record(e: &WorkEntry) {
    let Some(path) = worklog_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = serde_json::json!({
        "ts": e.ts,
        "session": e.session,
        "tab": e.tab,
        "verdict": e.verdict,
        "failed": e.failed,
        "dur_secs": e.dur_secs,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// The most recent `limit` snapshots for `session` (chronological).
pub fn recent(session: &str, limit: usize) -> Vec<WorkEntry> {
    let Some(path) = worklog_path() else { return Vec::new() };
    let Ok(content) = std::fs::read_to_string(&path) else { return Vec::new() };
    let mut out: Vec<WorkEntry> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|j| j["session"].as_str() == Some(session))
        .map(|j| WorkEntry {
            ts: j["ts"].as_u64().unwrap_or(0),
            session: session.to_string(),
            tab: j["tab"].as_str().unwrap_or("").to_string(),
            verdict: j["verdict"].as_str().unwrap_or("").to_string(),
            failed: j["failed"].as_bool().unwrap_or(false),
            dur_secs: j["dur_secs"].as_u64(),
        })
        .collect();
    let skip = out.len().saturating_sub(limit);
    out.drain(..skip);
    out
}

/// Persist the inferred mission for `session` (read by `mars ls`).
pub fn save_mission(session: &str, mission: &str, as_of: u64) {
    let Some(path) = mission_path() else { return };
    let mut map: serde_json::Map<String, serde_json::Value> = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    map.insert(
        session.to_string(),
        serde_json::json!({ "mission": mission, "as_of": as_of }),
    );
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, s);
    }
}

/// The last inferred mission for `session`, with when it was inferred.
pub fn load_mission(session: &str) -> Option<(String, u64)> {
    let path = mission_path()?;
    let s = std::fs::read_to_string(path).ok()?;
    let j: serde_json::Value = serde_json::from_str(&s).ok()?;
    let m = &j[session];
    Some((m["mission"].as_str()?.to_string(), m["as_of"].as_u64().unwrap_or(0)))
}
