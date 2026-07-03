/// Session daemon: tmux/zellij-style detach/reattach.
///
/// Architecture (recorded in key_design.md §H2): thin client, server renders.
/// The server runs the entire `App` headless and streams ratatui's ANSI bytes
/// over a unix socket as `Output` frames; the client owns the real TTY,
/// forwards serialized input events, and writes frames verbatim to stdout.
/// One client per session; a new attach takes over. Disconnect leaves the
/// session (buffers, panes, shells, agent threads) running.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use crossterm::event::{Event, KeyEvent, MouseEvent};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    Terminal, TerminalOptions, Viewport,
};
use serde::{Deserialize, Serialize};

use crate::{
    app::{App, InputEvent},
    ui,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Protocol ─────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub enum ClientFrame {
    Hello { cols: u16, rows: u16, version: String },
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize { cols: u16, rows: u16 },
    /// One-shot query: reply with `Status`, then close (used by `mars ls`).
    Status,
    /// Terminate the session daemon (used by `mars kill <name>`).
    Kill,
    /// Rename the session (used by `mars rename <old> <new>`).
    Rename { to: String },
}

#[derive(Serialize, Deserialize)]
pub enum ServerFrame {
    /// One rendered frame's ANSI bytes (base64).
    Output { b64: String },
    /// Connection is over (detach, quit, takeover, refusal) — show `message`.
    Exit { message: String },
    /// Reply to `ClientFrame::Status`.
    Status { attached: bool, version: String },
}

pub fn write_frame<T: Serialize>(w: &mut impl Write, frame: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(frame).map_err(io::Error::other)?;
    line.push('\n');
    w.write_all(line.as_bytes())?;
    w.flush()
}

fn send_exit(stream: &UnixStream, message: &str) -> io::Result<()> {
    let mut w = stream.try_clone()?;
    write_frame(&mut w, &ServerFrame::Exit { message: message.to_string() })
}

// ── TTY hygiene ──────────────────────────────────────────────────────────────

/// Repair a TTY left in raw mode by a killed client (SIGKILL can't restore
/// termios, and the next process inherits the mess — `\n` without `\r`,
/// staircase output, no echo). Idempotent; a no-op when stdout isn't a TTY.
/// Also run before entering the TUI, so crossterm saves a *sane* state to
/// restore on exit instead of faithfully re-breaking the terminal.
pub fn sanitize_tty() {
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

/// On panic, put the terminal back together before the message prints —
/// otherwise the report is unreadable and the shell is left broken.
pub fn install_panic_restore() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use crossterm::{event, execute, terminal};
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            terminal::LeaveAlternateScreen,
            event::DisableMouseCapture,
            event::DisableBracketedPaste,
            crossterm::cursor::Show
        );
        sanitize_tty();
        default(info);
    }));
}

// ── Socket paths ─────────────────────────────────────────────────────────────

fn socket_dir() -> Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("mars-{}", uid));
    std::fs::create_dir_all(&dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

pub fn socket_path(name: &str) -> Result<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return Err(anyhow!("invalid session name: {name:?}"));
    }
    Ok(socket_dir()?.join(format!("{name}.sock")))
}

/// Ask a live session whether a client is currently attached.
fn query_attached(path: &std::path::Path) -> Option<bool> {
    let stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    let mut w = stream.try_clone().ok()?;
    write_frame(&mut w, &ClientFrame::Status).ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    match serde_json::from_str::<ServerFrame>(line.trim()).ok()? {
        ServerFrame::Status { attached, .. } => Some(attached),
        _ => None,
    }
}

/// Lowest free numeric session name (tmux-style: 0, 1, 2, …).
pub fn next_auto_name() -> Result<String> {
    let taken: std::collections::HashSet<String> =
        list_sessions()?.into_iter().map(|(n, _, _)| n).collect();
    Ok((0..)
        .map(|n| n.to_string())
        .find(|n| !taken.contains(n))
        .unwrap_or_else(|| "0".to_string()))
}

/// (name, alive, attached) for every session socket; stale sockets are removed.
pub fn list_sessions() -> Result<Vec<(String, bool, bool)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(socket_dir()?)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        match query_attached(&path) {
            Some(attached) => out.push((name, true, attached)),
            None => {
                let _ = std::fs::remove_file(&path); // dead or unresponsive
                out.push((name, false, false));
            }
        }
    }
    out.sort();
    Ok(out)
}

// ── Server ───────────────────────────────────────────────────────────────────

/// Render sink: buffers ratatui's ANSI writes, ships one Output frame per
/// flush (i.e. per drawn frame). IO errors mark the client dead instead of
/// erroring the draw — the reader thread reports the disconnect.
struct FrameWriter {
    stream: UnixStream,
    buf: Vec<u8>,
    dead: bool,
}

impl FrameWriter {
    fn new(stream: UnixStream) -> Self {
        // Don't let one wedged client stall the whole session forever.
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        FrameWriter { stream, buf: Vec::new(), dead: false }
    }
}

impl Write for FrameWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() || self.dead {
            self.buf.clear();
            return Ok(());
        }
        let frame = ServerFrame::Output { b64: B64.encode(&self.buf) };
        self.buf.clear();
        if write_frame(&mut self.stream, &frame).is_err() {
            self.dead = true;
        }
        Ok(())
    }
}

enum SrvEvent {
    Attach { stream: UnixStream, cols: u16, rows: u16, gen: u64 },
    Input(InputEvent),
    ClientGone(u64),
    /// `mars kill <name>` — force-quit (autosaves first, skips the dirty guard).
    Kill,
    /// `mars rename <old> <new>`.
    Rename(String),
}

fn make_terminal(
    stream: UnixStream,
    cols: u16,
    rows: u16,
) -> Result<Terminal<CrosstermBackend<FrameWriter>>> {
    let backend = CrosstermBackend::new(FrameWriter::new(stream));
    // Fixed viewport: the daemon has no TTY to query for a size.
    let term = Terminal::with_options(
        backend,
        TerminalOptions { viewport: Viewport::Fixed(Rect::new(0, 0, cols, rows)) },
    )?;
    Ok(term)
}

/// The daemon: owns the App, keeps running with or without a client.
/// `name`/`path` are mutable: live rename moves the socket file (the bound
/// listener follows the inode, so clients keep connecting — verified).
pub fn server_main(name: &str, file: Option<String>) -> Result<()> {
    let mut name = name.to_string();
    let mut path = socket_path(&name)?;
    // Clean a stale socket (previous daemon died without unlinking).
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)
        .map_err(|e| anyhow!("cannot create session '{name}': {e} (already running?)"))?;

    let (tx, rx) = mpsc::channel::<SrvEvent>();
    let gen_counter = Arc::new(AtomicU64::new(0));
    // Shared with connection threads so `mars ls` can report attached state.
    let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let tx = tx.clone();
        let gen_counter = gen_counter.clone();
        let attached = attached.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let tx = tx.clone();
                let gc = gen_counter.clone();
                let at = attached.clone();
                std::thread::spawn(move || client_connection(stream, tx, gc, at));
            }
        });
    }

    let had_file = file.is_some();
    let mut app = App::new(file)?;
    app.session_name = Some(name.to_string());
    // A no-file session opens straight into a terminal (multiplexer default).
    if !had_file && std::env::var("MARS_OPEN_TERMINAL").is_ok() {
        app.open_terminal();
    }

    let mut client: Option<(UnixStream, u64)> = None;
    let mut term: Option<Terminal<CrosstermBackend<FrameWriter>>> = None;

    loop {
        if let Some(t) = term.as_mut() {
            if let Err(e) = t.draw(|f| ui::render(f, &mut app)) {
                debug_log(&format!("srv: draw error: {e}"));
            }
        }
        app.tick();

        match rx.recv_timeout(Duration::from_millis(app.tuning.poll_interval_ms)) {
            Ok(SrvEvent::Attach { stream, cols, rows, gen }) => {
                if let Some((old, _)) = client.take() {
                    let _ = send_exit(&old, "detached: another client attached");
                }
                client = Some((stream.try_clone()?, gen));
                term = Some(make_terminal(stream, cols, rows)?);
                attached.store(true, Ordering::SeqCst);
                if let Some(t) = term.as_mut() {
                    if let Err(e) = t.clear() {
                        debug_log(&format!("srv: clear error: {e}"));
                    }
                }
            }
            Ok(SrvEvent::Input(InputEvent::Resize(cols, rows))) => {
                if let Some((s, _)) = client.as_ref() {
                    term = Some(make_terminal(s.try_clone()?, cols, rows)?);
                    if let Some(t) = term.as_mut() {
                        let _ = t.clear();
                    }
                }
            }
            Ok(SrvEvent::Input(ev)) => {
                let _ = app.apply_input(ev);
            }
            Ok(SrvEvent::ClientGone(gen)) => {
                if client.as_ref().map(|(_, g)| *g == gen).unwrap_or(false) {
                    client = None;
                    term = None; // keep running headless
                    attached.store(false, Ordering::SeqCst);
                    app.autosave(); // the window may have been closed for good
                }
            }
            Ok(SrvEvent::Kill) => {
                app.autosave();
                app.should_quit = true; // forced: `mars kill` skips the dirty guard
            }
            Ok(SrvEvent::Rename(to)) => {
                app.rename_session_to = Some(to);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Live rename (from the editor's RenameSession action or `mars rename`).
        if let Some(to) = app.rename_session_to.take() {
            match socket_path(&to) {
                Ok(new_path) if new_path != path => {
                    if new_path.exists() {
                        app.status_msg = Some(format!("session '{to}' already exists"));
                    } else if std::fs::rename(&path, &new_path).is_ok() {
                        path = new_path;
                        name = to.clone();
                        app.session_name = Some(to);
                        app.status_msg = Some(format!("session renamed to '{name}'"));
                    } else {
                        app.status_msg = Some("session rename failed".into());
                    }
                }
                Ok(_) => {}
                Err(e) => app.status_msg = Some(format!("bad session name: {e}")),
            }
        }

        if app.detach_requested {
            app.detach_requested = false;
            if let Some((s, _)) = client.take() {
                let _ = send_exit(&s, &format!("detached — reattach with: mars --resume {name}"));
            }
            term = None;
            attached.store(false, Ordering::SeqCst);
            app.autosave();
        }
        if app.should_quit {
            if let Some((s, _)) = client.take() {
                let _ = send_exit(&s, "session ended");
            }
            break;
        }
    }

    app.save_state_now();
    let _ = std::fs::remove_file(&path);
    Ok(())
}

pub fn debug_log(msg: &str) {
    if let Ok(path) = std::env::var("MARS_DEBUG_LOG").or_else(|_| std::env::var("ARES_DEBUG_LOG")) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let _ = writeln!(f, "[{ms}] {msg}");
        }
    }
}

/// Per-connection thread: handshake, then pump client frames into the server.
fn client_connection(
    stream: UnixStream,
    tx: mpsc::Sender<SrvEvent>,
    gc: Arc<AtomicU64>,
    attached: Arc<std::sync::atomic::AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let Ok(read_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_half);

    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => return, // liveness ping or dead peer — not a real client
        Err(e) => { debug_log(&format!("hello: read err {e}")); return; }
        Ok(_) => {}
    }
    let first = serde_json::from_str::<ClientFrame>(line.trim());
    match &first {
        // One-shot management frames: answer and hang up.
        Ok(ClientFrame::Status) => {
            if let Ok(mut w) = stream.try_clone() {
                let _ = write_frame(&mut w, &ServerFrame::Status {
                    attached: attached.load(Ordering::SeqCst),
                    version: VERSION.to_string(),
                });
            }
            return;
        }
        Ok(ClientFrame::Kill) => {
            let _ = tx.send(SrvEvent::Kill);
            let _ = send_exit(&stream, "killed");
            return;
        }
        Ok(ClientFrame::Rename { to }) => {
            let _ = tx.send(SrvEvent::Rename(to.clone()));
            let _ = send_exit(&stream, &format!("rename to '{to}' requested"));
            return;
        }
        _ => {}
    }
    let Ok(ClientFrame::Hello { cols, rows, version }) = first else {
        debug_log(&format!("hello parse failed on {:?}: {:?}", line.trim(), first.err()));
        return;
    };
    if version != VERSION {
        let _ = send_exit(&stream, &format!("version mismatch: server {VERSION}, client {version} — rebuild/upgrade"));
        return;
    }
    let _ = stream.set_read_timeout(None);

    let gen = gc.fetch_add(1, Ordering::SeqCst) + 1;
    let Ok(attach_stream) = stream.try_clone() else { return };
    if tx.send(SrvEvent::Attach { stream: attach_stream, cols, rows, gen }).is_err() {
        return;
    }

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // client disconnected — normal detach/close
            Err(e) => { debug_log(&format!("conn: read err {e}")); break; }
            Ok(_) => {
                let parsed = serde_json::from_str::<ClientFrame>(line.trim());
                let ev = match &parsed {
                    Ok(ClientFrame::Key(k)) => Some(InputEvent::Key(*k)),
                    Ok(ClientFrame::Mouse(m)) => Some(InputEvent::Mouse(*m)),
                    Ok(ClientFrame::Paste(s)) => Some(InputEvent::Paste(s.clone())),
                    Ok(ClientFrame::Resize { cols, rows }) => Some(InputEvent::Resize(*cols, *rows)),
                    _ => {
                        debug_log(&format!("conn: parse failed on {:?}: {:?}", line.trim(), parsed.err()));
                        None
                    }
                };
                if let Some(ev) = ev {
                    if tx.send(SrvEvent::Input(ev)).is_err() {
                        break; // server loop gone
                    }
                }
            }
        }
    }
    let _ = tx.send(SrvEvent::ClientGone(gen));
}

// ── Client ───────────────────────────────────────────────────────────────────

/// Attach the real TTY to a running session.
pub fn client_main(name: &str) -> Result<()> {
    use crossterm::{
        event::{
            DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
            KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::{
            disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement,
            EnterAlternateScreen, LeaveAlternateScreen,
        },
    };

    let path = socket_path(name)?;
    let stream = UnixStream::connect(&path)
        .map_err(|_| anyhow!("no live session '{name}' — see: mars ls"))?;
    let mut writer = stream.try_clone()?;
    let (cols, rows) = crossterm::terminal::size()?;
    write_frame(
        &mut writer,
        &ClientFrame::Hello { cols, rows, version: VERSION.to_string() },
    )?;

    install_panic_restore();
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    // Server-frame pump: Output → stdout verbatim; Exit → done.
    let (done_tx, done_rx) = mpsc::channel::<String>();
    {
        let read_half = stream.try_clone()?;
        std::thread::spawn(move || {
            let mut reader = BufReader::new(read_half);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        let _ = done_tx.send("connection lost".into());
                        break;
                    }
                    Ok(_) => match serde_json::from_str::<ServerFrame>(line.trim()) {
                        Ok(ServerFrame::Output { b64 }) => {
                            if let Ok(bytes) = B64.decode(b64) {
                                let mut so = io::stdout().lock();
                                let _ = so.write_all(&bytes);
                                let _ = so.flush();
                            }
                        }
                        Ok(ServerFrame::Exit { message }) => {
                            let _ = done_tx.send(message);
                            break;
                        }
                        Ok(ServerFrame::Status { .. }) => {} // not expected mid-attach
                        Err(_) => {}
                    },
                }
            }
        });
    }

    // Input pump: TTY events → frames.
    let exit_msg;
    loop {
        if let Ok(msg) = done_rx.try_recv() {
            exit_msg = msg;
            break;
        }
        if crossterm::event::poll(Duration::from_millis(50))? {
            let frame = match crossterm::event::read()? {
                Event::Key(k) => Some(ClientFrame::Key(k)),
                Event::Mouse(m) => Some(ClientFrame::Mouse(m)),
                Event::Paste(s) => Some(ClientFrame::Paste(s)),
                Event::Resize(c, r) => Some(ClientFrame::Resize { cols: c, rows: r }),
                _ => None,
            };
            if let Some(f) = frame {
                if write_frame(&mut writer, &f).is_err() {
                    exit_msg = "connection lost".into();
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    if enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        crossterm::cursor::Show
    );
    println!("[mars] {exit_msg}");
    Ok(())
}

// ── CLI entries ──────────────────────────────────────────────────────────────

/// `~/.local/state/mars` (or $XDG_STATE_HOME/mars) — daemon logs live here.
fn state_dir() -> Option<PathBuf> {
    let base = std::env::var("XDG_STATE_HOME").map(PathBuf::from).ok().or_else(|| {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".local").join("state"))
    })?;
    let dir = base.join("mars");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// `mars --session <name>`: attach if alive, else spawn the daemon and attach.
pub fn session_main(name: &str, file: Option<String>) -> Result<()> {
    let path = socket_path(name)?;
    if UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path); // stale
        let exe = std::env::current_exe()?;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--server").arg(name);
        if let Some(f) = &file {
            cmd.arg(f);
        }
        // Daemon output goes to a log file — a crashed session must leave a
        // postmortem, not vanish into /dev/null.
        let log = state_dir()
            .map(|d| d.join(format!("{name}.log")))
            .and_then(|p| {
                std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
            });
        cmd.env("RUST_BACKTRACE", "1");
        // A no-file session opens straight into a terminal pane.
        if file.is_none() {
            cmd.env("MARS_OPEN_TERMINAL", "1");
        }
        cmd.stdin(std::process::Stdio::null());
        match log {
            Some(f) => {
                let f2 = f.try_clone().ok();
                cmd.stdout(f);
                match f2 {
                    Some(f2) => { cmd.stderr(f2); }
                    None => { cmd.stderr(std::process::Stdio::null()); }
                }
            }
            None => {
                cmd.stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
            }
        }
        // Fully detach from this TTY so the daemon survives the window.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn()?;
        // Wait for the daemon's socket to come up.
        let mut ok = false;
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(50));
            if UnixStream::connect(&path).is_ok() {
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(anyhow!("session daemon for '{name}' did not start"));
        }
    }
    client_main(name)
}

/// `mars attach [name]` / `--resume`: reattach (most recent if unnamed).
pub fn resume_main(name: Option<String>) -> Result<()> {
    if let Some(n) = name {
        return client_main(&n);
    }
    let alive: Vec<String> = list_sessions()?
        .into_iter()
        .filter(|(_, a, _)| *a)
        .map(|(n, _, _)| n)
        .collect();
    match alive.len() {
        0 => Err(anyhow!("no running sessions — start one with: mars new <name>")),
        1 => client_main(&alive[0]),
        _ => {
            // Most recently touched socket wins.
            let mut best: Option<(std::time::SystemTime, String)> = None;
            for n in &alive {
                let mtime = std::fs::metadata(socket_path(n)?)?.modified()?;
                if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                    best = Some((mtime, n.clone()));
                }
            }
            client_main(&best.unwrap().1)
        }
    }
}

/// `mars ls`
pub fn list_main() -> Result<()> {
    let sessions = list_sessions()?;
    if sessions.is_empty() {
        println!("no sessions — start one with: mars new <name>");
        return Ok(());
    }
    println!("{:<20} {}", "SESSION", "STATUS");
    for (name, alive, attached) in sessions {
        let status = match (alive, attached) {
            (true, true) => "attached".to_string(),
            (true, false) => format!("detached — reattach: mars attach {name}"),
            (false, _) => "dead (cleaned up)".to_string(),
        };
        println!("{name:<20} {status}");
    }
    Ok(())
}

/// `mars rename <old> <new>`: rename a running session from outside.
pub fn rename_main(old: &str, new: &str) -> Result<()> {
    let new_path = socket_path(new)?; // validates the name
    if new_path.exists() {
        return Err(anyhow!("session '{new}' already exists"));
    }
    let old_path = socket_path(old)?;
    let stream = UnixStream::connect(&old_path)
        .map_err(|_| anyhow!("no live session '{old}' — see: mars ls"))?;
    let mut w = stream.try_clone()?;
    write_frame(&mut w, &ClientFrame::Rename { to: new.to_string() })?;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        if new_path.exists() && !old_path.exists() {
            println!("session '{old}' renamed to '{new}'");
            return Ok(());
        }
    }
    Err(anyhow!("rename did not complete — see: mars ls"))
}

/// `mars kill <name>`: terminate a session daemon (autosaves, then exits).
pub fn kill_main(name: &str) -> Result<()> {
    let path = socket_path(name)?;
    let stream = UnixStream::connect(&path)
        .map_err(|_| anyhow!("no live session '{name}' — see: mars ls"))?;
    let mut w = stream.try_clone()?;
    write_frame(&mut w, &ClientFrame::Kill)?;
    // Wait briefly for the socket to disappear (clean shutdown).
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        if !path.exists() {
            println!("session '{name}' ended");
            return Ok(());
        }
    }
    println!("kill sent to '{name}' (still shutting down)");
    Ok(())
}
