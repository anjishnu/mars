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
    /// Open a file as a new tab in the running session (used by a nested
    /// `mars <file>` run from a terminal pane inside this session).
    Open { path: String },
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
    /// A nested `mars <file>` — open it as a new tab here.
    OpenFile(String),
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
        app.tick();
        // Draw only when visible state moved — the frames go to the client over
        // the socket (and thus over SSH), so an idle no-op draw is a wasted packet
        // that contends with the user's own keystrokes.
        if std::mem::take(&mut app.needs_redraw) {
            if let Some(t) = term.as_mut() {
                if let Err(e) = t.draw(|f| ui::render(f, &mut app)) {
                    debug_log(&format!("srv: draw error: {e}"));
                }
                // A copy queues an OSC 52 escape: append it raw after the
                // frame so it reaches the client's real terminal (and through
                // ssh, the clipboard of the machine the user is sitting at).
                if let Some(osc) = app.take_osc() {
                    let w = t.backend_mut(); // CrosstermBackend forwards Write to the FrameWriter
                    let _ = w.write_all(osc.as_bytes());
                    let _ = w.flush();
                }
            }
        }

        match rx.recv_timeout(Duration::from_millis(app.tuning.poll_interval_ms)) {
            Ok(SrvEvent::Attach { stream, cols, rows, gen }) => {
                if let Some((old, _)) = client.take() {
                    let _ = send_exit(&old, "detached: another client attached");
                }
                client = Some((stream.try_clone()?, gen));
                term = Some(make_terminal(stream, cols, rows)?);
                attached.store(true, Ordering::SeqCst);
                app.needs_redraw = true; // fresh client → full repaint
                app.on_attach(); // W7: "where was I?" briefing from the detach diff
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
                    app.needs_redraw = true;
                }
            }
            Ok(SrvEvent::Input(ev)) => {
                let _ = app.apply_input(ev);
                app.needs_redraw = true; // input → repaint
            }
            Ok(SrvEvent::ClientGone(gen)) => {
                if client.as_ref().map(|(_, g)| *g == gen).unwrap_or(false) {
                    client = None;
                    term = None; // keep running headless
                    attached.store(false, Ordering::SeqCst);
                    app.on_detach(); // W7: snapshot for the reattach briefing
                    app.autosave(); // the window may have been closed for good
                }
            }
            Ok(SrvEvent::Kill) => {
                app.autosave();
                app.should_quit = true; // forced: `mars kill` skips the dirty guard
            }
            Ok(SrvEvent::OpenFile(path)) => {
                app.open_file_in_new_tab(&path);
                app.needs_redraw = true;
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
            // Snapshot for the reattach shift report BEFORE dropping the client —
            // the intended "quit = detach" path (C-x C-c) must arm the save-state
            // restore exactly like an accidental disconnect (ClientGone) does.
            app.on_detach();
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
        Ok(ClientFrame::Open { path }) => {
            let _ = tx.send(SrvEvent::OpenFile(path.clone()));
            let _ = send_exit(&stream, &format!("opening '{path}'"));
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

/// One row of `mars ls`: local daemon sessions and remote fleet hosts behind a
/// single shape, so rendering, ordinals, and the follow-up resolver are one
/// code path and the freshest known status flows through the same field for
/// both — a live probe for locals, the broker status push for remotes.
pub struct SessionEntry {
    /// Session name (local) or host name (remote).
    pub name: String,
    pub remote: bool,
    pub status: String,
    /// LLM-derived gloss of what the session is FOR (inferred mission, else the
    /// last work-journal verdict) — kept apart from `status` so liveness stays
    /// scannable and the prose gets its own column at the end of the table.
    pub summary: String,
    /// When the status was observed: `None` = right now (live local probe);
    /// `Some(ts)` = the last time the remote self-reported.
    pub as_of: Option<u64>,
    /// The command that gets you there (`mars attach x` / `mars ssh h`).
    pub connect: String,
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Lifecycle noise that says nothing about the work: an interactive shell being
/// closed, a bare exit. These flooded the summary with "user quit" before the
/// auto-watch noise gate; filter them here too so the journal's legacy lines
/// (and any manual-watch lifecycle verdicts) never become the headline.
fn is_lifecycle_noise(verdict: &str) -> bool {
    let l = verdict.to_lowercase();
    [
        "user exited", "user quit", "shell exited", "shell closed", "user left",
        "terminal session closed", "exit command", "idle at prompt",
        "exited voluntarily", "exited terminal",
    ]
    .iter()
    .any(|m| l.contains(m))
}

/// What the session is FOR / what it needs — the useful glance, not a vague
/// distillation. Priority, all from cheap on-disk signals: (1) a failure or
/// block that needs you, (2) the goals captured at detach — the concrete
/// intent, (3) a *recent* inferred mission (stale ones are dropped, not shown),
/// (4) the freshest real thing that happened. Lifecycle noise never wins.
pub fn session_summary(name: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let recent = crate::worklog::recent(name, 12);
    let meaningful = recent.iter().rev().find(|e| !is_lifecycle_noise(&e.verdict));
    // 1. A failure/block that needs you leads — the reason you'd scan the list.
    if let Some(e) = meaningful {
        let low = e.verdict.to_lowercase();
        if e.failed || low.starts_with("blocked") || low.contains("failed") {
            return format!("{} · {}", clip(&e.verdict, 88), crate::broker::ago(e.ts));
        }
    }
    // 2. The goals captured at detach — the clearest "what is this session for."
    let goals = crate::worklog::load_goals(name);
    if let Some(first) = goals.first() {
        let head = clip(first, 52);
        return if goals.len() > 1 {
            format!("→ {head}  (+{} more)", goals.len() - 1)
        } else {
            format!("→ {head}")
        };
    }
    // 3. A *recent* inferred mission — age-gated so a days-old vague line doesn't
    //    masquerade as current state (the "basically useless" complaint).
    if let Some((mission, as_of)) = crate::worklog::load_mission(name) {
        if now.saturating_sub(as_of) < 3 * 86_400 {
            return clip(&mission, 160);
        }
    }
    // 4. The freshest real event (a completed run, etc.).
    if let Some(e) = meaningful {
        return format!("{} · {}", clip(&e.verdict, 88), crate::broker::ago(e.ts));
    }
    String::new()
}

/// Greedy word-wrap to `width` columns; words longer than a line are
/// hard-split rather than overflowing. Empty input → no lines.
pub fn wrap_text(s: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut len = 0;
    for word in s.split_whitespace() {
        let chars: Vec<char> = word.chars().collect();
        for piece in chars.chunks(width) {
            if len > 0 && len + 1 + piece.len() > width {
                lines.push(std::mem::take(&mut cur));
                len = 0;
            }
            if len > 0 {
                cur.push(' ');
                len += 1;
            }
            cur.extend(piece);
            len += piece.len();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Everything `mars ls` knows about, locals first. The single access path for
/// both kinds — callers never touch `list_sessions`/`fleet_load` shapes.
pub fn all_sessions() -> Result<Vec<SessionEntry>> {
    let mut out = Vec::new();
    for (name, alive, attached) in list_sessions()? {
        let status = match (alive, attached) {
            (true, true) => "attached",
            (true, false) => "detached",
            (false, _) => "dead (cleaned up)",
        }
        .to_string();
        let summary = if alive { session_summary(&name) } else { String::new() };
        out.push(SessionEntry {
            connect: format!("mars attach {name}"),
            name,
            remote: false,
            status,
            summary,
            as_of: None,
        });
    }
    for e in crate::broker::fleet_load() {
        let mut status = e.last_status.clone().unwrap_or_else(|| "seen".to_string());
        if let Some(s) = &e.session {
            status = format!("{status} · session {s}");
        }
        out.push(SessionEntry {
            connect: format!("mars ssh {}", e.host),
            name: e.host,
            remote: true,
            status,
            summary: String::new(),
            as_of: Some(e.as_of),
        });
    }
    Ok(out)
}

/// `mars ls` — one numbered table over local and remote alike; the follow-up
/// prompt resolves an ordinal/name to `attach` or `ssh` through the same list.
pub fn list_main(prompt: bool) -> Result<()> {
    let entries = all_sessions()?;
    if entries.is_empty() {
        println!("no sessions — start one with: mars new <name>, or reach a box with: mars ssh <host>");
        return Ok(());
    }
    println!(
        "  #  {:<20} {:<7} {:<28} {:<9} {}",
        "SESSION", "WHERE", "STATUS", "AS OF", "SUMMARY"
    );
    // A long summary wraps into a block justified under the SUMMARY column
    // (continuation lines indented to this row's summary start) instead of
    // spilling into an unreadable overlong line.
    let cols = crossterm::terminal::size().map(|(w, _)| w as usize).unwrap_or(100);
    for (i, e) in entries.iter().enumerate() {
        let seen = match e.as_of {
            None => "now".to_string(),
            Some(t) => crate::broker::ago(t),
        };
        let prefix = format!(
            "  {:<2} {:<20} {:<7} {:<28} {:<9} ",
            i + 1,
            e.name,
            if e.remote { "remote" } else { "local" },
            e.status,
            seen
        );
        let indent = prefix.chars().count();
        let mut lines = wrap_text(&e.summary, cols.saturating_sub(indent).max(20)).into_iter();
        match lines.next() {
            None => println!("{}", prefix.trim_end()),
            Some(first) => {
                println!("{prefix}{first}");
                for l in lines {
                    println!("{}{l}", " ".repeat(indent));
                }
            }
        }
    }

    // Interactive follow-up: an ordinal or (prefix of a) name attaches a local
    // session or sshes to a remote host — same resolver over the same list.
    // Skipped by --no-prompt or when stdin isn't a TTY (scripts).
    let is_tty = unsafe { libc::isatty(libc::STDIN_FILENO) == 1 };
    if prompt && is_tty {
        use std::io::Write;
        print!("\n→ open (number/name, Enter to skip): ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_ok() {
            let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
            if let Some(name) = crate::broker::resolve_target(&names, &line) {
                let e = entries.iter().find(|e| e.name == name).unwrap();
                return if e.remote {
                    crate::broker::ssh_main(e.name.clone(), Vec::new())
                } else {
                    client_main(&e.name)
                };
            }
        }
    }
    Ok(())
}

/// Open a file as a new tab in a running session (nested `mars <file>`).
/// Relative paths resolve against the caller's cwd (the shell's), so the file
/// opens correctly even though the daemon has a different working directory.
pub fn open_in_session(name: &str, path: &str) -> Result<()> {
    let sock = socket_path(name)?;
    let stream = UnixStream::connect(&sock)
        .map_err(|_| anyhow!("session '{name}' is not running"))?;
    let p = std::path::Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let mut w = stream.try_clone()?;
    write_frame(&mut w, &ClientFrame::Open { path: abs.to_string_lossy().to_string() })?;
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

/// `mars killall`: the reset button. End EVERY live session daemon (each
/// autosaves first), and with `force` (the CLI path) also put down anything
/// that didn't answer its socket, shut down lingering ssh ControlMasters and
/// the key broker, and sweep the stale sockets they leave behind. Agentic
/// memory (cmd_memory, worklog, mission, denylist, fleet) is untouched, and
/// no new session is started. `force: false` is for the selfcheck, whose
/// TMPDIR isolation a process-wide pkill would not respect.
pub fn killall_main(force: bool) -> Result<()> {
    let mut ended = 0;
    for (name, alive, _) in list_sessions()? {
        if alive {
            let _ = kill_main(&name); // graceful: autosave, then exit
            ended += 1;
        }
    }
    if !force {
        if ended == 0 {
            println!("no live sessions to kill");
        }
        return Ok(());
    }
    // Anything still standing didn't answer its socket — put it down hard.
    for pat in ["mars --server", "mars keyd"] {
        let _ = std::process::Command::new("pkill")
            .arg("-f").arg(pat)
            .status();
    }
    // Shut down ssh ControlMasters cleanly, then sweep their socket files —
    // a leftover master ambushes the next `mars ssh` with a broken pipe.
    if let Some(dir) = crate::broker::broker_socket_path().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if !e.file_name().to_string_lossy().starts_with("cm-") {
                    continue;
                }
                let _ = std::process::Command::new("ssh")
                    .arg("-O").arg("exit")
                    .arg("-o").arg(format!("ControlPath={}", e.path().display()))
                    .arg("killall-sweep")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                let _ = std::fs::remove_file(e.path());
            }
        }
        let _ = std::fs::remove_file(dir.join("auth.sock")); // keyd is down
    }
    // Dead forwarded sockets in /tmp (this box may itself be someone's remote).
    let _ = crate::broker::find_live_auth_sock(std::path::Path::new("/tmp")); // probe = sweep dead ones
    // Leftover session sockets of force-killed daemons.
    if let Ok(entries) = std::fs::read_dir(socket_dir()?) {
        for e in entries.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("sock") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    println!(
        "killall: {ended} session(s) ended gracefully; force-swept daemons, \
         ssh masters, and stale sockets. Memory files untouched."
    );
    Ok(())
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
