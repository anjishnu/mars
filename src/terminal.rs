/// Terminal panes — a real shell running inside a pane via a PTY.
/// Output is parsed by `vt100` into a screen grid that the UI renders.

use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};

use anyhow::Result;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

pub type TermId = usize;

/// Emitted by the reader thread whenever terminal `id`'s screen changes,
/// so the main loop knows to repaint — or when the shell exits.
pub enum TermEvent {
    Output(TermId),
    Exited(TermId),
}

pub struct Term {
    /// The shell has exited; the pane shows a notice until the user closes it.
    pub exited: bool,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    _child: Box<dyn Child + Send + Sync>,
    rows: u16,
    cols: u16,
    /// How far back the view is scrolled (0 = live). Mirrors the vt100 state.
    view_offset: usize,
    scrollback_limit: usize,
}

/// Spawn `$SHELL` on a PTY sized `rows` x `cols` with `scrollback` lines of
/// history, streaming output into a `vt100::Parser`. A background thread
/// pumps the PTY and signals `tx`.
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

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = CommandBuilder::new(shell);
    if let Some(dir) = cwd.filter(|d| d.is_dir()) {
        cmd.cwd(dir);
    }
    // Mark the shell as living inside this Mars session, so a nested `mars <file>`
    // opens a tab in the running instance instead of launching a new one.
    if let Some(name) = session {
        cmd.env("MARS_SESSION", name);
    }
    let child = pair.slave.spawn_command(cmd)?;
    // Drop the slave so the master reader sees EOF when the shell exits.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, scrollback)));
    let reader_parser = parser.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut p) = reader_parser.lock() {
                        p.process(&buf[..n]);
                    }
                    if tx.send(TermEvent::Output(id)).is_err() {
                        break;
                    }
                }
            }
        }
        // EOF: the shell is gone — tell the app so the pane can say so.
        let _ = tx.send(TermEvent::Exited(id));
    });

    Ok(Term {
        exited: false,
        parser,
        writer,
        master: pair.master,
        _child: child,
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
        let _ = self._child.kill();
    }
}

impl Term {
    pub fn send_bytes(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
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
    /// history, negative = toward live. Clamped to [0, scrollback_limit].
    pub fn scroll_view(&mut self, delta: i64) {
        let next = (self.view_offset as i64 + delta)
            .clamp(0, self.scrollback_limit as i64) as usize;
        self.view_offset = next;
        if let Ok(mut p) = self.parser.lock() {
            p.set_scrollback(next);
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
