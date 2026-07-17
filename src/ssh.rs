//! System-OpenSSH orchestration for `mars ssh`.
//!
//! Unix keeps connection multiplexing for its established path. Windows owns one
//! foreground ssh process and bridges its TCP destination to keyd through a
//! per-invocation capability relay; provider credentials never enter ssh.exe or
//! the remote environment.

use anyhow::Result;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const RELAY_CAPABILITY_MAX: usize = 128;
const RELAY_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_RESPONSE_TIMEOUT: Duration = Duration::from_secs(45);
const RELAY_ACCEPT_IDLE: Duration = Duration::from_millis(20);
const RELAY_CONNECTION_LIMIT: usize = 32;

/// The installer is embedded so the Unix prelude can stage the exact script
/// shipped with this binary without relying on GitHub being reachable.
pub const INSTALL_SH: &str = include_str!("../install.sh");

pub(crate) struct BrokerRelay {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl BrokerRelay {
    pub(crate) fn start(home_addr: &Path, capability: &str) -> Result<Self> {
        validate_capability(capability)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let home_addr = home_addr.to_path_buf();
        let capability: Arc<[u8]> = Arc::from(capability.as_bytes());
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let worker = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if active.fetch_add(1, Ordering::AcqRel) >= RELAY_CONNECTION_LIMIT {
                            active.fetch_sub(1, Ordering::AcqRel);
                            continue;
                        }
                        let home_addr = home_addr.clone();
                        let capability = capability.clone();
                        let active = active.clone();
                        std::thread::spawn(move || {
                            let _ = relay_connection(stream, &home_addr, &capability);
                            active.fetch_sub(1, Ordering::AcqRel);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(RELAY_ACCEPT_IDLE);
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            addr,
            stop,
            worker: Some(worker),
        })
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for BrokerRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.addr);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn validate_capability(capability: &str) -> Result<()> {
    if capability.is_empty()
        || capability.len() > RELAY_CAPABILITY_MAX
        || capability.bytes().any(|b| matches!(b, b'\r' | b'\n'))
    {
        anyhow::bail!("invalid broker tunnel capability");
    }
    Ok(())
}

fn read_capability(stream: &TcpStream) -> std::io::Result<Vec<u8>> {
    let deadline = Instant::now() + RELAY_HANDSHAKE_TIMEOUT;
    let mut got = Vec::with_capacity(RELAY_CAPABILITY_MAX);
    let mut byte = [0u8; 1];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "broker tunnel authentication timed out",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        match (&*stream).read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => break,
            Ok(1) if byte[0] != b'\r' && got.len() < RELAY_CAPABILITY_MAX => got.push(byte[0]),
            Ok(1) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid broker tunnel capability",
                ));
            }
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "broker tunnel closed during authentication",
                ));
            }
            Ok(_) => unreachable!(),
            Err(e) => return Err(e),
        }
    }
    stream.set_read_timeout(None)?;
    Ok(got)
}

fn capability_matches(expected: &[u8], got: &[u8]) -> bool {
    if expected.len() != got.len() {
        return false;
    }
    expected
        .iter()
        .zip(got)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn relay_connection(
    mut remote: TcpStream,
    home_addr: &Path,
    capability: &[u8],
) -> std::io::Result<()> {
    let got = read_capability(&remote)?;
    if !capability_matches(capability, &got) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "broker tunnel capability rejected",
        ));
    }

    let mut home = crate::sys::control::connect(home_addr)?;
    home.set_read_timeout(Some(RELAY_RESPONSE_TIMEOUT))?;
    let mut remote_reader = BufReader::new(remote.try_clone()?);
    let mut home_reader = BufReader::new(home.try_clone()?);
    let mut request = Vec::new();
    let mut response = Vec::new();
    loop {
        request.clear();
        if remote_reader.read_until(b'\n', &mut request)? == 0 {
            break;
        }
        home.write_all(&request)?;
        home.flush()?;

        response.clear();
        if home_reader.read_until(b'\n', &mut response)? == 0 {
            break;
        }
        remote.write_all(&response)?;
        remote.flush()?;
    }
    Ok(())
}

/// The remote command for the Unix prelude connection. A live multiplexed
/// forward must not be swept, because unlinking it orphans the listener inode.
pub fn remote_prelude_cmd(remote_sock: &str, sweep: bool) -> String {
    let rm = if sweep {
        format!("rm -f {remote_sock}; ")
    } else {
        String::new()
    };
    format!("{rm}mkdir -p ~/.mars && cat > ~/.mars/install.sh && chmod +x ~/.mars/install.sh")
}

pub fn remote_session_cmd(remote_sock: &str, pushed: bool) -> String {
    remote_session_cmd_with_capability(remote_sock, pushed, None)
}

pub fn remote_session_cmd_with_capability(
    remote_sock: &str,
    pushed: bool,
    capability: Option<&str>,
) -> String {
    let capability_export = capability
        .map(|cap| format!("export {}={cap}; ", crate::broker::BROKER_CAPABILITY_ENV))
        .unwrap_or_default();
    let protocol_check = capability
        .map(|_| {
            format!(
                r#"MARS_BROKER_PROTOCOL="$("$M" --broker-handoff-version 2>/dev/null || true)"; if [ "$MARS_BROKER_PROTOCOL" != "{}" ]; then printf '[mars] remote Mars is outdated for broker handoff — upgrade mars-terminal\n' >&2; exit 2; fi; "#,
                crate::broker::BROKER_HANDOFF_PROTOCOL
            )
        })
        .unwrap_or_default();
    remote_session_cmd_inner(
        Some(remote_sock),
        pushed,
        &capability_export,
        &protocol_check,
    )
}

#[cfg(windows)]
fn remote_session_cmd_without_tunnel(pushed: bool) -> String {
    remote_session_cmd_inner(None, pushed, "", "")
}

fn remote_session_cmd_inner(
    remote_sock: Option<&str>,
    pushed: bool,
    capability_export: &str,
    protocol_check: &str,
) -> String {
    let nudge = if pushed {
        "printf '[mars] not installed here — installer is ready. Run:\\n  sh ~/.mars/install.sh\\n'"
    } else {
        "printf '[mars] not installed here. Install:\\n  \
         curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust toolchain (>=1.85)\\n  \
         . \"$HOME/.cargo/env\" && cargo install mars-terminal --locked\\n'"
    };
    let (tunnel_status, exports) = match remote_sock {
        Some(sock) => (
            format!(
                "if [ -S {sock} ]; then \
                 printf '[mars] agent tunnel ready — your home key answers here\\n'; else \
                 printf '[mars] no agent tunnel (forward failed?) — the agent needs a key on this box\\n'; fi; "
            ),
            format!("export MARS_AUTH_SOCK={sock}; {capability_export}"),
        ),
        None => (
            "printf '[mars] no agent tunnel — the agent needs a key on this box\\n'; ".to_string(),
            String::new(),
        ),
    };
    format!(
        "{tunnel_status}\
         M=\"$(command -v mars 2>/dev/null)\"; \
         if [ -z \"$M\" ] && [ -x \"$HOME/.cargo/bin/mars\" ]; then M=\"$HOME/.cargo/bin/mars\"; fi; \
         if [ -z \"$M\" ] && [ -x \"$HOME/.local/bin/mars\" ]; then M=\"$HOME/.local/bin/mars\"; fi; \
         {exports}\
         if [ -n \"$M\" ]; then {protocol_check}\"$M\" attach 2>/dev/null || exec \"$M\" new main; else \
         {nudge}; exec ${{SHELL:-/bin/sh}} -l; fi"
    )
}

#[cfg(unix)]
fn remote_socket_path() -> String {
    format!("/tmp/mars-auth-{}.sock", crate::sys::proc::uid_tag())
}

#[cfg(windows)]
fn random_hex(bytes: usize) -> Result<String> {
    let mut raw = vec![0u8; bytes];
    getrandom::getrandom(&mut raw)
        .map_err(|e| anyhow::anyhow!("broker capability generation failed: {e}"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes * 2);
    for byte in raw {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

fn ssh_status(status: std::process::ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

pub(crate) fn ssh_command() -> std::process::Command {
    let mut command = std::process::Command::new("ssh");
    for name in crate::agent::PROVIDER_CREDENTIAL_ENV_VARS {
        command.env_remove(name);
    }
    command
}

#[cfg(windows)]
pub fn ssh_main(host: String, extra: Vec<String>) -> Result<()> {
    let home_addr = crate::broker::broker_socket_path()?;
    let keyd_ready = crate::broker::ensure_keyd(&home_addr);
    crate::fleet::fleet_record(&host, None);

    if !keyd_ready {
        let status = ssh_command()
            .args(&extra)
            .arg("-t")
            .arg(&host)
            .arg(remote_session_cmd_without_tunnel(false))
            .status()
            .map_err(|e| anyhow::anyhow!("mars ssh: could not launch ssh: {e}"))?;
        return ssh_status(status);
    }

    let capability = random_hex(16)?;
    let nonce = random_hex(12)?;
    let home_tag: String = crate::sys::proc::uid_tag().chars().take(24).collect();
    let remote_sock = format!("/tmp/mars-auth-cap-{home_tag}-{nonce}.sock");
    let relay = BrokerRelay::start(&home_addr, &capability)?;
    let remote_cmd = remote_session_cmd_with_capability(&remote_sock, false, Some(&capability));
    let forward = format!("{remote_sock}:127.0.0.1:{}", relay.addr().port());

    let status = ssh_command()
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-R")
        .arg(forward)
        .args(&extra)
        .arg("-t")
        .arg(&host)
        .arg(remote_cmd)
        .status()
        .map_err(|e| anyhow::anyhow!("mars ssh: could not launch ssh: {e}"))?;
    drop(relay);
    ssh_status(status)
}

#[cfg(unix)]
pub fn ssh_main(host: String, extra: Vec<String>) -> Result<()> {
    let home_sock = crate::broker::broker_socket_path()?;
    crate::broker::ensure_keyd(&home_sock);

    crate::fleet::fleet_record(&host, None);
    let remote_sock = remote_socket_path();
    let control = home_sock.with_file_name("cm-%r@%h:%p");
    if let Some(dir) = control.parent() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with("cm-") {
                    continue;
                }
                let alive = ssh_command()
                    .arg("-O")
                    .arg("check")
                    .arg("-o")
                    .arg(format!("ControlPath={}", e.path().display()))
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

    let master_alive = ssh_command()
        .arg("-O")
        .arg("check")
        .arg("-o")
        .arg(format!("ControlPath={}", control.display()))
        .args(&extra)
        .arg(&host)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let pushed = {
        let mut child = ssh_command()
            .arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg("ControlPersist=60s")
            .arg("-o")
            .arg("ServerAliveInterval=30")
            .arg("-o")
            .arg("ServerAliveCountMax=3")
            .arg("-o")
            .arg(format!("ControlPath={}", control.display()))
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
    let mut cmd = ssh_command();
    cmd.arg("-o")
        .arg("StreamLocalBindUnlink=yes")
        .arg("-o")
        .arg("ControlMaster=auto")
        .arg("-o")
        .arg("ControlPersist=60s")
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg(format!("ControlPath={}", control.display()));
    if !master_alive {
        cmd.arg("-R")
            .arg(format!("{remote_sock}:{}", home_sock.display()));
    }
    let status = cmd
        .args(&extra)
        .arg("-t")
        .arg(&host)
        .arg(&remote_cmd)
        .status()
        .map_err(|e| anyhow::anyhow!("mars ssh: could not launch ssh: {e}"))?;
    ssh_status(status)
}
