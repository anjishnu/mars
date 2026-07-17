//! Unix adapter for the platform abstraction layer.
//!
//! Every `std::os::unix` / `libc` call the application makes lives here (plus the
//! ssh broker, which is a separate, deferred capability — see `WINDOWS_PORT.md`).
//! Each function wraps *exactly* the behavior the pre-abstraction code had, so the
//! Unix build is byte-for-byte identical after the migration.

/// Where the app's files live.
pub mod paths {
    use std::path::PathBuf;

    /// The env var `home_dir` reads — for tests that redirect the home dir.
    pub const HOME_ENV: &str = "HOME";

    /// The user's home directory (`$HOME`). All of `~/.mars`, `~/.config/mars`,
    /// and `~/.local/state/mars` are derived from this by the call sites.
    pub fn home_dir() -> Option<PathBuf> {
        std::env::var_os(HOME_ENV).map(PathBuf::from)
    }
}

/// A named, same-machine, bidirectional byte channel — the session control plane.
///
/// The wire protocol (JSON-line `ClientFrame`/`ServerFrame`) is written once
/// against `Stream: Read + Write` and never learns what's underneath. On Unix the
/// channel is a Unix-domain socket; the Windows adapter uses authenticated
/// loopback TCP with the same method surface (`read`, `write`, `flush`,
/// `try_clone`, `set_read_timeout` on `Stream`; `bind` +
/// `incoming()`/`accept()` on `Listener`).
pub mod control {
    use std::io;
    use std::path::Path;

    pub type Stream = std::os::unix::net::UnixStream;
    pub type Listener = std::os::unix::net::UnixListener;

    /// Connect to the channel at `addr` (a filesystem socket path on Unix).
    pub fn connect(addr: impl AsRef<Path>) -> io::Result<Stream> {
        Stream::connect(addr.as_ref())
    }

    /// Bind a listener at `addr`.
    pub fn bind(addr: impl AsRef<Path>) -> io::Result<Listener> {
        Listener::bind(addr.as_ref())
    }

    /// Cheap liveness probe: is there a live listener at `addr`?
    pub fn probe(addr: impl AsRef<Path>) -> bool {
        Stream::connect(addr.as_ref()).is_ok()
    }
}

/// Terminal hygiene.
pub mod tty {
    /// Restore the controlling terminal to a sane cooked mode (`OPOST|ONLCR`,
    /// `ICANON|ECHO|ECHOE|ISIG`, `ICRNL`) after a force-killed program left it
    /// raw — the "staircase output" repair. Idempotent; a no-op when stdout isn't
    /// a TTY, and run before entering the TUI so crossterm saves a *sane* state to
    /// restore on exit.
    pub fn sanitize() {
        unsafe {
            if libc::isatty(libc::STDOUT_FILENO) != 1 {
                return;
            }
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDOUT_FILENO, &mut t) == 0 {
                t.c_oflag |= libc::OPOST | libc::ONLCR;
                t.c_lflag |= libc::ICANON | libc::ECHO | libc::ECHOE | libc::ISIG;
                t.c_iflag |= libc::ICRNL;
                let _ = libc::tcsetattr(libc::STDOUT_FILENO, libc::TCSANOW, &t);
            }
        }
    }

    /// Is stdin a real terminal?
    pub fn is_stdin_tty() -> bool {
        unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
    }
}

/// Spawn a process detached from this terminal (the session daemon).
pub mod daemon {
    use std::process::Command;

    /// Configure `cmd` so the spawned child becomes a session leader, fully
    /// detached from this controlling terminal, and survives the window closing.
    pub fn detach(cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid() is async-signal-safe and the closure does nothing else.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
}

/// Process identity and lifecycle.
pub mod proc {
    /// A per-user tag used to namespace runtime sockets — the numeric uid on Unix.
    pub fn uid_tag() -> String {
        unsafe { libc::getuid() }.to_string()
    }

    /// Kill every process whose command line contains `needle`. Best-effort.
    pub fn kill_matching(needle: &str) {
        let _ = std::process::Command::new("pkill").arg("-f").arg(needle).status();
    }
}

/// Make a path private to its owning user.
pub mod fsperm {
    use std::io;
    use std::path::Path;

    /// Restrict a directory to its owner (`0700`).
    pub fn restrict_dir(path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
}

/// Which shell a new terminal pane runs.
pub mod shell {
    /// `$SHELL`, falling back to `/bin/bash` — exactly the pre-abstraction
    /// behavior of `terminal.rs`.
    pub fn default_shell() -> String {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}
