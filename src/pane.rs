use crate::buffer::BufferId;

pub type PaneId = usize;
pub type TermId = usize;

#[derive(Debug, Clone)]
pub enum PaneContent {
    Editor(BufferId),
    Terminal(TermId),
}

#[derive(Debug, Clone)]
pub struct Pane {
    /// Legacy field kept for compatibility — prefer `content`.
    pub buffer_id: BufferId,
    pub content: PaneContent,
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// Desired column when navigating up/down — preserves position across short lines.
    pub col_affinity: usize,
    pub scroll_row: usize,
    /// Selection anchor (row, col) — `Some` while a region is active (Shift-move).
    pub selection_anchor: Option<(usize, usize)>,
    /// Viewport height from the last render — used by recenter (C-l).
    pub view_h: usize,
    /// User-set pane title; falls back to the buffer name / "terminal".
    pub title: Option<String>,
    /// Read-only rendered-Markdown reading-mode (reflow/tables via termimad;
    /// document-scrolled, editing disabled).
    pub md_view: bool,
    /// Document-scroll offset (rendered lines) for the Markdown reading-mode.
    pub md_scroll: usize,
    /// Total rendered (reflowed) line count from the last termimad draw — set by
    /// render, read by the scroll handler to clamp exactly and show a position %.
    pub md_rendered_total: std::cell::Cell<usize>,
}

impl Pane {
    pub fn new(buffer_id: BufferId) -> Self {
        Pane {
            buffer_id,
            content: PaneContent::Editor(buffer_id),
            cursor_row: 0,
            cursor_col: 0,
            col_affinity: 0,
            scroll_row: 0,
            selection_anchor: None,
            view_h: 0,
            title: None,
            md_view: false,
            md_scroll: 0,
            md_rendered_total: std::cell::Cell::new(0),
        }
    }

    pub fn ensure_scroll(&mut self, viewport_height: usize, margin: usize) {
        if self.cursor_row < self.scroll_row + margin {
            self.scroll_row = self.cursor_row.saturating_sub(margin);
        } else {
            let bottom = self.scroll_row + viewport_height.saturating_sub(margin + 1);
            if self.cursor_row > bottom {
                self.scroll_row = self.cursor_row + margin + 1 - viewport_height.max(1);
            }
        }
    }
}
