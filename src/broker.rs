//! The key-never-leaves-home broker (`mars keyd`) and the remote-side proxy call.
//!
//! `mars keyd` runs on your home machine, holds the LLM key, and answers `Chat`
//! requests that arrive over a Unix socket. When you `mars ssh <host>`, that
//! socket is remote-forwarded, so the agent on the remote box asks the broker
//! instead of ever holding a key. Reuses `session.rs`'s JSON-lines frame style
//! (`write_frame` + `read_line`) — no new transport.

use crate::agent::{self, AgentConfig};
use crate::session::write_frame;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

const BROKER_VERSION: &str = "1";

/// Remote → home. One request per connection lifetime is enough, but the
/// connection is kept open for reuse across an agent session.
#[derive(Serialize, Deserialize)]
pub enum BrokerRequest {
    Chat {
        version: String,
        /// `None` → the broker uses its own configured model (the robust default,
        /// since the remote may not know which provider the key is for).
        model: Option<String>,
        messages: Vec<serde_json::Value>,
        max_tokens: u32,
        temperature: f64,
    },
}

/// Home → remote.
#[derive(Serialize, Deserialize)]
pub enum BrokerResponse {
    Chat { text: String },
    Error { message: String },
}

/// `$HOME/.mars/auth.sock`, under a `0700` dir — the home broker's socket, and
/// the thing `ssh -R` forwards to the remote.
pub fn broker_socket_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
    let dir = PathBuf::from(home).join(".mars");
    std::fs::create_dir_all(&dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir.join("auth.sock"))
}

/// Well-known forwarded-socket path on the remote (per-uid), so a plain `ssh`
/// with a `RemoteForward` line (from `mars ssh-setup`) works even when
/// `MARS_AUTH_SOCK` isn't exported.
pub fn remote_socket_path() -> String {
    let uid = unsafe { libc::getuid() };
    format!("/tmp/mars-auth-{uid}.sock")
}

/// The socket the remote agent should proxy through, if any: an explicit
/// `MARS_AUTH_SOCK`, else the well-known forwarded path if it exists.
pub fn detect_broker_sock() -> Option<String> {
    if let Ok(s) = std::env::var("MARS_AUTH_SOCK") {
        if !s.is_empty() {
            return Some(s);
        }
    }
    let wk = remote_socket_path();
    if std::path::Path::new(&wk).exists() {
        return Some(wk);
    }
    None
}

/// The home broker daemon. Loads the key once (from env today), binds the
/// socket, and answers `Chat` by running the real LLM call — the only process
/// that ever constructs an `Authorization` header.
pub fn keyd_main() -> Result<()> {
    let cfg = AgentConfig::from_env();
    if !cfg.is_configured() {
        anyhow::bail!(
            "mars keyd: no API key found. Set GROQ_API_KEY / GEMINI_API_KEY / MARS_LLM_KEY \
             on this machine first."
        );
    }
    let path = broker_socket_path()?;
    // Clear a stale socket from a previous run (nothing listening).
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)
        .map_err(|e| anyhow::anyhow!("mars keyd: cannot bind {}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    println!(
        "mars keyd: broker listening (provider: {}) at {}",
        cfg.provider,
        path.display()
    );
    println!("  now run:  mars ssh <host>   — the agent works there, no key on the box.");
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                std::thread::spawn(move || {
                    let _ = handle_conn(stream);
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

fn handle_conn(stream: UnixStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut w = stream;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break; // client hung up
        }
        let req: BrokerRequest = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let BrokerRequest::Chat { version, model, messages, max_tokens, temperature } = req;
        let resp = if version != BROKER_VERSION {
            BrokerResponse::Error {
                message: format!("broker version mismatch (home {BROKER_VERSION}, remote {version})"),
            }
        } else {
            // Fresh config each request → the key is read here, at home, and a
            // provider/key change is picked up without restarting the daemon.
            let mut c = AgentConfig::from_env();
            if let Some(m) = model {
                c.model = m;
            }
            c.max_tokens = max_tokens;
            c.temperature = temperature;
            match agent::chat(&c, messages) {
                Ok(text) => BrokerResponse::Chat { text },
                Err(e) => BrokerResponse::Error { message: e.to_string() },
            }
        };
        write_frame(&mut w, &resp)?;
    }
    Ok(())
}

/// Remote side: send a chat request home over the forwarded socket and block for
/// the completion. No `Authorization` header, no key — ever — on this box.
pub fn chat_via_broker(
    sock: &str,
    cfg: &AgentConfig,
    messages: Vec<serde_json::Value>,
) -> Result<String> {
    let stream = UnixStream::connect(sock)
        .map_err(|e| anyhow::anyhow!("home broker unreachable ({e}); is `mars keyd` running + the tunnel up?"))?;
    // A little longer than chat()'s own 30s, so the home call's timeout wins.
    stream.set_read_timeout(Some(Duration::from_secs(40)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut w = stream;
    let model = if cfg.model.is_empty() { None } else { Some(cfg.model.clone()) };
    write_frame(
        &mut w,
        &BrokerRequest::Chat {
            version: BROKER_VERSION.to_string(),
            model,
            messages,
            max_tokens: cfg.max_tokens,
            temperature: cfg.temperature,
        },
    )?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    match serde_json::from_str::<BrokerResponse>(line.trim())
        .map_err(|e| anyhow::anyhow!("broker sent a malformed reply: {e}"))?
    {
        BrokerResponse::Chat { text } => Ok(text),
        BrokerResponse::Error { message } => anyhow::bail!("{message}"),
    }
}

// ── Fleet cache: which hosts you've been on, for `mars ls` ───────────────────

/// One host you've connected to — the home machine's view of the fleet. `cwd` /
/// `last_status` are filled by a later status-push; today only `host`/`as_of`.
#[derive(Serialize, Deserialize, Clone)]
pub struct FleetEntry {
    pub host: String,
    pub cwd: Option<String>,
    pub session: Option<String>,
    pub last_status: Option<String>,
    /// Unix seconds of the last interaction.
    pub as_of: u64,
}

fn fleet_path() -> Result<PathBuf> {
    Ok(broker_socket_path()?.with_file_name("fleet.json"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load the fleet, most-recent first. Empty on any error (the cache is best-effort).
pub fn fleet_load() -> Vec<FleetEntry> {
    let mut v: Vec<FleetEntry> = fleet_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    v.sort_by(|a, b| b.as_of.cmp(&a.as_of));
    v
}

/// Record (upsert) a host interaction. Best-effort; never fails a connection.
pub fn fleet_record(host: &str, cwd: Option<String>) {
    let mut v = fleet_load();
    match v.iter_mut().find(|e| e.host == host) {
        Some(e) => {
            e.as_of = now_secs();
            if cwd.is_some() {
                e.cwd = cwd;
            }
        }
        None => v.push(FleetEntry {
            host: host.to_string(),
            cwd,
            session: None,
            last_status: None,
            as_of: now_secs(),
        }),
    }
    v.sort_by(|a, b| b.as_of.cmp(&a.as_of));
    v.truncate(50);
    if let Ok(p) = fleet_path() {
        if let Ok(s) = serde_json::to_string_pretty(&v) {
            let _ = std::fs::write(p, s);
        }
    }
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

/// Resolve a `mars ls` follow-up (an ordinal like "2", or a host name / unique
/// prefix) to a host from the numbered list. `None` = skip / no match.
pub fn resolve_target(hosts: &[String], input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() || t == "q" {
        return None;
    }
    if let Ok(n) = t.parse::<usize>() {
        return hosts.get(n.checked_sub(1)?).cloned();
    }
    if let Some(h) = hosts.iter().find(|h| h.as_str() == t) {
        return Some(h.clone());
    }
    let pre: Vec<&String> = hosts.iter().filter(|h| h.starts_with(t)).collect();
    if pre.len() == 1 {
        Some(pre[0].clone())
    } else {
        None
    }
}

/// Make sure the home broker is running, auto-starting it (detached) if not —
/// so `mars ssh` is one command, not two. The spawned `mars keyd` inherits THIS
/// shell's env, which is exactly where the API key lives. Best-effort: ssh
/// proceeds either way (a keyless box just won't have an agent).
fn ensure_keyd(home_sock: &std::path::Path) -> bool {
    if UnixStream::connect(home_sock).is_ok() {
        return true; // already up
    }
    // Starting the broker needs a key in this environment.
    if !AgentConfig::from_env().is_configured() {
        eprintln!(
            "mars ssh: no API key in this shell, so the remote agent won't have one.\n  \
             set GROQ_API_KEY / GEMINI_API_KEY here (then it auto-starts), or run `mars keyd` \
             where your key lives."
        );
        return false;
    }
    let _ = std::fs::remove_file(home_sock); // clear a stale socket
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("keyd");
    cmd.env_remove("MARS_AUTH_SOCK"); // the broker must never run in proxy mode
    // Log to ~/.mars/keyd.log; never spill the daemon's output onto this TTY.
    let log = home_sock.with_file_name("keyd.log");
    match std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        Ok(f) => {
            let f2 = f.try_clone().ok();
            cmd.stdout(f);
            match f2 {
                Some(f2) => { cmd.stderr(f2); }
                None => { cmd.stderr(std::process::Stdio::null()); }
            }
        }
        Err(_) => {
            cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        }
    }
    cmd.stdin(std::process::Stdio::null());
    // Detach from this TTY so the broker outlives the ssh session (like ssh-agent).
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if cmd.spawn().is_err() {
        return false;
    }
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        if UnixStream::connect(home_sock).is_ok() {
            eprintln!("mars ssh: started the home broker (mars keyd) automatically.");
            return true;
        }
    }
    eprintln!("mars ssh: could not start the home broker (see ~/.mars/keyd.log).");
    false
}

/// `mars ssh <host> [ssh args…]` — wraps system ssh so the auth socket is
/// forwarded and `MARS_AUTH_SOCK` is set in the remote shell, with no reliance
/// on server-side `AcceptEnv`.
pub fn ssh_main(host: String, extra: Vec<String>) -> Result<()> {
    let home_sock = broker_socket_path()?;
    ensure_keyd(&home_sock); // auto-start the broker if it isn't already up

    fleet_record(&host, None); // remember this host for `mars ls`
    let remote_sock = remote_socket_path();
    let control = home_sock.with_file_name("cm-%r@%h:%p");
    // Set the env via the remote command (not SetEnv); nudge an install if mars is
    // missing (never a dead end); then hand over to a login shell.
    // If mars is missing, print the real install steps. A distro `cargo` (e.g.
    // Ubuntu's 1.75) is too old — needs Rust >= 1.85, so rustup first (official
    // one-liner from rust-lang.org/tools/install), then cargo install.
    let remote_cmd = format!(
        "command -v mars >/dev/null 2>&1 || printf '[mars] not installed here. Install:\\n  \
         curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust toolchain (>=1.85)\\n  \
         . \"$HOME/.cargo/env\" && cargo install mars-terminal\\n'; \
         MARS_AUTH_SOCK={remote_sock} exec ${{SHELL:-/bin/sh}} -l"
    );
    let status = std::process::Command::new("ssh")
        .arg("-o").arg("StreamLocalBindUnlink=yes")
        .arg("-o").arg("ControlMaster=auto")
        .arg("-o").arg("ControlPersist=60s")
        .arg("-o").arg(format!("ControlPath={}", control.display()))
        .arg("-R").arg(format!("{remote_sock}:{}", home_sock.display()))
        .args(&extra)
        .arg("-t")
        .arg(&host)
        .arg(&remote_cmd)
        .status()
        .map_err(|e| anyhow::anyhow!("mars ssh: could not launch ssh: {e}"))?;
    std::process::exit(status.code().unwrap_or(1));
}
