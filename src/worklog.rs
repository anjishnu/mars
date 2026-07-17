//! The work journal — watch-mode verdicts persisted as a stream of "what was
//! happening" snapshots. Deliberately separate from `llm_log` (that log is
//! about the cost and behavior of LLM calls; this one is about the user's
//! work). Consumers today: the mission inference (a one-line "what is this
//! person working on", refreshed in the background and shown by `mars ls`) and
//! the expand-all notices digest. The stream is the substrate for standup
//! digests, deviation alerts, and procedure mining (§future work).

use std::path::PathBuf;

/// One journal line. The schema doubles as the future per-command ledger
/// (OSC 133 shell integration would write the same shape), so the stores
/// never fork: a watch verdict is just a ledger entry whose `verdict` is
/// LLM-compressed and whose `error_excerpt` is the deterministic evidence
/// under it. `cwd`/`command`/`exit`/`error_excerpt` are honest-when-known:
/// spawn cwd, the last mars-sent command, the PTY exit code, the redacted
/// tail on failure — absent otherwise (and on pre-0.4 lines).
pub struct WorkEntry {
    pub ts: u64,
    pub session: String,
    pub tab: String,
    pub verdict: String,
    pub failed: bool,
    pub dur_secs: Option<u64>,
    pub cwd: String,
    pub command: Option<String>,
    pub exit: Option<i32>,
    pub error_excerpt: Option<String>,
}

/// Unix seconds now — the journal's clock.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A short "how long ago" for a unix timestamp: "just now" / "12m ago" / "3h ago" / "2d ago".
pub fn ago(as_of: u64) -> String {
    let secs = now_secs().saturating_sub(as_of);
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// `~/.mars/worklog.jsonl`; `MARS_WORKLOG` overrides (tests, eval isolation).
pub fn worklog_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MARS_WORKLOG") {
        return Some(PathBuf::from(p));
    }
    crate::sys::paths::home_dir().map(|h| h.join(".mars").join("worklog.jsonl"))
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
    let mut line = serde_json::json!({
        "ts": e.ts,
        "session": e.session,
        "tab": e.tab,
        "verdict": e.verdict,
        "failed": e.failed,
        "dur_secs": e.dur_secs,
    });
    // Optional outcome fields stay off old-shape lines entirely when unknown.
    if !e.cwd.is_empty() {
        line["cwd"] = serde_json::json!(e.cwd);
    }
    if let Some(c) = &e.command {
        line["command"] = serde_json::json!(c);
    }
    if let Some(x) = e.exit {
        line["exit"] = serde_json::json!(x);
    }
    if let Some(x) = &e.error_excerpt {
        line["error_excerpt"] = serde_json::json!(x);
    }
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
            cwd: j["cwd"].as_str().unwrap_or("").to_string(),
            command: j["command"].as_str().map(str::to_string),
            exit: j["exit"].as_i64().map(|x| x as i32),
            error_excerpt: j["error_excerpt"].as_str().map(str::to_string),
        })
        .collect();
    let skip = out.len().saturating_sub(limit);
    out.drain(..skip);
    out
}

/// Bound the journal: past 2×`max_lines`, rewrite it to the newest `max_lines`
/// (tmp file + rename, so a crash can't truncate). Called once at App start —
/// concurrent appends during the rewrite window are best-effort, like the file.
pub fn compact(max_lines: usize) {
    if max_lines == 0 {
        return;
    }
    let Some(path) = worklog_path() else { return };
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max_lines * 2 {
        return;
    }
    let keep = &lines[lines.len() - max_lines..];
    let tmp = path.with_extension("jsonl.tmp");
    if std::fs::write(&tmp, keep.join("\n") + "\n").is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
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

fn goals_path() -> Option<PathBuf> {
    worklog_path().map(|p| p.with_file_name("goals.json"))
}

/// The goals captured at the last detach for `session` — what the user was
/// working toward when they stepped away. The return briefing reads these to
/// assess progress against what actually happened on the panes.
pub fn save_goals(session: &str, goals: &[String], as_of: u64) {
    let Some(path) = goals_path() else { return };
    let mut map: serde_json::Map<String, serde_json::Value> = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    map.insert(session.to_string(), serde_json::json!({ "goals": goals, "as_of": as_of }));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, s);
    }
}

/// The goals last captured for `session` (empty if none).
pub fn load_goals(session: &str) -> Vec<String> {
    let Some(path) = goals_path() else { return Vec::new() };
    let Ok(s) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&s) else { return Vec::new() };
    j[session]["goals"]
        .as_array()
        .map(|a| a.iter().filter_map(|g| g.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// When the goals for `session` were captured (for freshness gating).
pub fn goals_as_of(session: &str) -> Option<u64> {
    let path = goals_path()?;
    let s = std::fs::read_to_string(path).ok()?;
    let j: serde_json::Value = serde_json::from_str(&s).ok()?;
    j[session]["as_of"].as_u64()
}

fn summarizing_path() -> Option<PathBuf> {
    worklog_path().map(|p| p.with_file_name("summarizing.json"))
}

/// Note that a fresh summary (the detach-time goal-capture LLM call) is in flight
/// for `session`, as of wall-clock `ts`. Kept in its own marker file — never in
/// goals.json — so goal CONSUMERS (the briefing's evidence) never mistake the
/// "…summarizing…" placeholder for a real goal. The real goals overwrite nothing
/// here; the marker simply ages out.
pub fn mark_summarizing(session: &str, ts: u64) {
    let Some(path) = summarizing_path() else { return };
    let mut map: serde_json::Map<String, serde_json::Value> = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    map.insert(session.to_string(), serde_json::json!(ts));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(&map) {
        let _ = std::fs::write(&path, s);
    }
}

/// When a summary was last marked in flight for `session` (None if never).
pub fn summarizing_since(session: &str) -> Option<u64> {
    let path = summarizing_path()?;
    let s = std::fs::read_to_string(path).ok()?;
    let j: serde_json::Value = serde_json::from_str(&s).ok()?;
    j[session].as_u64()
}

fn briefings_path() -> Option<PathBuf> {
    worklog_path().map(|p| p.with_file_name("briefings.jsonl"))
}

/// One prior briefing, distilled for continuity in the next one.
pub struct PrevBriefing {
    /// Compact manifest distillation (`"failed: OOM · blocked: deploy y/N"`).
    pub facts: String,
    /// When it was shown (unix secs), so the next briefing can say "3h ago".
    pub ts: u64,
}

/// Append a finalized briefing to the log — the continuity backbone. Every
/// return records what it said, so the next one can report progress against it
/// and (later) a history can be paged back through. Best-effort; bounded.
pub fn log_briefing(session: &str, narrative: &str, facts: &str, away_secs: u64, ts: u64) {
    let Some(path) = briefings_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = serde_json::json!({
        "ts": ts, "session": session, "narrative": narrative,
        "facts": facts, "away_secs": away_secs,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
    // Bound the file: past 2×500 lines, keep the newest 500 (tmp + rename).
    if let Ok(content) = std::fs::read_to_string(&path) {
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() > 1000 {
            let keep = &lines[lines.len() - 500..];
            let tmp = path.with_extension("jsonl.tmp");
            if std::fs::write(&tmp, keep.join("\n") + "\n").is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

/// The most recent briefing logged for `session`, for the "since last time"
/// continuity line. None if this is the first return.
pub fn load_last_briefing(session: &str) -> Option<PrevBriefing> {
    let path = briefings_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    content
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|j| j["session"].as_str() == Some(session))
        .map(|j| PrevBriefing {
            facts: j["facts"].as_str().unwrap_or("").to_string(),
            ts: j["ts"].as_u64().unwrap_or(0),
        })
}
