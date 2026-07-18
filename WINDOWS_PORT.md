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
- Windows-home `mars keyd` and `mars ssh` handoff to Unix remotes, including
  automatic remote bootstrap and detach/reattach of an existing remote Mars
  session.

Windows as the SSH remote remains unsupported. Windows-home stages the same
embedded Unix `install.sh` as Unix-home in a separate bootstrap connection, but
does not use OpenSSH `ControlMaster`.

## Platform boundary

All operating-system primitives stay behind `src/sys/`; the application core uses
capabilities rather than syscalls. `tools/check-platform-isolation.sh` enforces
that rule, including for the SSH broker.

| Capability | Unix adapter | Windows adapter |
|---|---|---|
| Home directory | `$HOME` | `%USERPROFILE%`, then `%HOMEDRIVE%%HOMEPATH%` |
| Session control | Unix-domain socket | authenticated loopback TCP + rendezvous file |
| TTY hygiene | termios repair | crossterm console restore |
| Daemon detach | `setsid` | detached process-group creation flags |
| Process sweep | `pkill -f` | PowerShell CIM exact-name sweep of every other `mars.exe` |
| Directory privacy | mode `0700` | inherited user-profile ACL |
| Default shell | `$SHELL`, then `/bin/bash` | PowerShell 7, Windows PowerShell, then `%ComSpec%` |

### Windows session transport

`sys::control::bind` listens on `127.0.0.1:0` and creates the usual
`<name>.sock` address file containing a random token and the selected port.
Connector and listener prove possession of that token with nonce-bound
HMAC-SHA256 before either side exposes the stream to the frame protocol. Mutual
authentication prevents a process that rebinds a stale recorded port from
impersonating keyd or a session daemon. This preserves the file semantics session
management needs (existence, stale cleanup, rename, and collision detection)
while providing the `try_clone` and timeout behavior the daemon requires.
Liveness probes classify authenticated, definitively dead, and indeterminate
endpoints separately. Legacy descriptors and handshake timeouts are retained and
produce a restart/upgrade message; only a refused or missing endpoint is swept.

Named pipes were rejected for the MVP because the available stream API did not
provide the read/write timeout surface already used by the session protocol.

### Windows-home SSH handoff

`mars keyd` uses the same platform control capability as sessions: a Unix socket
on Unix and authenticated loopback TCP plus an owner-profile rendezvous file on
Windows. Each Windows `mars ssh` invocation creates a second, short-lived
loopback relay with its own random capability, then launches stock `ssh.exe` with:

```text
-o ExitOnForwardFailure=yes
-R /tmp/mars-auth-cap-<home>-<nonce>.sock:127.0.0.1:<relay-port>
```

Before any `ssh.exe` launch, Mars removes every supported provider credential
from the child environment. This keeps custom OpenSSH `SendEnv` rules from
exporting a home API key; SSH authentication variables such as `SSH_AUTH_SOCK`
remain available.

The Unix sshd owns the remote Unix-socket listener; Windows OpenSSH only makes an
outbound local TCP connection. The remote Mars client sends the tunnel capability
before the normal JSON broker frame. The relay verifies it, opens an authenticated
local keyd connection, and proxies request/response frames. The provider key never
enters ssh, the relay, or the remote environment.

The current socket and capability ride in the session `Hello` frame. A persistent
remote daemon therefore replaces its dead prior route whenever a new client
attaches; its buffers, panes, PTYs, and agent access all survive detach/reattach.
Capability-marked sockets are never selected by unauthenticated `/tmp` discovery.
The remote command first requires the capability-handoff protocol marker, and
the `Hello` version includes the session protocol, so an outdated binary or
persistent daemon fails with an upgrade/restart message instead of losing agent
access silently. Mars subprocesses launched from a persistent terminal pane use
`MARS_SESSION` plus an immutable instance ID to query that daemon's current route,
so `mars ask` and nested attach operations do not reuse the shell's first,
expired tunnel or lose routing when the session is renamed.
Nested session daemons start with the parent identity and route variables removed;
their attaching client supplies the current route explicitly.

This path deliberately does not use `ControlMaster` or `ControlPersist`, which
stock Windows OpenSSH parses as configuration but does not implement as the Unix
client does.

Before opening the interactive connection, Windows starts a short-lived SSH
prelude that writes the embedded `install.sh` to `~/.mars/install.sh`. It runs the
script only when Mars is absent or does not support the required handoff protocol,
then verifies the installed binary. Mars normalizes the embedded script to Unix
line endings before sending it, even when the home binary was built from a Windows
checkout. This keeps installer bytes separate from the interactive TTY, at the
cost of a second password/2FA prompt where authentication is not cached.

`mars killall` first asks every reachable session to autosave and exit. Its Windows
recovery sweep then force-stops every other process whose executable name is
exactly `mars.exe`, followed by capability-marked `ssh.exe` forwards. This broad
reset behavior is intentional for the beta; the invoking `mars killall` process is
excluded.

### ConPTY process lifecycle

ConPTY can keep its output pipe open after the shell process has exited, so EOF is
not a reliable lifecycle signal. `terminal.rs` moves the child handle to a watcher
thread that polls for exit and accepts kill requests from `Term`. Natural exit
records the code, allows a bounded final-output drain, and emits one
`TermEvent::Exited`; dropping a pane suppresses that event and asks the
handle-owning watcher to kill the child.

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

The mixed reverse forward and full handoff were also exercised with the Windows
9.5p2 OpenSSH client against an Ubuntu OpenSSH server under WSL: the remote Unix
socket reached a Windows TCP service, `mars ssh` created `main`, detach left it
running, and a second invocation attached the existing session with a fresh
broker route.

## Remaining work

The reviewed medium-level design, threat model, milestones, and file ownership map
are in [`design_ideas/windows-parity-handoff.md`](design_ideas/windows-parity-handoff.md).
That document is a proposal; this file remains the description of shipped behavior.

1. Complete a manual Windows Terminal pass for physical key encodings, mouse,
   clipboard, window-close daemon survival, detach/reattach, and Mission Briefing.
2. Add a per-session Job Object and eventually replace the accepted broad
   PowerShell recovery sweep with native process enumeration.
3. Protect credential-bearing runtime state with creation-time explicit DACLs.
4. Expand the Windows-home SSH matrix beyond the proven Windows-OpenSSH to Ubuntu
   path (password/2FA, jump hosts, policy failures, and network loss), then design
   Windows-as-remote separately.
5. Consider moving Windows config/state from Unix-shaped dot directories to the
   appropriate AppData locations as a separate migration.
6. Add native ARM64 CI, release artifacts, an installer, and code signing.
