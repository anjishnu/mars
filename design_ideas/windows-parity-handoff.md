# Native Windows parity: engineering handoff

**Status:** Windows-home -> Unix-remote implemented; remaining milestones proposed
**Baseline:** `a940302` (`Complete native Windows core port`)
**Primary scope:** harden the native core and define the later Windows-remote and
release work.

Read this with `WINDOWS_PORT.md` for shipped behavior and `DESIGN.md` for the
system-wide architecture. This document remains forward-looking except for the
Windows-home transport in section 4, which now describes the implementation.

**Implementation update:** the portable keyd, per-invocation authenticated relay,
mixed remote-UDS/local-TCP forward, attach-time broker-route refresh, and explicit
remote-binary/session protocol gates are now implemented. Persistent PTYs query
the daemon's current route after reattach, and Windows local control channels use
nonce/HMAC mutual authentication with non-destructive upgrade-aware liveness
classification. SSH child environments are scrubbed of provider keys. Stock
Windows OpenSSH 9.5p2 completed the mixed-forward spike and
created, detached from, and reattached an existing Mars session on an Ubuntu
OpenSSH remote under WSL. Windows as the remote, installer staging from a Windows
home, native lifecycle hardening, and distribution remain open.

## 1. Outcome and recommended order

The native Windows core is complete enough for an internal beta. It is not yet a
broad Windows release: physical-terminal behavior has not had a complete manual
pass, process-tree cleanup is not deterministic, runtime files rely on inherited
ACLs, there is no native installer/signing pipeline, and Windows cannot yet be
the SSH remote.

Implement the remaining work in this order:

1. **Beta gate:** run the real Windows Terminal matrix, push the baseline, and
   observe both Windows and Ubuntu CI jobs.
2. **Lifecycle and security:** establish a per-session Windows Job Object, replace
   the PowerShell CIM sweep with native process enumeration, and create sensitive
   runtime files with explicit DACLs.
3. **Windows-home SSH (implemented for Unix remotes):** keep validating the
   system-OpenSSH, authenticated relay, and attach-time route-refresh path.
4. **Distribution:** publish x64 and ARM64 artifacts, then add an installer and
   Authenticode signing.
5. **Path migration:** centralize config/state/cache locations before moving
   Windows data to LocalAppData.
6. **Windows as an SSH remote:** treat this as a separate project after the
   Windows-home design is proven.

The next implementation agent should start with the remaining Job Object and
secure-creation spikes in section 11, not another broad broker refactor.

## 2. Current architecture and gaps

| Area | Current implementation | Remaining gap |
|---|---|---|
| Editor and input | Shared crossterm input boundary; release events filtered in `App::apply_input` | Real terminal encodings, IME/AltGr, paste, mouse, and resize need manual coverage |
| Terminal panes | `portable-pty` / ConPTY; separate child waiter handles ConPTY's non-EOF exit behavior | Killing a shell handle does not guarantee descendant termination |
| Persistent sessions | Shared JSON-line protocol; Windows uses nonce/HMAC mutually-authenticated loopback TCP and a rendezvous file | Force cleanup shells out to PowerShell and runtime ACLs are inherited |
| Daemon lifecycle | `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP` | No Job Object contains the daemon's process tree |
| Paths | Unix-shaped locations under `%USERPROFILE%` plus `%TEMP%` runtime addresses | No Known Folder layout or migration contract |
| SSH broker | Portable keyd plus Windows authenticated relay to Unix remotes | Remote bootstrap, broader SSH matrix, and Windows-as-remote remain |
| Fleet | Portable `fleet.rs` registry with broker activity from Windows-home sessions | Windows-as-remote remains |
| Packaging | Source build with Rust/MSVC; x64 Windows CI | No native ARM64 CI, release artifacts, installer, signing, or update path |

The implementation removed the prior coupling: `broker.rs` now owns the portable
protocol/keyd service, while `ssh.rs` owns OpenSSH lifecycle, remote POSIX commands,
and the Windows relay. `broker_stub.rs` is used only when the Cargo feature is off.
The Unix path retains its two-connection `ControlMaster` installer flow; Windows
uses one foreground interactive connection and requires a preinstalled remote.

## 3. Why the remaining parity is difficult

### 3.1 It is a four-platform problem

SSH behavior is the cross-product of home OS and remote OS:

| Home | Remote | Status |
|---|---|---|
| Unix | Unix | Current implementation |
| Windows | Unix | Implemented |
| Unix | Windows | Deferred |
| Windows | Windows | Deferred |

The implemented Windows-to-Unix row preserves a remote Unix-domain socket. The
latter rows cannot
assume `/tmp`, POSIX shell commands, Unix modes, or a server-side Unix-socket
listener. Solving all rows at once would hide separate lifecycle and security
problems behind one abstraction.

### 3.2 OpenSSH option parsing is not capability proof

The stock client on the current development host is
`OpenSSH_for_Windows_9.5p2`. It accepts and prints `ControlMaster`,
`ControlPersist`, `ControlPath`, and `StreamLocalBindUnlink` through `ssh -G`.
Microsoft's OpenSSH project scope nevertheless lists client `ControlMaster` and
background SSH execution as unsupported. A parsed option must not be treated as a
working multiplexor.

The same client accepts this mixed forward syntactically:

```text
-R /tmp/mars-probe.sock:127.0.0.1:9
remoteforward /tmp/mars-probe.sock [127.0.0.1]:9
```

OpenSSH has specified mixed Unix-socket/TCP forwarding since 6.7. In the proposed
shape, Unix `sshd` creates the remote Unix listener and Windows `ssh.exe` only
connects to local TCP. That avoids Windows OpenSSH's Unix-listener gap, but it
still requires an end-to-end spike against a real Unix server.

### 3.3 Authentication cannot be copied from session control unchanged

`sys::control::connect(path)` reads a local rendezvous file and sends its token
before exposing the stream. With `ssh -R`, `ssh.exe` opens the local TCP
destination and forwards bytes from the remote socket; it does not know Mars's
token and cannot perform that handshake.

Therefore "point `-R` directly at the Windows keyd port and reuse
`sys::control`" is incomplete. Either the remote Mars process must send a
tunnel capability as its first bytes, or an authenticated local relay must
translate the tunnel capability into a normal keyd connection. This document
chooses the relay because it does not disclose keyd's long-lived local capability
to a remote host.

### 3.4 Process containment has a spawn-order race

`portable-pty` 0.8.1 exposes `Child::as_raw_handle()` on Windows, but its ConPTY
backend calls `CreateProcessW` without `CREATE_SUSPENDED` and without a Job
Object. Assigning the returned child immediately is useful but cannot prove that
the shell did not create a descendant first.

A per-session Job Object avoids that race: the Mars server assigns its own process
to the job before constructing `App` or spawning any PTY. All later descendants
inherit membership. Per-pane containment is harder and requires either a
`portable-pty` spawn hook or a suspended-create/assign/resume sequence.

### 3.5 Security is part of functional parity

The broker protects LLM credentials and can initiate paid model calls. A listener
that merely works is insufficient. Windows lacks a filesystem namespace around a
TCP port, inherited ACLs vary by deployment, remote hosts are not trusted with the
home key, and a failed forward must not look like a working agent tunnel.

### 3.6 The important tests are not all headless

`--selfcheck` can cover protocol, lifecycle, rendering, and stale-address logic.
It cannot prove physical key encodings, console-window close behavior, OpenSSH
authentication prompts, reverse-forward policy, EDR interaction, or SmartScreen
behavior. Those need named manual or VM test cases rather than optimistic skips.

## 4. Target Windows-home SSH architecture

### 4.1 Data path

```text
remote mars
  -> MARS_AUTH_SOCK=/tmp/mars-auth-<uid>-<nonce>.sock
  -> sends one bounded tunnel-capability line
  -> Unix sshd reverse-forward listener
  -> foreground Windows ssh.exe
  -> 127.0.0.1:<per-invocation relay port>
  -> relay verifies the tunnel capability
  -> relay connects to keyd through sys::control
  -> keyd performs BrokerRequest -> agent::chat
```

Properties:

- The LLM provider key remains only in keyd's home environment.
- The keyd rendezvous capability remains local and is never sent to the remote.
- The remote receives a random, invocation-scoped tunnel capability.
- The relay and tunnel die with the foreground `mars ssh` process.
- Each invocation uses a unique remote socket, so stale files and concurrent
  sessions do not collide.
- The remote command exports the exact socket and capability. Directory scanning
  remains only a compatibility fallback for manually-created sessions.

### 4.2 Local keyd endpoint

Use the existing `sys::control` capability for the home keyd endpoint:

- Unix remains a mode-restricted Unix-domain socket.
- Windows is an owner-protected rendezvous file plus token-authenticated loopback
  TCP.
- `handle_conn` becomes generic over the platform stream rather than naming
  `UnixStream`.
- keyd startup uses `sys::daemon::detach` on both platforms.
- broker paths use `sys::paths`, not direct `HOME` access.

Do not expose the session-control rendezvous format as the remote protocol. It is
a local implementation detail and includes the long-lived keyd capability.

### 4.3 Per-invocation relay

`mars ssh` creates a relay before launching `ssh.exe`:

1. Bind `127.0.0.1:0`.
2. Generate a 128-bit random tunnel capability.
3. For each accepted connection, read one newline-terminated capability with a
   short timeout and fixed maximum length.
4. Reject mismatches before parsing broker frames.
5. Connect to keyd with `sys::control::connect`, which performs the local keyd
   handshake.
6. copy bytes in both directions until either side closes.

The relay may be threads inside the foreground `mars ssh` process; it does not
justify another daemon or helper executable. Bound concurrent connections and
handshake time so local port-scanning cannot pin all workers.

The remote client sends the capability only when
`MARS_BROKER_CAPABILITY` is present. Existing Unix-to-Unix flows remain unchanged.
Increment `BROKER_VERSION` only if the JSON protocol changes; the transport
preamble itself is outside the JSON frame.

### 4.4 SSH invocation

The Windows interactive invocation should include:

```text
ssh
  -o ExitOnForwardFailure=yes
  -o ServerAliveInterval=30
  -o ServerAliveCountMax=3
  -R /tmp/mars-auth-<uid>-<nonce>.sock:127.0.0.1:<relay-port>
  <user options>
  -t <host> <remote command>
```

It must not include or rely on `ControlMaster`, `ControlPersist`, `ControlPath`,
or `ssh -O`. `ExitOnForwardFailure=yes` is required by the honesty invariant: if
the broker forward cannot bind, `mars ssh` must fail rather than opening a session
that claims agent access.

Generate the remote socket suffix and capability from an OS CSPRNG. Keep the path
within conservative Unix socket-length limits. Use shell-safe fixed alphabets
(hex) and quote all generated and user-derived remote command values through one
tested POSIX-shell quoting helper.

### 4.5 Stale sockets and concurrency

Use `/tmp/mars-auth-<home-tag>-<nonce>.sock`, not the current fixed path.
Forward creation occurs before the remote command, so that command cannot safely
remove a stale fixed path first. A unique path removes that ordering dependency.

The exact path is exported into the launched remote Mars session. Two concurrent
connections to the same host therefore remain deterministic. Normal SSH shutdown
should remove its listener; bounded remote discovery may sweep dead matching
sockets left by crashes. Never unlink a socket that answers a probe.

### 4.6 Remote bootstrap

Do not claim that installer staging and an interactive TTY can share stdin.
The current prelude pipes embedded `install.sh` over stdin; the interactive
connection needs stdin for the user.

Stage 1 had two honest choices:

1. **Implemented first slice:** require Mars to be installed on the Unix remote.
   If it is absent, print the existing install instructions and fall back to the
   login shell.
2. **Follow-up:** retain a short-lived installer prelude as a separate SSH
   invocation. Without multiplexing this may authenticate twice, especially for
   password/2FA users, and the UI must say so.

Do not hide a network download inside the interactive command, and do not add a
native SSH library merely to regain one-prompt bootstrap.

### 4.7 Code boundary

Use two substantive modules, not a directory of thin wrappers:

- `broker.rs`: portable protocol, endpoint detection, stream-generic keyd server,
  chat proxy, and local keyd lifecycle.
- `ssh.rs` (new): OpenSSH command construction, remote POSIX command generation,
  Windows relay, Unix multiplexing policy, and tunnel lifecycle.

`broker_stub.rs` remains only for builds without the `ssh` feature. The full
broker can compile on Unix and Windows; platform primitives remain in the
existing `src/sys/unix.rs` and `src/sys/windows.rs` adapters.

## 5. Windows lifecycle and security hardening

### 5.1 Per-session Job Object

Add a `sys` capability that creates and holds a Windows Job Object configured with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.

For a session server, assign the current Mars process to the job before creating
`App`. Every later ConPTY shell and descendant then inherits membership. Hold the
job handle until server exit. This makes graceful kill, force-kill of the daemon,
and crashes close the job and terminate the session tree.

For standalone mode, establish the same containment before spawning panes.

This first slice does not guarantee that closing one pane kills that pane's
grandchildren while other panes remain alive. Per-pane jobs require a
create-suspended hook in `portable-pty` or a maintained local/upstream change.
Keep that limitation explicit.

Test nested-job behavior because CI runners and enterprise launchers may already
place Mars in a Job Object. The minimum supported Windows version remains an open
product decision.

### 5.2 Native process enumeration

Replace the PowerShell `Get-CimInstance Win32_Process` sweep with native process
enumeration using Toolhelp APIs. Validate executable identity, command line where
available, PID, and process start time before termination; never kill by basename
alone.

The Job Object is primary containment. Native enumeration is recovery for daemons
created before Job Objects, corrupted rendezvous state, and upgrade transitions.
It should not become a second process supervisor.

Prefer one target-specific Windows API dependency that can also support Job
Objects and ACLs. Evaluate `windows-sys` versus `windows` in a bounded spike;
avoid adding both `sysinfo` and an ACL wrapper unless raw API safety proves worse
than the dependency cost.

### 5.3 Explicit DACLs and atomic creation

Before Windows keyd ships, protect runtime directories and rendezvous files with
an explicit, inheritance-protected DACL:

- full control for the current user SID;
- full control for `SYSTEM`;
- administrative recovery access only if that is an explicit product decision;
- no broad `Users`, `Everyone`, or anonymous ACE.

`OpenOptions::create_new(true)` already maps to Windows `CREATE_NEW`, so
rendezvous-name collision is atomic. Applying a DACL after creation introduces a
race. Sensitive files should be created with a security descriptor passed to
`CreateFileW`; directories need equivalent creation-time security attributes.
Existing legacy paths need a checked hardening/migration path.

Reject or explicitly handle reparse points before writing security-sensitive
state. Same-user processes are outside the isolation boundary: they can inspect
Mars's process and environment regardless of the DACL.

### 5.4 Named-pipe decision

Keep authenticated loopback TCP. Named pipes can enforce a logon-SID DACL and are
the stronger design for RDP/Fast User Switching isolation, but the synchronous
timeout/clone surface still does not match the control loop. Revisit only if:

- strict cross-logon-session isolation becomes a requirement; or
- Mars adopts an async runtime where Tokio's named-pipe API is a natural fit.

Do not migrate transport solely for aesthetic Unix/Windows symmetry.

## 6. Paths and migration

Centralize these capabilities before changing locations:

```text
sys::paths::config_dir()
sys::paths::state_dir()
sys::paths::runtime_dir()
sys::paths::cache_dir()
```

Proposed Windows layout:

```text
%LOCALAPPDATA%\Mars\config
%LOCALAPPDATA%\Mars\state
%LOCALAPPDATA%\Mars\run
%LOCALAPPDATA%\Mars\cache
```

Using LocalAppData for all four is simpler and avoids enterprise roaming-profile
surprises. Split true user preferences into `%APPDATA%` only if roaming is a
concrete requirement.

The migration is cross-cutting. Current call sites include `config.rs`,
`tuning.rs`, `tiers.rs`, `session.rs`, `fleet.rs`, `llm_log.rs`, `persona.rs`,
`retrieval.rs`, and `worklog.rs`. Move them behind the path capability before
moving any data.

Migration rules:

1. New installs use the new layout.
2. Existing files are copied atomically on first use; do not silently delete the
   legacy copy in the first release.
3. Running daemons using legacy runtime addresses remain discoverable until they
   exit. Do not migrate live rendezvous files.
4. Record migration completion and make retries idempotent.
5. Environment overrides used by selfcheck continue to win.

Path migration is not a prerequisite for the internal beta. Explicit DACLs are a
prerequisite for putting credential-bearing broker state in any new location.

## 7. Packaging and deployment

### 7.1 Build matrix

Continuously build and selfcheck:

- `x86_64-pc-windows-msvc`, default features;
- `x86_64-pc-windows-msvc`, no default features;
- `aarch64-pc-windows-msvc`, default features;
- `aarch64-pc-windows-msvc`, no default features.

Use a native ARM64 runner where available; an emulated x64 binary on ARM64 is not
ARM64 validation. Fresh native builds need the MSVC C++ workload and Windows SDK
because transitive native crates such as `ring` require a compiler/linker.

### 7.2 Release sequence

1. Signed or checksummed ZIP artifacts for x64 and ARM64.
2. MSI installer with PATH registration and clean uninstall.
3. Authenticode-sign both EXE and MSI in CI using protected signing credentials.
4. Add a winget manifest after stable artifact URLs and upgrade semantics exist.
5. Decide on MSIX/Store only if Store distribution becomes a product goal.

An unsigned installer is useful for engineering validation but should not be
called a broad deployment: SmartScreen and enterprise policy make signing part of
the user experience.

Do not run the interactive per-user session daemon as a Windows Service. SCM
services run in Session 0 and are the wrong ownership model for user ConPTY
sessions. A login Scheduled Task may be evaluated later only if keyd autostart is
actually desired.

### 7.3 LLM credentials

Static environment credentials work today, provided detached daemons inherit
them. Azure CLI can acquire a short-lived token, but Mars has no refreshable
credential-provider contract and the selected deployment used during this port
was blocked by Azure RBAC.

Do not persist CLI bearer tokens in config or installer state. Treat Azure
identity/token refresh as a separate agent-auth design. Windows release readiness
requires at least one documented, working provider path; it does not require
solving enterprise Azure identity in this port.

## 8. Windows as an SSH remote

Defer this until Windows-home -> Unix-remote is stable.

A Windows remote cannot use `/tmp` or rely on Windows OpenSSH to bind the same
remote Unix socket. The likely transport is a remote loopback TCP forward with a
capability, but dynamic-port allocation creates a bootstrap problem: Mars must
learn the remote port before constructing the remote process environment.

Candidate designs to investigate later:

- parse an OpenSSH allocated remote port and pass it to a remote helper;
- start a remote Mars bridge over SSH stdio;
- use a fixed, capability-protected port range with collision retries;
- adopt a native SSH library only if system OpenSSH proves unable to express the
  required channel.

Each candidate changes authentication, cleanup, or compatibility with users'
existing `ssh_config`, agents, host keys, proxies, and 2FA. None belongs in the
Windows-home milestone.

## 9. Threat model

| Threat | Required property / mitigation |
|---|---|
| Another local Windows user reads the keyd descriptor | Explicit DACL on directory and file; atomic secure creation |
| Local process scans loopback ports | 128-bit capability before broker framing; bounded handshake and workers |
| Remote host steals the provider key | Key remains in keyd; remote receives only an invocation-scoped tunnel capability |
| Compromised remote spends model quota while connected | Accepted residual risk; tunnel-scoped lifetime, visible active tunnel, optional future rate policy |
| Stale remote socket hijacks a later session | Unique CSPRNG socket path plus exact environment binding |
| Forward silently fails | `ExitOnForwardFailure=yes`; no success-shaped fallback |
| Remote command injection | Fixed-alphabet generated values and one tested POSIX-shell quoting helper |
| Daemon dies but descendants survive | Per-session Job Object with kill-on-close |
| Name-based force kill targets unrelated process | Native identity checks; Job Object is primary |
| Token leaks in logs | Never log descriptors/capabilities; redact command diagnostics |
| Protocol downgrade/mismatched binaries | Explicit broker version error and compatibility tests |

Trust boundary: Mars does not defend one process from another process running as
the same OS user. The remote host is trusted to request model work while its
tunnel is active, but is never trusted with the home provider credential.

## 10. Milestones and acceptance criteria

### M0 - Internal beta gate

- Default and no-default selfchecks pass on Windows x64, Windows ARM64, and Ubuntu.
- A clean Windows build works with documented MSVC prerequisites.
- Physical typing produces exactly one event per press; held-key repeat still
  works.
- Windows Terminal checks pass for AltGr, IME/dead keys, bracketed paste,
  clipboard, mouse wheel, resize, split panes, detach/reattach, takeover, and
  Mission Briefing.
- Closing the client window leaves the daemon alive; closing/killing the session
  removes it.

### M1 - Native hardening

- Killing a session terminates a long-running shell grandchild.
- Force-killing the Mars server also terminates the contained tree.
- Nested Job Object behavior is tested on every supported Windows release.
- `killall` does not invoke PowerShell or WMI.
- A second local user cannot read or replace runtime/keyd descriptors.
- Concurrent bind attempts produce exactly one winner and never expose a partial
  descriptor.
- Existing sessions created before the upgrade remain recoverable.

### M2 - Windows-home SSH to Unix

- Stock Windows OpenSSH connects to a supported Linux OpenSSH server.
- Key, ssh-agent, password, 2FA, first-host-key prompt, custom port, and
  `ProxyJump` paths are manually exercised.
- A brokered remote request succeeds with no provider key on the remote.
- Direct connection to the relay without the capability is rejected.
- Forward bind failure exits nonzero before launching the remote Mars session.
- Network loss and console close tear down the relay and forward.
- Two simultaneous sessions to the same remote use distinct sockets and both
  work.
- A stale socket from a crashed prior session cannot block the next connection.
- Missing/outdated remote Mars produces a clear install/version message, not a
  plain-SSH success.
- Unix-to-Unix broker behavior remains unchanged.

### M3 - Distribution

- Clean x64 and ARM64 VMs install, run `mars --selfcheck`, upgrade, and uninstall.
- PATH changes are correct without requiring a reboot.
- EXE and installer signatures verify and no secrets appear in build logs.
- Release checksums and architecture names are unambiguous.

### M4 - AppData migration

- Fresh and legacy profiles resolve the documented layout.
- Migration is idempotent and preserves config, memory, logs, fleet, and worklog.
- A running legacy daemon is not orphaned.
- Rollback to the preceding release does not destroy user data.

## 11. Required implementation spikes

### Spike A - mixed OpenSSH forward (completed)

Windows OpenSSH 9.5p2 successfully reverse-forwarded a Unix-domain socket on an
Ubuntu sshd to a Windows loopback TCP service. The full `mars ssh` path then
created `main`, detached, and reattached the existing session with a newly-minted
socket/capability delivered through `ClientFrame::Hello`.

On a real Windows host and real Unix sshd:

1. Start a token-checking TCP echo service on Windows loopback.
2. Run `ssh -vvv -o ExitOnForwardFailure=yes
   -R /tmp/mars-spike-<nonce>.sock:127.0.0.1:<port> <host>`.
3. Connect through the Unix socket and prove bidirectional bytes.
4. Test normal exit, forced `ssh.exe` termination, network loss, two concurrent
   forwards, and server policy rejection.
5. Save only sanitized logs; never include real provider credentials.

Exit criterion: mixed forwarding and cleanup behavior are empirical, not inferred
from `ssh -G`.

### Spike B - Job Object containment

Create a Job Object in the session server, assign the current process before
`App` construction, spawn a shell and grandchild through ConPTY, then terminate
the server. Repeat when Mars itself is already in a parent job.

Exit criterion: the grandchild always exits and supported nested-job environments
are known. If assignment fails under a parent job, specify the fallback before
shipping.

### Spike C - secure creation

Create runtime directory and rendezvous file with creation-time security
descriptors, inspect the resulting ACL, and attempt read/replace from a second
local account. Include reparse-point and concurrent-creation cases.

Exit criterion: there is no create-then-harden window for credential-bearing
state.

## 12. File-level implementation map

| File | Expected ownership |
|---|---|
| `Cargo.toml` | Compile full `ssh` feature on Windows; add one Windows API dependency; update feature comments |
| `src/main.rs` | Change broker module gating; initialize containment before app/PTY creation; extend selfcheck |
| `src/broker.rs` | Portable protocol/server, generic stream handling, endpoint detection, keyd lifecycle |
| `src/ssh.rs` (new) | All OpenSSH orchestration, remote commands, relay, and Unix/Windows policy |
| `src/broker_stub.rs` | No-feature build only |
| `src/sys/mod.rs` | Document new path, security, and process-containment capabilities |
| `src/sys/windows.rs` | Job Objects, native enumeration, DACL creation, Known Folder paths |
| `src/sys/unix.rs` | Matching capability signatures with existing Unix behavior |
| `src/session.rs` | Establish server containment, use centralized runtime/state paths, lifecycle tests |
| `src/terminal.rs` | Preserve ConPTY waiter; only add per-pane containment if the spawn race is solved |
| `src/config.rs`, `src/tuning.rs`, `src/tiers.rs` | Centralized config path |
| `src/fleet.rs`, `src/llm_log.rs`, `src/persona.rs`, `src/retrieval.rs`, `src/worklog.rs` | Centralized state/cache paths and migration |
| `WINDOWS_PORT.md`, `DESIGN.md`, `architecture_overview.md` | Fold in behavior as each milestone ships |
| `.github/workflows/ci.yml` | ARM64 build/selfcheck and eventual SSH integration job |
| future installer files | Add only when the packaging milestone starts; do not scaffold variants early |

## 13. Open product and engineering questions

1. What is the oldest supported Windows release? This controls nested Job Object,
   ConPTY, ARM64, and OpenSSH assumptions.
2. Is RDP/Fast User Switching isolation required? If yes, named pipes with a logon
   SID may become mandatory.
3. Should administrative recovery retain an ACE on Mars state, or should the DACL
   contain only user and `SYSTEM`?
4. Is requiring preinstalled Mars on the first Windows-home SSH milestone
   acceptable, or is a second authentication prompt for installer staging
   preferable?
5. Is pane-close descendant cleanup a release gate, or is session-level cleanup
   sufficient for the first beta?
6. Should any user preference roam through `%APPDATA%`, or should all Mars data
   stay machine-local?
7. Which installer is the supported contract: MSI, winget, both, or a signed ZIP
   first?
8. Is Windows-as-remote required for the same release as Windows-home, or can it
   remain explicitly deferred?
9. Should keyd enforce request concurrency/rate limits against a compromised
   connected remote?
10. Is Azure CLI/Entra token refresh in scope for Windows deployment, or will the
    first release document static provider credentials only?

## 14. Explicit non-goals

- Replacing system OpenSSH with `russh` in the Windows-home milestone.
- Implementing WinRM as an SSH substitute.
- Running interactive Mars sessions as an SCM service.
- Migrating the entire application to Tokio to obtain named pipes.
- Solving all home/remote OS combinations in one patch.
- Persisting provider keys or short-lived Azure bearer tokens in Mars config.
- Refactoring unrelated editor, action, layout, or retrieval code.
- Adding installer variants before one distribution contract is selected.

## 15. References

Project sources:

- `WINDOWS_PORT.md`
- `DESIGN.md` sections 7, 9, and 11
- `src/broker.rs`
- `src/sys/windows.rs`
- `src/session.rs`
- `src/terminal.rs`
- `portable-pty` 0.8.1 `Child` trait and ConPTY backend

External primary references:

- [Win32-OpenSSH project scope](https://github.com/PowerShell/Win32-OpenSSH/wiki/Project-Scope)
- [OpenSSH client configuration](https://man.openbsd.org/ssh_config)
- [OpenSSH server configuration](https://man.openbsd.org/sshd_config)
- [OpenSSH 6.7 StreamLocal release note](https://www.openssh.com/txt/release-6.7)
- [AF_UNIX comes to Windows](https://devblogs.microsoft.com/commandline/af_unix-comes-to-windows/)
- [Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Job Object extended limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information)
- [Named-pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
- [Creating a security descriptor for a new object](https://learn.microsoft.com/en-us/windows/win32/secauthz/creating-a-security-descriptor-for-a-new-object-in-c--)
- [CreateFile creation semantics](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
- [Creating a pseudoconsole session](https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session)
- [Process creation flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)
- [Windows Known Folder identifiers](https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid)
- [cargo-wix](https://github.com/volks73/cargo-wix)
