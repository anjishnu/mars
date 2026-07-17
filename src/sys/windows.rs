//! Windows adapter for the platform abstraction layer.
//!
//! Compiled and selfchecked on Windows 11 (aarch64 + x86_64 MSVC). The contract
//! each module satisfies is the Unix adapter (`sys/unix.rs`) — same signatures,
//! so the rest of the tree compiles unchanged.

/// Where the app's files live.
pub mod paths {
    use std::path::PathBuf;

    /// The env var `home_dir` reads — for tests that redirect the home dir.
    pub const HOME_ENV: &str = "USERPROFILE";

    /// The user's home directory. Windows has no `$HOME`; use `%USERPROFILE%`
    /// (falling back to `%HOMEDRIVE%%HOMEPATH%`). The call sites then append
    /// `.mars` etc., so a Windows user gets `C:\Users\me\.mars\...` — acceptable
    /// for the MVP. A later refinement can move state under `%LOCALAPPDATA%`
    /// (see the `paths` port note in WINDOWS_PORT.md).
    pub fn home_dir() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(HOME_ENV) {
            return Some(PathBuf::from(p));
        }
        match (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH")) {
            (Some(d), Some(p)) => {
                let mut s = std::ffi::OsString::from(d);
                s.push(p);
                Some(PathBuf::from(s))
            }
            _ => None,
        }
    }
}

/// A named, same-machine, bidirectional byte channel — the session control plane.
///
/// On Windows the channel is a **loopback TCP socket** with a filesystem
/// rendezvous: `bind` listens on `127.0.0.1:0` and writes `"<port> <token>"`
/// into the file at `addr`. Named pipes were the first draft, but
/// `interprocess`'s pipe streams cannot set read/write timeouts — and the
/// daemon poll loops depend on them — while `TcpStream` carries the exact
/// `UnixStream` method surface (`try_clone`, `set_read_timeout`,
/// `set_write_timeout`).
///
/// The rendezvous file keeps every Unix-socket-path semantic the callers rely
/// on: a live channel is a file that exists, a stale one is swept with
/// `remove_file`, `rename` moves a live session, and `bind` over an existing
/// file fails like `AddrInUse`. The random token (checked by `accept` before a
/// stream is surfaced) stands in for the 0700 socket directory: the file is
/// ACL-protected under the user profile, so another local user can portscan
/// the listener but cannot present the token.
pub mod control {
    use std::io::{self, Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::path::Path;
    use std::time::Duration;

    /// How long `accept` waits for a connector to present the token. Generous
    /// for a same-machine round-trip; bounds how long a broken or hostile
    /// connection can stall the daemon's accept loop.
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);
    const TOKEN_MAX: usize = 128;

    /// A connected channel end — `Read + Write + Send`, `try_clone`,
    /// `set_read_timeout`, `set_write_timeout`, mirroring `UnixStream`.
    pub struct Stream(TcpStream);

    impl Stream {
        pub fn try_clone(&self) -> io::Result<Stream> {
            self.0.try_clone().map(Stream)
        }
        pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
            self.0.set_read_timeout(dur)
        }
        pub fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
            self.0.set_write_timeout(dur)
        }
    }

    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Read for &Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            (&self.0).read(buf)
        }
    }
    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    impl Write for &Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            (&self.0).write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            (&self.0).flush()
        }
    }

    /// A bound channel — yields token-verified `Stream`s via `incoming`/`accept`.
    pub struct Listener {
        inner: TcpListener,
        token: String,
    }

    impl Listener {
        /// Accept the next *authenticated* connection. A connector that fails
        /// the token handshake is dropped and never surfaced — a hostile local
        /// process must not be able to reach the frame protocol, nor kill the
        /// daemon's accept loop with a handshake error.
        pub fn accept(&self) -> io::Result<Stream> {
            loop {
                let (stream, _) = self.inner.accept()?;
                if let Some(s) = self.handshake(stream) {
                    return Ok(s);
                }
            }
        }

        pub fn incoming(&self) -> impl Iterator<Item = io::Result<Stream>> + '_ {
            std::iter::from_fn(move || Some(self.accept()))
        }

        /// Read the token line byte-at-a-time — buffering here would swallow
        /// the connector's first protocol frame.
        fn handshake(&self, stream: TcpStream) -> Option<Stream> {
            stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).ok()?;
            let mut got = Vec::with_capacity(TOKEN_MAX);
            let mut byte = [0u8; 1];
            loop {
                match (&stream).read(&mut byte) {
                    Ok(1) if byte[0] == b'\n' => break,
                    Ok(1) if got.len() < TOKEN_MAX => got.push(byte[0]),
                    _ => return None,
                }
            }
            if got != self.token.as_bytes() {
                return None;
            }
            stream.set_read_timeout(None).ok()?;
            Some(Stream(stream))
        }
    }

    /// `"<port> <token>"` from the rendezvous file at `addr`. A missing file
    /// surfaces the same `NotFound` a missing Unix socket would.
    fn read_addr(addr: &Path) -> io::Result<(u16, String)> {
        let s = std::fs::read_to_string(addr)?;
        let mut it = s.split_whitespace();
        match (it.next().and_then(|p| p.parse::<u16>().ok()), it.next()) {
            (Some(port), Some(token)) => Ok((port, token.to_string())),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed channel rendezvous file: {}", addr.display()),
            )),
        }
    }

    /// 128 bits of operating-system randomness for the per-listener secret.
    fn fresh_token() -> io::Result<String> {
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes)
            .map_err(|e| io::Error::other(format!("channel token generation failed: {e}")))?;
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(32);
        for byte in bytes {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        Ok(out)
    }

    pub fn connect(addr: impl AsRef<Path>) -> io::Result<Stream> {
        let (port, token) = read_addr(addr.as_ref())?;
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        stream.write_all(token.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        Ok(Stream(stream))
    }

    pub fn bind(addr: impl AsRef<Path>) -> io::Result<Listener> {
        let inner = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = inner.local_addr()?.port();
        let token = fresh_token()?;
        // create_new: binding over an existing file must fail like AddrInUse
        // does on Unix — a second daemon must never hijack a live session's name.
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(addr.as_ref())?;
        f.write_all(format!("{port} {token}\n").as_bytes())?;
        Ok(Listener { inner, token })
    }

    /// Cheap liveness probe: is there a live listener behind `addr`?
    pub fn probe(addr: impl AsRef<Path>) -> bool {
        connect(addr).is_ok()
    }
}

/// Terminal hygiene.
pub mod tty {
    /// No-op on Windows: crossterm's raw-mode teardown already restores the
    /// console on exit, and there is no termios cooked-mode to repair.
    pub fn sanitize() {}

    /// Is stdin a real terminal? `std::io::IsTerminal` is cross-platform and
    /// backed by `GetConsoleMode` on Windows.
    pub fn is_stdin_tty() -> bool {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    }
}

/// Spawn a process detached from this console (the session daemon).
pub mod daemon {
    use std::process::Command;

    // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW: the daemon runs with a
    // hidden console of its own — detached enough to survive this window
    // closing, but still console-backed so ConPTY panes can spawn from it.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Detach the child from this console so it survives the window closing.
    pub fn detach(cmd: &mut Command) {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
}

/// Process identity and lifecycle.
pub mod proc {
    /// A per-user tag to namespace runtime channels. Windows has no uid; use
    /// the username (sanitized to path-safe chars).
    pub fn uid_tag() -> String {
        std::env::var("USERNAME")
            .unwrap_or_else(|_| "user".into())
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }

    /// Kill every process whose executable and arguments match `needle`
    /// (`killall`). Shells out to PowerShell's CIM sweep — the same best-effort
    /// contract as Unix `pkill -f`.
    pub fn kill_matching(needle: &str) {
        let wildcard_literal = |s: &str| {
            s.replace('\'', "''")
                .replace('`', "``")
                .replace('[', "`[")
                .replace(']', "`]")
                .replace('*', "`*")
                .replace('?', "`?")
        };
        let predicate = if let Some((program, args)) = needle.split_once(' ') {
            let program = if program.to_ascii_lowercase().ends_with(".exe") {
                program.to_string()
            } else {
                format!("{program}.exe")
            };
            let program = program.replace('\'', "''");
            let args = wildcard_literal(args.trim());
            format!("$_.Name -ieq '{program}' -and $_.CommandLine -like '*{args}*'")
        } else {
            let needle = wildcard_literal(needle);
            format!("$_.CommandLine -like '*{needle}*'")
        };
        let caller_pid = std::process::id();
        let script = format!(
            "Get-CimInstance Win32_Process | \
             Where-Object {{ {predicate} -and $_.ProcessId -ne {caller_pid} }} | \
             ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Make a path private to its owning user.
pub mod fsperm {
    use std::io;
    use std::path::Path;

    /// No-op for the MVP: a directory created under the user profile inherits
    /// an ACL that already restricts it to the owner (+ SYSTEM/Administrators).
    /// HARDENING: set an explicit owner-only DACL for parity with Unix `0700`
    /// once state can live outside the profile.
    pub fn restrict_dir(_path: &Path) -> io::Result<()> {
        Ok(())
    }
}

/// Which shell a new terminal pane runs.
pub mod shell {
    /// Windows has no `$SHELL` (and inherited ones often hold unspawnable
    /// Unix paths, e.g. under Git Bash) — prefer PowerShell 7, then Windows
    /// PowerShell, then `%ComSpec%` (cmd).
    pub fn default_shell() -> String {
        for cand in ["pwsh.exe", "powershell.exe"] {
            if on_path(cand) {
                return cand.to_string();
            }
        }
        std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string())
    }

    fn on_path(exe: &str) -> bool {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
            .unwrap_or(false)
    }
}
