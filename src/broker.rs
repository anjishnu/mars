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

/// The installer, embedded so `mars ssh` can drop it on any host it connects
/// to — version-matched to this binary, no GitHub/crates availability needed
/// for the script itself to arrive.
pub const INSTALL_SH: &str = include_str!("../install.sh");

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
        /// Self-reported by the remote so the home fleet reflects live activity
        /// (`mars ls`). Optional — older remotes simply omit them.
        #[serde(default)]
        host: Option<String>,
        #[serde(default)]
        session: Option<String>,
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
/// `MARS_AUTH_SOCK` isn't exported. The uid is the *local* one — two users who
/// share a local uid (e.g. two single-user Macs, both 501) would collide on a
/// shared remote; fixing that needs remote-home discovery (a protocol change).
pub fn remote_socket_path() -> String {
    let uid = unsafe { libc::getuid() };
    format!("/tmp/mars-auth-{uid}.sock")
}

/// True if something is listening at `path`. A dead leftover socket file is
/// unlinked: sshd refuses to bind a `-R` forward over it (server-side
/// `StreamLocalBindUnlink` is off by default and the client-side flag only
/// covers local forwards), so sweeping here lets the next connection bind.
pub fn probe_and_sweep(path: &std::path::Path) -> bool {
    if UnixStream::connect(path).is_ok() {
        return true;
    }
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    false
}

/// The socket the remote agent should proxy through, if any: an explicit
/// `MARS_AUTH_SOCK`, else any live forwarded socket — a dead socket (the
/// tunnel is gone) must fall through to the provider chain, not pin every
/// call to an unreachable broker.
pub fn detect_broker_sock() -> Option<String> {
    if let Ok(s) = std::env::var("MARS_AUTH_SOCK") {
        if !s.is_empty() {
            return Some(s);
        }
    }
    find_live_auth_sock(std::path::Path::new("/tmp"))
}

/// The forwarded socket's name carries the HOME machine's uid, which rarely
/// matches this box's (a Mac's 501 vs Linux's 1000) — so scan for any live
/// `mars-auth-*.sock` instead of guessing by uid. Own-uid first (the
/// same-uid case stays deterministic), then lexicographic. Dead leftovers
/// are swept along the way, where permissions allow.
pub fn find_live_auth_sock(dir: &std::path::Path) -> Option<String> {
    let own = dir.join(format!("mars-auth-{}.sock", unsafe { libc::getuid() }));
    if probe_and_sweep(&own) {
        return Some(own.to_string_lossy().into_owned());
    }
    let mut candidates: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("mars-auth-") && n.ends_with(".sock"))
        })
        .collect();
    candidates.sort();
    candidates
        .into_iter()
        .find(|p| probe_and_sweep(p))
        .map(|p| p.to_string_lossy().into_owned())
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
        let BrokerRequest::Chat { version, model, messages, max_tokens, temperature, host, session } =
            req;
        // Status push: a brokered call is proof the remote's agent is alive —
        // refresh the fleet so `mars ls` shows it as current, not stale.
        if let Some(h) = &host {
            fleet_status(h, session, "agent active");
        }
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
            match agent::chat(&c, messages, "remote") {
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
            host: hostname(),
            session: std::env::var("MARS_SESSION").ok(),
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

/// One host you've connected to — the home machine's view of the fleet.
/// `session` / `last_status` are refreshed by the status push in `handle_conn`
/// (every brokered agent call self-reports host + session); `cwd` is recorded
/// by `mars ssh`.
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
    fleet_save(v);
}

/// The status-push half of the fleet cache: every brokered agent call from a
/// remote refreshes the home view of that host, so `mars ls` shows current
/// activity instead of only "when you last ssh'd there".
pub fn fleet_status(host: &str, session: Option<String>, status: &str) {
    let mut v = fleet_load();
    match v.iter_mut().find(|e| e.host == host) {
        Some(e) => {
            e.as_of = now_secs();
            e.last_status = Some(status.to_string());
            if session.is_some() {
                e.session = session;
            }
        }
        None => v.push(FleetEntry {
            host: host.to_string(),
            cwd: None,
            session,
            last_status: Some(status.to_string()),
            as_of: now_secs(),
        }),
    }
    fleet_save(v);
}

fn fleet_save(mut v: Vec<FleetEntry>) {
    v.sort_by(|a, b| b.as_of.cmp(&a.as_of));
    v.truncate(50);
    if let Ok(p) = fleet_path() {
        if let Ok(s) = serde_json::to_string_pretty(&v) {
            let _ = std::fs::write(p, s);
        }
    }
}

/// This machine's hostname — what a remote self-reports over the broker.
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    let ok =
        unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    if !ok {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
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

/// The remote command for the prelude ssh (the connection that authenticates
/// once and persists the ControlMaster): sweep a stale auth socket left by a
/// dead session — it would make the interactive ssh's `-R` bind fail — then
/// stage the embedded installer. The sweep is `;`-separated so it runs even if
/// the installer write fails and the exit status stays that of the `&&` chain
/// (which is what `pushed` reads). `sweep` must be false when a live
/// ControlMaster is being reused: its previous `-R` forward survives on the
/// master, still bound to the existing socket inode, so removing the file
/// would orphan a working tunnel — and the re-requested forward is a mux
/// no-op that never re-binds the path.
pub fn remote_prelude_cmd(remote_sock: &str, sweep: bool) -> String {
    let rm = if sweep {
        format!("rm -f {remote_sock}; ")
    } else {
        String::new()
    };
    format!(
        "{rm}mkdir -p ~/.mars && cat > ~/.mars/install.sh && chmod +x ~/.mars/install.sh"
    )
}

/// The interactive session's remote command: report the tunnel's actual state
/// (a working `mars ssh` must not be indistinguishable from plain ssh), then
/// land the user IN a remote mars session — attach to the most recent live one,
/// else create "main" — with the auth socket exported so the daemon and its
/// shells inherit it. Detaching ends the ssh, tmux-style. `command -v` sees
/// only sshd's bare non-login PATH, so the real install destinations are
/// probed too; if mars is missing, nudge and fall back to a plain login shell
/// (plain `ssh` is the deliberate escape hatch for a bare shell).
pub fn remote_session_cmd(remote_sock: &str, pushed: bool) -> String {
    let nudge = if pushed {
        "printf '[mars] not installed here — installer is ready. Run:\\n  sh ~/.mars/install.sh\\n'"
    } else {
        "printf '[mars] not installed here. Install:\\n  \
         curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust toolchain (>=1.85)\\n  \
         . \"$HOME/.cargo/env\" && cargo install mars-terminal --locked\\n'"
    };
    format!(
        "if [ -S {remote_sock} ]; then \
         printf '[mars] agent tunnel ready — your home key answers here\\n'; else \
         printf '[mars] no agent tunnel (forward failed?) — the agent needs a key on this box\\n'; fi; \
         M=\"$(command -v mars 2>/dev/null)\"; \
         if [ -z \"$M\" ] && [ -x \"$HOME/.cargo/bin/mars\" ]; then M=\"$HOME/.cargo/bin/mars\"; fi; \
         if [ -z \"$M\" ] && [ -x \"$HOME/.local/bin/mars\" ]; then M=\"$HOME/.local/bin/mars\"; fi; \
         export MARS_AUTH_SOCK={remote_sock}; \
         if [ -n \"$M\" ]; then \"$M\" attach 2>/dev/null || exec \"$M\" new main; else \
         {nudge}; exec ${{SHELL:-/bin/sh}} -l; fi"
    )
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
    // A ControlMaster killed uncleanly (pkill, crash) leaves its socket file
    // behind; ssh then warns "ControlSocket … already exists, disabling
    // multiplexing" and drops connection-sharing. Sweep dead ones first:
    // `ssh -O check` answers from the socket alone, so a dummy destination works.
    if let Some(dir) = control.parent() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with("cm-") {
                    continue;
                }
                let alive = std::process::Command::new("ssh")
                    .arg("-O").arg("check")
                    .arg("-o").arg(format!("ControlPath={}", e.path().display()))
                    .arg("stale-check")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !alive {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    // Is a live master already serving this host? Then its previous -R forward
    // is still up (mux forwards live as long as the master), so the sweep and
    // the forward request must both be skipped: rm would delete the socket out
    // from under the live listener, and the re-request is a mux no-op that
    // would never re-bind the path it just lost.
    let master_alive = std::process::Command::new("ssh")
        .arg("-O").arg("check")
        .arg("-o").arg(format!("ControlPath={}", control.display()))
        .args(&extra)
        .arg(&host)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    // Drop the embedded installer at ~/.mars/install.sh over the SAME connection,
    // BEFORE the interactive session: this first ssh performs the (single)
    // authentication and persists the ControlMaster, which the interactive ssh
    // then reuses — one prompt total. Password prompts read /dev/tty, so piping
    // the script through stdin is safe. Best-effort: never blocks the session.
    let pushed = {
        use std::io::Write as _;
        let mut child = std::process::Command::new("ssh")
            .arg("-o").arg("ControlMaster=auto")
            .arg("-o").arg("ControlPersist=60s")
            // A master whose TCP died (sleep, network change) still answers
            // `-O check` over its local socket, then ambushes the next session
            // with "Broken pipe". Keepalives make it notice and exit instead.
            .arg("-o").arg("ServerAliveInterval=30")
            .arg("-o").arg("ServerAliveCountMax=3")
            .arg("-o").arg(format!("ControlPath={}", control.display()))
            .args(&extra)
            .arg(&host)
            .arg(remote_prelude_cmd(&remote_sock, !master_alive))
            .stdin(std::process::Stdio::piped())
            .spawn()
            .ok();
        match child.as_mut() {
            Some(c) => {
                let ok_write = c
                    .stdin
                    .take()
                    .and_then(|mut s| s.write_all(INSTALL_SH.as_bytes()).ok())
                    .is_some();
                let ok_exit = c.wait().map(|s| s.success()).unwrap_or(false);
                ok_write && ok_exit
            }
            None => false,
        }
    };
    if !pushed {
        eprintln!("mars ssh: note — couldn't drop the installer on the remote (continuing).");
    }

    let remote_cmd = remote_session_cmd(&remote_sock, pushed);
    let mut cmd = std::process::Command::new("ssh");
    cmd.arg("-o").arg("StreamLocalBindUnlink=yes")
        .arg("-o").arg("ControlMaster=auto")
        .arg("-o").arg("ControlPersist=60s")
        .arg("-o").arg("ServerAliveInterval=30")
        .arg("-o").arg("ServerAliveCountMax=3")
        .arg("-o").arg(format!("ControlPath={}", control.display()));
    if !master_alive {
        cmd.arg("-R").arg(format!("{remote_sock}:{}", home_sock.display()));
    }
    let status = cmd
        .args(&extra)
        .arg("-t")
        .arg(&host)
        .arg(&remote_cmd)
        .status()
        .map_err(|e| anyhow::anyhow!("mars ssh: could not launch ssh: {e}"))?;
    std::process::exit(status.code().unwrap_or(1));
}
