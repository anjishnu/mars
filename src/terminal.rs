/// Terminal panes — a real shell running inside a pane via a PTY.
/// Output is parsed by `vt100` into a screen grid that the UI renders.

use std::io::{Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};

use anyhow::Result;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

pub type TermId = usize;

/// Emitted when terminal `id`'s screen changes or its child process exits.
pub enum TermEvent {
    Output(TermId),
    Exited(TermId),
}

pub struct Term {
    /// The shell has exited; the pane shows a notice until the user closes it.
    pub exited: bool,
    /// Where the shell was spawned (the work journal's cwd). The shell may
    /// `cd` later — a PTY can't see that without shell integration, so this
    /// is honest spawn-time truth, not a live value.
    pub spawn_cwd: Option<std::path::PathBuf>,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    exit_code: Arc<Mutex<Option<i32>>>,
    notify_exit: Arc<AtomicBool>,
    rows: u16,
    cols: u16,
    /// How far back the view is scrolled (0 = live). Mirrors the vt100 state.
    view_offset: usize,
    scrollback_limit: usize,
}

/// Spawn the platform shell on a PTY sized `rows` x `cols` with `scrollback` lines of
/// history, streaming output into a `vt100::Parser`. One background thread
/// pumps the PTY; another waits for child-process exit.
pub fn spawn(
    id: TermId,
    rows: u16,
    cols: u16,
    scrollback: usize,
    cwd: Option<std::path::PathBuf>,
    session: Option<&str>,
    tx: mpsc::Sender<TermEvent>,
) -> Result<Term> {
    let rows = rows.max(1);
    let cols = cols.max(1);

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let shell = crate::sys::shell::default_shell();
    let mut cmd = CommandBuilder::new(shell);
    let spawn_cwd = cwd.filter(|d| d.is_dir());
    if let Some(dir) = &spawn_cwd {
        cmd.cwd(dir);
    }
    // Mark the shell as living inside this Mars session, so a nested `mars <file>`
    // opens a tab in the running instance instead of launching a new one.
    if let Some(name) = session {
        cmd.env("MARS_SESSION", name);
    }
    let mut child = pair.slave.spawn_command(cmd)?;
    let killer = child.clone_killer();
    // Drop our slave copy; child-process exit is tracked separately because
    // ConPTY can keep the master output pipe open after the child is gone.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, scrollback)));
    let reader_parser = parser.clone();
    let output_tx = tx.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut p) = reader_parser.lock() {
                        p.process(&buf[..n]);
                    }
                    if output_tx.send(TermEvent::Output(id)).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let exit_code = Arc::new(Mutex::new(None));
    let wait_exit_code = exit_code.clone();
    let notify_exit = Arc::new(AtomicBool::new(true));
    let wait_notify_exit = notify_exit.clone();
    std::thread::spawn(move || {
        let code = child.wait().ok().map(|status| status.exit_code() as i32);
        if let Ok(mut slot) = wait_exit_code.lock() {
            *slot = code;
        }
        if wait_notify_exit.swap(false, Ordering::AcqRel) {
            let _ = tx.send(TermEvent::Exited(id));
        }
    });

    Ok(Term {
        exited: false,
        spawn_cwd,
        parser,
        writer,
        master: pair.master,
        killer,
        exit_code,
        notify_exit,
        rows,
        cols,
        view_offset: 0,
        scrollback_limit: scrollback,
    })
}

/// Removing a Term (closed pane/tab, app exit) must not orphan the shell: kill
/// the child process tree with the pane, never leave it running invisibly.
impl Drop for Term {
    fn drop(&mut self) {
        self.notify_exit.store(false, Ordering::Release);
        let _ = self.killer.kill();
    }
}

impl Term {
    pub fn send_bytes(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// The shell's exit code, if it has exited and the OS reported one —
    /// available once the process watcher has reported exit.
    pub fn exit_code(&mut self) -> Option<i32> {
        self.exit_code.lock().ok().and_then(|code| *code)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            p.set_size(rows, cols);
        }
    }

    /// Clone of the latest screen for rendering.
    pub fn screen(&self) -> vt100::Screen {
        self.parser.lock().unwrap().screen().clone()
    }

    /// Scroll the view within the scrollback: positive = further back in
    /// history, negative = toward live. Clamped to [0, real history depth].
    pub fn scroll_view(&mut self, delta: i64) {
        let requested = (self.view_offset as i64 + delta)
            .clamp(0, self.scrollback_limit as i64) as usize;
        if let Ok(mut p) = self.parser.lock() {
            p.set_scrollback(requested);
            // vt100 clamps to the ACTUAL available history; mirror that real
            // value so the "↑N" indicator is honest and a wheel-down doesn't
            // have to burn through a phantom offset past the top of history.
            self.view_offset = p.screen().scrollback();
        } else {
            self.view_offset = requested;
        }
    }

    /// The last `lines` lines of scrollback + live screen, oldest-first (W5).
    /// Pages back through history under the lock and restores the live view.
    pub fn history_tail(&self, lines: usize) -> String {
        let Ok(mut p) = self.parser.lock() else { return String::new() };
        let rows = p.screen().size().0 as usize;
        let saved = self.view_offset;
        let mut pages: Vec<String> = Vec::new();
        let (mut off, mut got) = (0usize, 0usize);
        loop {
            p.set_scrollback(off);
            pages.push(p.screen().contents());
            got += rows;
            if got >= lines || off >= self.scrollback_limit {
                break;
            }
            off += rows;
        }
        p.set_scrollback(saved); // restore the live view before releasing the lock
        pages.reverse(); // oldest screenful first
        let joined = pages.join("\n");
        let all: Vec<&str> = joined.lines().collect();
        let start = all.len().saturating_sub(lines);
        all[start..].join("\n")
    }

    /// Snap back to the live screen (any keystroke does this).
    pub fn scroll_to_live(&mut self) {
        if self.view_offset != 0 {
            self.scroll_view(-(self.view_offset as i64));
        }
    }

    pub fn view_offset(&self) -> usize {
        self.view_offset
    }
}
