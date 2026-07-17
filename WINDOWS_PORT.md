# Windows Port — implementation status

The native Windows core MVP is implemented. Mars builds on Windows with the
default feature set, uses ConPTY for terminal panes, and supports local persistent
sessions through the same `ClientFrame` / `ServerFrame` protocol as Unix. The
Windows `--selfcheck` passes end to end; its POSIX-only tty wheel probe skips by
design, while the cross-platform wheel dispatch check still runs.

## What works

- The editor, command bar, agent, panes, tabs, clipboard, file navigation, and
  Mission Briefing.
- Terminal panes through `portable-pty` / ConPTY, including reliable process-exit
  detection and exit codes.
- Persistent sessions: create, attach, detach, takeover, list, rename, kill, and
  `killall`.
- Detached session daemons that survive the launching client.
- Default and `--no-default-features` builds.
- Portable session names and an isolated runtime-directory override for tests.
- The portable fleet registry and time-formatting helpers.

`mars ssh` and `mars keyd` remain Unix-only. Windows builds use
`broker_stub.rs`, which rejects those commands explicitly instead of silently
degrading; local LLM providers and API keys work normally.

## Platform boundary

All operating-system primitives stay behind `src/sys/`; the application core uses
capabilities rather than syscalls. `tools/check-platform-isolation.sh` enforces
that rule. The Unix-only SSH broker is the sole documented exemption.

| Capability | Unix adapter | Windows adapter |
|---|---|---|
| Home directory | `$HOME` | `%USERPROFILE%`, then `%HOMEDRIVE%%HOMEPATH%` |
| Session control | Unix-domain socket | authenticated loopback TCP + rendezvous file |
| TTY hygiene | termios repair | crossterm console restore |
| Daemon detach | `setsid` | detached process-group creation flags |
| Process sweep | `pkill -f` | PowerShell CIM sweep |
| Directory privacy | mode `0700` | inherited user-profile ACL |
| Default shell | `$SHELL`, then `/bin/bash` | PowerShell 7, Windows PowerShell, then `%ComSpec%` |

### Windows session transport

`sys::control::bind` listens on `127.0.0.1:0` and creates the usual
`<name>.sock` address file containing a random token and the selected port.
Connectors must present that token before the listener exposes the stream to the
frame protocol. This preserves the file semantics session management needs
(existence, stale cleanup, rename, and collision detection) while providing the
`try_clone` and timeout behavior the daemon requires.

Named pipes were rejected for the MVP because the available stream API did not
provide the read/write timeout surface already used by the session protocol.

### ConPTY process lifecycle

ConPTY can keep its output pipe open after the shell process has exited, so EOF is
not a reliable lifecycle signal. `terminal.rs` moves the child handle to a watcher
thread blocked in `Child::wait` and retains a cloned `ChildKiller` in `Term`.
Natural exit records the code and emits one `TermEvent::Exited`; dropping a pane
suppresses that event and kills the child.

### Session paths and names

The normal runtime root is the platform temp directory under
`mars-<user-tag>`. `MARS_RUNTIME_DIR` overrides the base for hermetic execution;
`--selfcheck` always uses it, so its `killall` test cannot discover real sessions.

Session names use a portable subset on every OS. Path separators, traversal
components, Windows-invalid characters, trailing dots, surrounding whitespace,
and device names such as `NUL`, `CON`, `COM1`, and `LPT9` are rejected centrally.

## Verification

Windows PowerShell:

```powershell
cargo build --locked
.\target\debug\mars.exe --selfcheck
cargo build --locked --no-default-features
.\target\debug\mars.exe --selfcheck
```

Unix:

```bash
cargo build --locked
./target/debug/mars --selfcheck
cargo build --locked --no-default-features
./target/debug/mars --selfcheck
./tools/check-platform-isolation.sh
```

`.github/workflows/ci.yml` runs both selfcheck SKUs on `ubuntu-latest` and
`windows-latest`, plus the platform-isolation lint on Ubuntu.

## Remaining work

1. Complete a manual Windows Terminal pass for physical key encodings, mouse,
   clipboard, window-close daemon survival, detach/reattach, and Mission Briefing.
2. Replace the inherited-ACL assumption with an explicit owner-only DACL if Mars
   moves runtime state outside the user profile.
3. Replace the best-effort PowerShell process sweep with a native process walk if
   `killall` reliability proves insufficient.
4. Design Windows-home support for `mars ssh` / `mars keyd`; OpenSSH
   `ControlMaster` and Unix-socket forwarding do not have direct Windows parity.
5. Consider moving Windows config/state from Unix-shaped dot directories to the
   appropriate AppData locations as a separate migration.
