# Windows Port — status & handoff

This branch (`windows-port`) lays the **platform-abstraction foundation** for a
Windows build of Mars and migrates the core off Unix primitives. It is a starting
point authored on macOS: the Unix build is fully green, and the Windows adapter is
drafted but **not yet compiled on Windows**. This document is self-contained — a
fresh session on a Windows machine (Claude Code with Opus/Fable) should be able to
finish the port from here.

> Fuller design rationale lives in `design_ideas/windows-port.md` (git-ignored,
> local-only). The essentials are reproduced below so nothing is lost.

---

## TL;DR for the next session

- **What's done:** a `src/sys/` platform-abstraction layer (ports + Unix adapter +
  Windows adapter draft) and the migration of *all* platform coupling in the app
  core (`session.rs`, `agent.rs`, and every `$HOME` read) onto it. The Unix build
  is byte-for-byte unchanged and **green** (`--selfcheck` passes on both SKUs).
- **What's next (in order):** (1) compile & fix `src/sys/windows.rs` on a Windows
  toolchain; (2) handle the **ssh broker** — the one module still using Unix
  primitives directly and the only thing blocking a full Windows compile; (3) the
  one-line shell default in `terminal.rs`; (4) a real-terminal pass + CI matrix.
- **The rule that keeps it clean:** no code outside `src/sys/` may name a platform
  primitive (`std::os::*`, `libc`, `windows_sys`, `interprocess`). Enforced by
  `tools/check-platform-isolation.sh`. Keep it green as you work.
- **Verify anytime:** `cargo build && ./target/debug/mars --selfcheck` (Unix),
  `./tools/check-platform-isolation.sh` (isolation lint).

---

## Design principles (why it's shaped this way)

The port is **not an app rewrite** — the editor, UI, agent, layout, and
terminal-pane rendering already ride on cross-platform crates (`crossterm`,
`portable-pty` → ConPTY on Windows, `ratatui`, `vt100`, `arboard`) with no OS
coupling. The risk is **leakage**: `#[cfg(windows)]` metastasizing through every
file. Three principles prevent that:

1. **Abstract capabilities, not syscalls.** The port grain is one module per
   capability the app needs — *"a named local channel"*, *"where my files live"*,
   *"spawn a detached process"* — not a trait per `setsid`/`getuid`. Six ports
   cover the whole surface.

2. **One dependency rule (enforced).** No module outside `src/sys/` may name a
   platform primitive. Everything platform-specific is reached through
   `sys::<capability>`. `tools/check-platform-isolation.sh` fails the build on a
   violation, so leaks are caught before they spread.

3. **Ports & adapters (hexagonal), dependency pointing inward.**

   ```
   app core (unchanged) ──uses──▶ systems logic (session.rs) ──depends──▶
        sys:: PORTS ◀──implements── sys/unix.rs  &  sys/windows.rs (adapters)
   ```

   Adapters know the ports; ports know nothing of adapters; the app core never
   learns what OS it's on. Each adapter exposes the **same** capability modules
   with the **same** signatures — that shared signature *is* the port.

---

## What this MVP did

### 1. The platform abstraction layer — `src/sys/`

| File | Role |
|---|---|
| `sys/mod.rs` | `#[cfg]`-selects the adapter (`unix` or `windows`) and re-exports it as `sys::*`. The ONLY `#[cfg(unix)]`/`#[cfg(windows)]` in the tree. |
| `sys/unix.rs` | Unix adapter — wraps *today's exact code*, so the Unix build is unchanged. |
| `sys/windows.rs` | Windows adapter — **draft, uncompiled**; every spot needing a Windows toolchain is marked `// VERIFY:`. |

**The six capability ports** (each a module with matching signatures in both
adapters):

- `sys::paths` — `home_dir()` (Unix `$HOME` / Windows `%USERPROFILE%`).
- `sys::control` — the session IPC channel: `type Stream`, `type Listener`,
  `connect(addr)`, `bind(addr)`, `probe(addr)`. Unix = Unix-domain socket;
  Windows = named pipe (`interprocess`). The wire protocol (`ClientFrame` /
  `ServerFrame` JSON-lines) is written once against `Stream: Read + Write`.
- `sys::tty` — `sanitize()` (termios repair / no-op on Windows), `is_stdin_tty()`.
- `sys::daemon` — `detach(&mut Command)` (setsid / `DETACHED_PROCESS`).
- `sys::proc` — `uid_tag()` (uid / username, for socket namespacing),
  `kill_matching(needle)` (`pkill -f` / `taskkill`).
- `sys::fsperm` — `restrict_dir(path)` (`0700` / ACL no-op).

### 2. Core migrated onto the ports (Unix build byte-identical)

- **`$HOME` → `sys::paths::home_dir()`** across `config.rs`, `llm_log.rs`,
  `persona.rs`, `retrieval.rs` (×3), `worklog.rs`, `session.rs` (state dir).
- **`session.rs`** — Unix sockets → `sys::control`; `sanitize_tty` → `sys::tty`;
  `socket_dir` uid/chmod → `sys::proc` + `sys::fsperm`; daemon `setsid` →
  `sys::daemon::detach`; `killall`'s `pkill` → `sys::proc::kill_matching`;
  stdin `isatty` → `sys::tty::is_stdin_tty`.
- **`agent.rs`** — the broker-socket liveness probe → `sys::control::probe`.
- **`main.rs` selfcheck** — the daemon test client/listeners → `sys::control`,
  `getuid` → `sys::proc::uid_tag`. So the *tests* are portable too.

### 3. Dependencies & lint

- `Cargo.toml`: `libc` is now `[target.'cfg(unix)'.dependencies]`; `interprocess`
  is `[target.'cfg(windows)'.dependencies]`. Neither is pulled on the other OS.
- `tools/check-platform-isolation.sh` — the enforced dependency rule.

**Verification (all green on macOS/Unix):** `cargo build` and
`cargo build --no-default-features` both compile; `--selfcheck` passes on both;
the isolation lint passes.

---

## Remaining work (ordered checklist for a Windows machine)

### Step 1 — Compile and fix `src/sys/windows.rs`
It's a structured draft; the compiler will point at the gaps. Grep for `VERIFY:`.
Focus points:
- **`control` (named pipes via `interprocess`).** The 2.x `local_socket` API
  (`GenericNamespaced`, `ListenerOptions::create_sync`, `Stream::connect`) needs
  its exact types confirmed. The **contract to satisfy is `sys/unix.rs`**: `Stream`
  must be `Read + Write + Send` with `try_clone()` and `set_read_timeout()`;
  `Listener` must yield `Stream`s via `incoming()`/`accept()`. If `interprocess`'s
  types don't offer `try_clone`/`set_read_timeout`, wrap them (or hand-roll named
  pipes with `windows-sys`).
- **`daemon::detach`** — `DETACHED_PROCESS` is the default guess; if ConPTY panes
  fail to spawn from the daemon, try `CREATE_NO_WINDOW` and/or a Job Object.
- **`proc::kill_matching`** — the PowerShell/`Win32_Process` sweep is a placeholder;
  a native `CreateToolhelp32Snapshot` pass is more robust.
- **Addressing seam:** callers pass Unix-style socket `Path`s; the Windows adapter
  derives a pipe name from the path's file stem (`control::pipe_name`). Works, but
  an explicit `Addr` type in the port would be cleaner (see design doc §5.1).

### Step 2 — The ssh broker (`src/broker.rs`) — the one compile blocker
`broker.rs` is the **only** module still using Unix primitives directly, and it's
**exempt from the lint** on purpose: it's the deferred capability (Unix sockets +
OpenSSH `ControlMaster` + unix-socket `-R` forwarding, none of which Windows-as-home
supports well). To get a Windows build:
- **Recommended MVP path — compile it out.** `broker.rs` mixes ssh/socket code with
  two *portable* helpers that the rest of the app uses: `ago()` (a "5m ago"
  formatter, ~7 call sites) and the **fleet registry** (`fleet_load/record/status`,
  `resolve_target`, JSON files). Extract those two into a portable module (e.g.
  `ago` → `worklog.rs`; fleet → a small `fleet.rs`), then gate the ssh/socket
  remainder behind a `ssh` cargo feature (default-on) and `#[cfg(feature="ssh")]`
  the `mars ssh`/`mars keyd` dispatch in `main.rs`. Windows MVP builds
  `--no-default-features --features memory` → local sessions, no fleet, no ssh (a
  clean product story). This is the smallest path to "it compiles."
- **Full parity (later):** port the broker's control channel through `sys::control`
  too and redesign the tunnel (Windows OpenSSH has no `ControlMaster`; likely a TCP
  tunnel). Do **Windows-as-remote first** (the forwarded socket lands on a Linux
  box — most existing code works).

### Step 3 — `terminal.rs` shell default (one line)
`terminal.rs` spawns `$SHELL` and falls back to `/bin/bash`. On Windows `$SHELL` is
unset. Fall back to `%ComSpec%` / `powershell.exe`. (ConPTY itself is already
handled by `portable-pty` — no other change.)

### Step 4 — Real-terminal pass + CI
- **Real-terminal pass:** `--selfcheck` uses `ratatui::TestBackend` and can't verify
  Windows Terminal byte encodings / chord handling (same caveat `DESIGN.md` §9
  documents for Unix). Do a manual pass: keybindings, splits, PTY panes, detach/
  reattach, the Mission Briefing.
- **CI matrix:** run `--selfcheck` on `windows-latest` alongside Unix, and run
  `tools/check-platform-isolation.sh` as a lint step.
- **Optional testing dividend:** a `sys/fake.rs` in-memory `control` adapter (a pipe
  pair) makes the daemon tests hermetic and kills the known os-22 socket flake.

---

## What must NOT change

The application core is out of bounds — `app.rs`, `ui.rs`, `agent.rs` (beyond the
one probe already migrated), `briefing.rs`, the editor, layout, tuning, prompts,
persona. If a change there needs a `#[cfg]`, that's a smell: the right fix is a new
capability in `sys::`. Keep `tools/check-platform-isolation.sh` green.

## Build / verify reference

```bash
# Unix (must stay green throughout):
cargo build && ./target/debug/mars --selfcheck
cargo build --no-default-features && ./target/debug/mars --selfcheck   # memory-free SKU
./tools/check-platform-isolation.sh                                    # the dependency rule

# Windows MVP target (after Steps 1–3), on a Windows machine:
cargo build --no-default-features --features memory                    # no ssh/broker
cargo run  --no-default-features --features memory -- --selfcheck
```

## Files touched on this branch

- **New:** `src/sys/{mod,unix,windows}.rs`, `tools/check-platform-isolation.sh`,
  `WINDOWS_PORT.md`.
- **Migrated (Unix-identical):** `src/{session,agent,config,llm_log,persona,retrieval,worklog}.rs`,
  `src/main.rs` (module registration + test sockets), `Cargo.toml` (deps).
- **Untouched / deferred:** `src/broker.rs` (Step 2), `src/terminal.rs` (Step 3).
