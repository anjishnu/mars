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

/// `mars ssh <host> [ssh args…]` — wraps system ssh so the auth socket is
/// forwarded and `MARS_AUTH_SOCK` is set in the remote shell, with no reliance
/// on server-side `AcceptEnv`.
pub fn ssh_main(host: String, extra: Vec<String>) -> Result<()> {
    let home_sock = broker_socket_path()?;
    if UnixStream::connect(&home_sock).is_err() {
        eprintln!(
            "mars ssh: note — the home broker isn't running, so the remote agent won't have a key.\n  \
             start it first (in another terminal):  mars keyd"
        );
    }
    let remote_sock = remote_socket_path();
    let control = home_sock.with_file_name("cm-%r@%h:%p");
    // Set the env via the remote command (not SetEnv), then hand over to a login shell.
    let remote_cmd = format!("MARS_AUTH_SOCK={remote_sock} exec ${{SHELL:-/bin/sh}} -l");
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
