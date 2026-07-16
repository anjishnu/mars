//! Windows adapter for the platform abstraction layer.
//!
//! ⚠️  DRAFT — authored on macOS, NOT yet compiled or run on Windows. Every item
//! marked `// VERIFY:` needs a Windows toolchain in the loop. This file is
//! `#[cfg(windows)]`, so it never affects the Unix build; treat it as the
//! starting point a Windows session (Claude Code, Opus/Fable) finishes. The
//! contract each module must satisfy is the Unix adapter (`sys/unix.rs`) — match
//! its signatures exactly and the rest of the tree compiles unchanged.
//!
//! Dependencies used here (see Cargo.toml `[target.'cfg(windows)'.dependencies]`):
//!   - `interprocess` for named-pipe local sockets (the `control` capability)

/// Where the app's files live.
pub mod paths {
    use std::path::PathBuf;

    /// The user's home directory. Windows has no `$HOME`; use `%USERPROFILE%`
    /// (falling back to `%HOMEDRIVE%%HOMEPATH%`). The call sites then append
    /// `.mars` etc., so a Windows user gets `C:\Users\me\.mars\...` — acceptable
    /// for the MVP. A later refinement can move state under `%LOCALAPPDATA%`
    /// (see the `paths` port note in WINDOWS_PORT.md).
    pub fn home_dir() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("USERPROFILE") {
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
/// On Windows the channel is a **named pipe** (`\\.\pipe\...`). The callers pass a
/// filesystem-style socket `Path` (built for the Unix world); we derive the pipe
/// name from its file stem so the rest of the code is unchanged. A cleaner
/// long-term fix is an explicit `Addr` type in the port (see WINDOWS_PORT.md §5.1).
pub mod control {
    use std::io;
    use std::path::Path;

    // VERIFY: interprocess 2.x local_socket API. The types below are the shape we
    // need (Read + Write + try_clone on Stream; bind + accept on Listener). Adjust
    // imports/type paths to the crate version pinned in Cargo.toml.
    use interprocess::local_socket::{
        prelude::*, GenericNamespaced, ListenerOptions, Stream as IpcStream,
    };

    /// A connected pipe end. Must be `Read + Write + Send` and offer `try_clone`
    /// and `set_read_timeout` (the daemon poll loop uses the timeout).
    pub type Stream = IpcStream;
    /// A bound pipe server. Must offer `incoming()`/`accept()` yielding `Stream`s.
    pub type Listener = interprocess::local_socket::Listener;

    /// `work.sock` → `mars-<user>-work`, in the pipe namespace.
    fn pipe_name(addr: &Path) -> String {
        let stem = addr
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("default");
        format!("mars-{}-{}", super::proc::uid_tag(), stem)
    }

    pub fn connect(addr: impl AsRef<Path>) -> io::Result<Stream> {
        // VERIFY: GenericNamespaced::map into a Name, then IpcStream::connect.
        let name = pipe_name(addr.as_ref()).to_ns_name::<GenericNamespaced>()?;
        IpcStream::connect(name)
    }

    pub fn bind(addr: impl AsRef<Path>) -> io::Result<Listener> {
        // VERIFY: ListenerOptions::new().name(...).create_sync().
        let name = pipe_name(addr.as_ref()).to_ns_name::<GenericNamespaced>()?;
        ListenerOptions::new().name(name).create_sync()
    }

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

    // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    /// Detach the child from this console so it survives the window closing.
    /// VERIFY: DETACHED_PROCESS is usually right for a background daemon; if the
    /// daemon needs its own hidden console for ConPTY children, switch to
    /// CREATE_NO_WINDOW (0x0800) and test that PTY panes still spawn. Consider a
    /// Job Object for clean orphan-kill semantics.
    pub fn detach(cmd: &mut Command) {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

/// Process identity and lifecycle.
pub mod proc {
    /// A per-user tag to namespace runtime pipes. Windows has no uid; use the
    /// username (sanitized to pipe-safe chars).
    pub fn uid_tag() -> String {
        std::env::var("USERNAME")
            .unwrap_or_else(|_| "user".into())
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }

    /// Kill every Mars daemon (`killall`). VERIFY: Windows can't kill by
    /// command-line substring as simply as `pkill -f`. This shells out to
    /// PowerShell to match on the daemon's command line; a native
    /// CreateToolhelp32Snapshot + OpenProcess/TerminateProcess pass is the robust
    /// version. `needle` is the same marker the Unix side uses ("mars --server").
    pub fn kill_matching(needle: &str) {
        let script = format!(
            "Get-CimInstance Win32_Process | \
             Where-Object {{ $_.CommandLine -like '*{needle}*' }} | \
             ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}"
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status();
    }
}

/// Make a path private to its owning user.
pub mod fsperm {
    use std::io;
    use std::path::Path;

    /// No-op for the MVP: a directory created under the user profile inherits an
    /// ACL that already restricts it to the owner. VERIFY / HARDENING: set an
    /// explicit DACL (owner-only) for parity with Unix `0700`, especially for the
    /// key broker's material once that capability is ported.
    pub fn restrict_dir(_path: &Path) -> io::Result<()> {
        Ok(())
    }
}
