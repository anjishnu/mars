use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::{
    app::App,
    layout::PaneLayout,
    mode::Mode,
    palette::{Action, BarMode, ItemKind},
    pane::{PaneContent, PaneId},
};

/// Width of the "NNNN│ " line-number prefix when `line_numbers` is on.
pub const LINE_NUM_W: u16 = 6;
/// Width of the default gutter: a 1-char cursor-line pointer + 1 space.
pub const POINTER_W: u16 = 2;

/// Default is a slim pointer gutter (current-line marker only); the full
/// line-number gutter is opt-in (`line_numbers` knob). Either way the live
/// line/col lives in the status bar.
pub fn gutter_width(tuning: &crate::tuning::Tuning) -> u16 {
    if tuning.line_numbers { LINE_NUM_W } else { POINTER_W }
}

/// Tuning stores colors as [r, g, b] so they stay agent-editable JSON.
fn rgb(c: [u8; 3]) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

// ── Entry point ──────────────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Layout: tab-bar (1) | pane area (min) | status (1) | control bar (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(1),    // pane area
            Constraint::Length(1), // status bar
            Constraint::Length(1), // control bar
        ])
        .split(area);

    let (tab_area, full_pane_area, status_area, bar_area) =
        (chunks[0], chunks[1], chunks[2], chunks[3]);

    // Carve a left sidebar for the file tree when it's open; panes take the rest.
    let pane_area = if app.tree_open {
        let tw = app.tuning.tree_width.min(full_pane_area.width.saturating_sub(20));
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(tw), Constraint::Min(1)])
            .split(full_pane_area);
        render_file_tree(frame, app, cols[0]);
        cols[1]
    } else {
        full_pane_area
    };

    render_tab_bar(frame, app, tab_area);
    render_panes(frame, app, pane_area);
    // Session-start splash: the MARS banner overlays the workspace (terminal or
    // editor) until the first keypress dismisses it.
    if app.show_splash {
        render_splash(frame, app, pane_area);
    }
    // The shift report: the save-state restore overlays the workspace on
    // reattach; any key resumes. Suppresses notice noise while up.
    if app.shift_report.is_some() {
        render_shift_report(frame, app, pane_area);
    }
    // Proactive notice (W6): one dim line at the bottom of the workspace, the
    // agent's only path to the screen. Failures first; Esc dismisses.
    if !app.notices.is_empty() && !app.show_splash && app.shift_report.is_none() {
        render_notice(frame, app, pane_area);
    }
    render_status(frame, app, status_area);
    render_control_bar(frame, app, bar_area);

    // Bar dropdown / ask-panel drawn last so it sits on top (grows upward).
    if app.palette.is_some() && matches!(app.mode, Mode::Bar) {
        match app.palette.as_ref().map(|p| p.bar_mode.clone()) {
            Some(BarMode::Ask)     => render_ask_panel(frame, app, pane_area, bar_area),
            Some(BarMode::Command) => {
                let dropdown = render_bar_dropdown(frame, app, pane_area, bar_area);
                // In a terminal, the unified composer also shows the red inline
                // overlay at the cursor (type-in-place) — but the menu outranks
                // it: when the two would collide, the overlay stays hidden.
                if app.bar_return == Mode::Terminal {
                    render_shell_overlay(frame, app, pane_area, dropdown);
                }
            }
            // Shell: an inline composer anchored at the cursor (no eye-jump).
            Some(BarMode::Shell)   => render_shell_overlay(frame, app, pane_area, None),
            None => {}
        }
    }

    // which-key: after a short hesitation on a prefix, show the continuations.
    if !app.pending_prefix.is_empty()
        && app.frame_tick.saturating_sub(app.prefix_tick) >= app.tuning.which_key_delay_ticks()
    {
        render_which_key(frame, app, pane_area, status_area);
    }

    // C-t travel mode: always-on cheat panel — the characters tell you what to do.
    if matches!(app.mode, Mode::Tab) {
        render_travel_panel(frame, app, pane_area, status_area);
    }
}

// ── C-t travel panel ─────────────────────────────────────────────────────────

fn render_travel_panel(frame: &mut Frame, app: &App, pane_area: Rect, status_area: Rect) {
    let panel_width = app.tuning.travel_panel_width;
    let rows: &[(&str, &str)] = &[
        ("← → ↑ ↓",  "move focus  ·  pane → tab at the edges"),
        ("1-9",      "jump to tab"),
        ("z / Spc",  "zoom (maximize)  ·  toggle"),
        ("d / ⌫",    "close focused  ·  pane, or tab if last"),
        ("",         ""),
        ("t / n",    "new tab"),
        ("T",        "new terminal tab"),
        ("| / -",    "split right / below"),
        ("⇧ ← →",    "reorder tab"),
        ("< >",      "resize pane"),
        ("x",        "swap pane"),
        ("r",        "rename tab"),
        ("",         ""),
        ("?",        "why did this fail? (triage)"),
        ("w",        "watch this pane (summarize when done)"),
        ("D",        "detach session (keeps running)"),
        ("Esc ⏎",    "done  ·  creation exits, navigation stays"),
    ];

    let mut lines: Vec<Line> = Vec::new();
    for (keys, what) in rows {
        if keys.is_empty() {
            lines.push(Line::from(Span::raw("")));
            continue;
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:<9}", keys),
                Style::default().fg(rgb(app.tuning.theme_accent_bright)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}", what), Style::default().fg(Color::White)),
        ]));
    }

    let panel_h = (lines.len() as u16 + 1).min(pane_area.height.saturating_sub(1));
    let width = panel_width.min(status_area.width);
    let rect = Rect {
        x: status_area.x + status_area.width.saturating_sub(width),
        y: status_area.y.saturating_sub(panel_h),
        width,
        height: panel_h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .title(Span::styled(
            " C-t · space warp ",
            Style::default().fg(rgb(app.tuning.theme_accent)).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::TOP | Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

// ── which-key panel ──────────────────────────────────────────────────────────

fn render_which_key(frame: &mut Frame, app: &App, pane_area: Rect, status_area: Rect) {
    let conts = app.keys.continuations(&app.pending_prefix);
    if conts.is_empty() {
        return;
    }
    let prefix = crate::config::render_chords(&app.pending_prefix);

    let mut lines: Vec<Line> = Vec::new();
    for (tail, action) in &conts {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:<4}", tail),
                Style::default()
                    .fg(rgb(app.tuning.theme_accent_bright))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", action.label()),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    let panel_h = (lines.len() as u16 + 1).min(pane_area.height.saturating_sub(1)); // +1 border
    let width = app.tuning.which_key_panel_width.min(status_area.width);
    let rect = Rect {
        x: status_area.x + status_area.width.saturating_sub(width),
        y: status_area.y.saturating_sub(panel_h),
        width,
        height: panel_h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .title(Span::styled(
            format!(" {} - ", prefix),
            Style::default()
                .fg(rgb(app.tuning.theme_accent_bright))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::TOP | Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

// ── Tab bar ──────────────────────────────────────────────────────────────────

/// The one place a surface verdict becomes a glyph + semantic color. Tab labels,
/// pane borders, and the workspace summary all read this, so a status means the
/// same thing everywhere it shows. Returns None for idle (Context) — each caller
/// picks its own recede (a readable label, a dim border, or nothing at all).
fn verdict_style(app: &App, v: crate::briefing::Verdict) -> Option<(&'static str, Color)> {
    use crate::briefing::Verdict;
    // Conventional semantic palette (obvious at a glance): amber=blocked/waiting,
    // red=failed, green=running, teal=done, gray=idle. Colorblind-safe because the
    // glyph disambiguates the hue.
    Some(match v {
        Verdict::Blocked => ("⏸", rgb(app.tuning.theme_status_blocked)), // needs you (amber)
        Verdict::Failed  => ("✗", rgb(app.tuning.theme_status_failed)),  // failed (red)
        Verdict::Running => ("●", rgb(app.tuning.theme_healthy)),        // working (green)
        Verdict::Done    => ("✓", rgb(app.tuning.theme_terminal)),       // done (teal)
        Verdict::Context => return None,                                // idle (gray)
    })
}

/// A pane's display name for the top bar and the board: an editor's filename (with a
/// dirty dot), or a terminal's title / running command / "shell" — never a bare
/// "terminal". This is what gives editor panes and split terminals a real identity.
pub(crate) fn pane_name(app: &App, pane_id: PaneId) -> String {
    let Some(pane) = app.panes.get(&pane_id) else { return "—".to_string() };
    match pane.content {
        PaneContent::Editor(buf_id) => {
            let b = app.buffers.get(&buf_id);
            let name = b.map(|b| b.name.clone()).unwrap_or_else(|| "buffer".to_string());
            if b.map(|b| b.modified).unwrap_or(false) { format!("{name} ●") } else { name }
        }
        PaneContent::Terminal(tid) => {
            if let Some(t) = pane.title.as_deref() {
                return t.to_string();
            }
            if let Some(cmd) = app.watches.get(&tid).and_then(|w| w.last_command.as_ref()) {
                if let Some(w0) = cmd.split_whitespace().next() {
                    return w0.rsplit('/').next().unwrap_or(w0).to_string();
                }
            }
            "shell".to_string()
        }
    }
}

/// The informative name for a workspace (tab): the tab's own name, unless that's
/// empty or just its number — then fall back to the focused pane's name, so the tab
/// bar and the board read "trainer" / "main.rs" / "shell", never a bare "1".
pub(crate) fn workspace_name(app: &App, tab: &crate::tab::Tab) -> String {
    if tab.name.is_empty() || tab.name.parse::<usize>().is_ok() {
        pane_name(app, tab.focused_pane)
    } else {
        tab.name.clone()
    }
}

fn render_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    for (i, tab) in app.tabs.iter().enumerate() {
        // The tab's worst-pane verdict colors the whole label (one pop-out dimension,
        // no dot to hunt) with a colorblind-safe glyph; idle stays a readable default.
        let (glyph, status_color) = match verdict_style(app, app.tab_status(tab)) {
            Some((g, c)) => (format!("{g} "), c),
            None => (String::new(), rgb(app.tuning.theme_accent_bright)),
        };
        let name = workspace_name(app, tab);
        let label = format!(" {glyph}{name} ");
        if i == app.active_tab {
            // The active tab keeps the chrome highlight (you're looking at it); the
            // glyph still hints its worst-pane status.
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(rgb(app.tuning.theme_chip_fg))
                    .bg(rgb(app.tuning.theme_accent))
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, Style::default().fg(status_color)));
        }
        spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    // (The top-right status counter/beacon was removed: it counted finished-Done
    // surfaces that never clear, so it silted up into a persistent, glyph-garbled
    // "✓N ●N" in the corner. Per-tab colors carry status until a better aggregate is
    // designed. Status lives in the tab labels and the WORKSPACES panel.)
}

// ── Pane layout ───────────────────────────────────────────────────────────────

fn compute_rects(layout: &PaneLayout, area: Rect) -> Vec<(PaneId, Rect)> {
    match layout {
        PaneLayout::Single(id) => vec![(*id, area)],
        PaneLayout::HSplit { top, bottom, ratio } => {
            let halves = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(*ratio), Constraint::Percentage(100 - *ratio)])
                .split(area);
            let mut v = compute_rects(top, halves[0]);
            v.extend(compute_rects(bottom, halves[1]));
            v
        }
        PaneLayout::VSplit { left, right, ratio } => {
            let halves = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(*ratio), Constraint::Percentage(100 - *ratio)])
                .split(area);
            let mut v = compute_rects(left, halves[0]);
            v.extend(compute_rects(right, halves[1]));
            v
        }
    }
}

fn render_panes(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused_id = app.focused_pane_id();

    // Zoom follows focus: moving focus away (or closing the pane) unzooms.
    {
        let tab = &mut app.tabs[app.active_tab];
        let stale = match tab.zoomed {
            Some(z) => z != focused_id || !tab.layout.pane_ids().contains(&z),
            None => false,
        };
        if stale {
            tab.zoomed = None;
        }
    }
    let rects: Vec<(PaneId, Rect)> = {
        let tab = &app.tabs[app.active_tab];
        match tab.zoomed {
            Some(z) => vec![(z, area)],
            None => compute_rects(&tab.layout, area),
        }
    };

    // Remember pane rects for mouse hit-testing.
    app.pane_rects = rects.clone();

    // Update scroll offsets now that we know the real viewport heights.
    let margin = app.tuning.scroll_margin;
    for (pane_id, rect) in &rects {
        let inner_h = rect.height.saturating_sub(2) as usize;
        if let Some(p) = app.panes.get_mut(pane_id) {
            p.view_h = inner_h;
            p.ensure_scroll(inner_h, margin);
        }
    }

    // Keep terminal PTYs sized to their panes' inner area.
    for (pane_id, rect) in &rects {
        let tid = match app.panes.get(pane_id).map(|p| p.content.clone()) {
            Some(PaneContent::Terminal(id)) => Some(id),
            _ => None,
        };
        if let Some(tid) = tid {
            let iw = rect.width.saturating_sub(2);
            let ih = rect.height.saturating_sub(2);
            if let Some(t) = app.terms.get_mut(&tid) {
                t.resize(ih, iw);
            }
        }
    }

    let bar_open = app.palette.is_some() && matches!(app.mode, Mode::Bar);
    let mut cursor_screen: Option<(u16, u16)> = None;
    for (pane_id, rect) in &rects {
        let is_focused = *pane_id == focused_id;
        if let Some(pos) = render_pane(frame, app, *pane_id, *rect, is_focused) {
            cursor_screen = Some(pos);
        }
    }
    app.cursor_screen = cursor_screen; // anchors the shell-translate overlay

    // Place the terminal cursor — but not while the bar owns it.
    if !bar_open {
        if let Some((cx, cy)) = cursor_screen {
            if matches!(app.mode, Mode::Edit | Mode::Terminal) {
                frame.set_cursor_position((cx, cy));
            }
        }
    }
}

fn render_pane(
    frame: &mut Frame,
    app: &App,
    pane_id: PaneId,
    rect: Rect,
    focused: bool,
) -> Option<(u16, u16)> {
    let pane = app.panes.get(&pane_id)?;

    match pane.content.clone() {
        PaneContent::Editor(buf_id) => render_editor_pane(frame, app, pane_id, buf_id, rect, focused),
        PaneContent::Terminal(_term_id) => render_terminal_pane(frame, app, pane_id, rect, focused),
    }
}

fn render_editor_pane(
    frame: &mut Frame,
    app: &App,
    pane_id: PaneId,
    buf_id: usize,
    rect: Rect,
    focused: bool,
) -> Option<(u16, u16)> {
    let pane = app.panes.get(&pane_id)?;
    let buf  = app.buffers.get(&buf_id)?;

    let border_style = if focused {
        Style::default().fg(rgb(app.tuning.theme_accent))
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let title_mod = if focused { Modifier::BOLD } else { Modifier::empty() };
    let marker    = if buf.modified { " ●" } else { "" };
    let shown     = pane.title.as_deref().unwrap_or(&buf.name);
    let title     = format!(" {}{} ", shown, marker);

    let block = Block::default()
        .title(Span::styled(title, Style::default().add_modifier(title_mod)))
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let vp_h = inner.height as usize;
    let line_count = buf.line_count();
    let mut lines: Vec<Line> = Vec::with_capacity(vp_h);

    // Ordered selection range (start ≤ end) for highlighting.
    let sel: Option<((usize, usize), (usize, usize))> = pane.selection_anchor.map(|a| {
        let c = (pane.cursor_row, pane.cursor_col);
        if a <= c { (a, c) } else { (c, a) }
    });
    let [sr, sg, sb] = app.tuning.selection_bg;
    let [hr, hg, hb] = app.tuning.search_match_bg;
    let sel_style = Style::default().bg(Color::Rgb(sr, sg, sb));
    let search_style = Style::default().bg(Color::Rgb(hr, hg, hb));
    // Teleport labels: high-contrast chip (accent bg, dark fg, bold).
    let label_style = Style::default()
        .bg(rgb(app.tuning.theme_accent_bright))
        .fg(rgb(app.tuning.theme_chip_fg))
        .add_modifier(Modifier::BOLD);

    let numbers = app.tuning.line_numbers;
    for row_off in 0..vp_h {
        let row = pane.scroll_row + row_off;
        if row >= line_count {
            // Blank gutter beyond end-of-buffer.
            let blank = " ".repeat(gutter_width(&app.tuning) as usize);
            lines.push(Line::from(Span::styled(blank, Style::default().fg(Color::DarkGray))));
        } else {
            let content = buf.line_str(row);
            let on_cursor = focused && row == pane.cursor_row;
            let mut spans = Vec::new();
            if numbers {
                let num_style = if on_cursor {
                    Style::default().fg(rgb(app.tuning.theme_accent_bright)).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                spans.push(Span::styled(format!("{:>4}│ ", row + 1), num_style));
            } else {
                // Slim pointer gutter: a marker on the cursor line, else blank.
                let (glyph, style) = if on_cursor {
                    ("▸ ", Style::default().fg(rgb(app.tuning.theme_accent)).add_modifier(Modifier::BOLD))
                } else {
                    ("  ", Style::default())
                };
                spans.push(Span::styled(glyph, style));
            }
            let chars: Vec<char> = content.chars().collect();

            // Per-char highlight map: 0 none, 1 selection, 2 isearch match.
            let mut hl: Vec<u8> = vec![0; chars.len()];
            if let Some(((sr, sc), (er, ec))) = sel {
                if row >= sr && row <= er {
                    let start = (if row == sr { sc } else { 0 }).min(chars.len());
                    let end = (if row == er { ec } else { chars.len() }).min(chars.len());
                    for h in hl.iter_mut().take(end).skip(start) { *h = 1; }
                }
            }
            let mut label_ch: Vec<Option<char>> = vec![None; chars.len()];
            if focused {
                for &(hr, hc, hlen) in &app.search_hl {
                    if hr == row {
                        let end = (hc + hlen).min(chars.len());
                        for h in hl.iter_mut().take(end).skip(hc.min(chars.len())) { *h = 2; }
                    }
                }
                // Teleport labels (kind 3) overwrite the first cell of each match.
                if app.search_pick {
                    for &(lr, lc, ch) in &app.search_labels {
                        if lr == row && lc < chars.len() {
                            hl[lc] = 3;
                            label_ch[lc] = Some(ch);
                        }
                    }
                }
            }

            if hl.iter().all(|&h| h == 0) {
                spans.push(Span::raw(content));
            } else {
                let mut i = 0;
                while i < chars.len() {
                    let kind = hl[i];
                    if kind == 3 {
                        let ch = label_ch[i].unwrap_or(chars[i]);
                        spans.push(Span::styled(ch.to_string(), label_style));
                        i += 1;
                        continue;
                    }
                    let mut j = i;
                    while j < chars.len() && hl[j] == kind && hl[j] != 3 { j += 1; }
                    let text: String = chars[i..j].iter().collect();
                    spans.push(match kind {
                        1 => Span::styled(text, sel_style),
                        2 => Span::styled(text, search_style),
                        _ => Span::raw(text),
                    });
                    i = j;
                }
            }
            lines.push(Line::from(spans));
        }
    }

    frame.render_widget(Paragraph::new(Text::from(lines)), inner);

    if focused {
        let sy = inner.y + (pane.cursor_row.saturating_sub(pane.scroll_row)) as u16;
        let sx = inner.x + gutter_width(&app.tuning) + pane.cursor_col as u16;
        if sy < inner.y + inner.height && sx < inner.x + inner.width {
            return Some((sx, sy));
        }
    }
    None
}

// ── Splash (day-0 banner) ─────────────────────────────────────────────────────

/// Parse a truecolor-ANSI line (`\x1b[38;2;r;g;bm` fg + `\x1b[0m` reset) into a
/// ratatui `Line`, also returning its *visible* width (escapes excluded).
fn ansi_to_line(raw: &str) -> (Line<'static>, u16) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut buf = String::new();
    let mut width: u16 = 0;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), style));
            }
            if chars.peek() == Some(&'[') {
                chars.next();
            }
            let mut code = String::new();
            for nc in chars.by_ref() {
                if nc == 'm' {
                    break;
                }
                code.push(nc);
            }
            if code == "0" || code.is_empty() {
                style = Style::default();
            } else if let Some(rest) = code.strip_prefix("38;2;") {
                let p: Vec<u8> = rest.split(';').filter_map(|x| x.parse().ok()).collect();
                if p.len() == 3 {
                    style = Style::default().fg(Color::Rgb(p[0], p[1], p[2]));
                }
            }
        } else {
            buf.push(c);
            width += 1;
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style));
    }
    (Line::from(spans), width)
}

fn render_splash(frame: &mut Frame, app: &App, inner: Rect) {
    let t = &app.tuning;
    // Overlay: wipe whatever's underneath (terminal shell or empty editor).
    frame.render_widget(Clear, inner);

    // Parse the rich ANSI banner; fall back to a plain wordmark when narrow.
    let parsed: Vec<(Line, u16)> = crate::banner::BANNER_LINES
        .iter()
        .map(|l| ansi_to_line(l))
        .collect();
    let banner_w = parsed.iter().map(|(_, w)| *w).max().unwrap_or(0);
    let big = inner.width >= banner_w && inner.height >= (parsed.len() as u16 + 7);

    let mut lines: Vec<Line> = Vec::new();
    if big {
        // Uniform left pad so the art's internal spacing stays aligned.
        let pad = (inner.width.saturating_sub(banner_w) / 2) as usize;
        for (line, _) in parsed {
            let mut spans = vec![Span::raw(" ".repeat(pad))];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        lines.push(Line::raw(""));
    } else {
        lines.push(Line::from(Span::styled(
            "M A R S",
            Style::default().fg(rgb(t.theme_accent)).add_modifier(Modifier::BOLD),
        )).centered());
        lines.push(Line::from(Span::styled(
            "mission control for your terminal",
            Style::default().fg(rgb(t.theme_accent_bright)).add_modifier(Modifier::ITALIC),
        )).centered());
        lines.push(Line::raw(""));
    }

    // Key commands — rendered as one aligned block (keys right-justified into a
    // column, descriptions left-aligned), the whole block centered. Per-line
    // centering made these look ragged; a single left pad keeps the columns true.
    let cmds: &[(&str, &str)] = &[
        ("C-Space", "mission control — search actions · ! shell · ? ask the agent"),
        ("C-x C-f", "navigator — browse & jump to any project file"),
        ("C-t",     "space warp — tabs, panes, splits, open terminal"),
        ("C-u",     "time-travel — scrub back through undo history"),
        ("C-x C-d", "detach — work keeps running while you're gone"),
        ("C-g",     "cancel anything"),
    ];
    let keyw = cmds.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let block_w = cmds
        .iter()
        .map(|(_, d)| keyw + 3 + d.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let lpad = " ".repeat((inner.width.saturating_sub(block_w) / 2) as usize);
    let key_style = Style::default().fg(rgb(t.theme_accent_bright)).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::Gray);
    for (k, d) in cmds {
        lines.push(Line::from(vec![
            Span::raw(lpad.clone()),
            Span::styled(format!("{k:>keyw$}"), key_style),
            Span::raw("   "),
            Span::styled(d.to_string(), desc_style),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "or just start typing",
        Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
    )).centered());

    // Vertically center the banner block.
    let block_h = lines.len() as u16;
    let top_pad = inner.height.saturating_sub(block_h) / 2;
    let area = Rect {
        x: inner.x,
        y: inner.y + top_pad,
        width: inner.width,
        height: block_h.min(inner.height),
    };
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// Greedy word-wrap to `width` columns (char-count approximate; ASCII-dominant
/// terminal text). Overlong words are hard-split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
        while line.chars().count() > width {
            let head: String = line.chars().take(width).collect();
            out.push(head);
            line = line.chars().skip(width).collect();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// The shift report — the save-state restore. The MARS wordmark up top (centered),
/// then a plain-English persona-voiced situation briefing (the star — it streams
/// in, justified within a centered measure), then a compact glyph manifest of the
/// workstreams. Splash pattern: Clear + one centered Paragraph. Any key resumes.
fn render_shift_report(frame: &mut Frame, app: &App, inner: Rect) {
    let Some(rep) = app.shift_report.as_ref() else { return };
    frame.render_widget(Clear, inner);
    let accent = rgb(app.tuning.theme_accent);
    let bright = rgb(app.tuning.theme_accent_bright);
    let teal = rgb(app.tuning.theme_terminal);
    let green = rgb(app.tuning.theme_healthy);
    let dim = Style::default().fg(Color::DarkGray);
    let white = Style::default().fg(Color::White);
    let cw = inner.width as usize;
    // The reading measure: one centered column the prose and manifest share, so
    // every element hangs off the same axis down the middle of the screen.
    let bw = (cw.saturating_sub(8)).clamp(24, 64);
    let block_pad = " ".repeat(cw.saturating_sub(bw) / 2);
    // Prepend padding to a span list so its visible width centers in the page.
    let centered = |spans: Vec<Span<'static>>, vis_len: usize| -> Line<'static> {
        let pad = " ".repeat(cw.saturating_sub(vis_len) / 2);
        let mut v = vec![Span::raw(pad)];
        v.extend(spans);
        Line::from(v)
    };
    let center1 = |s: String, style: Style| -> Line<'static> {
        let len = s.chars().count();
        centered(vec![Span::styled(s, style)], len)
    };

    // The boot-up reveal: elements come online over ~0.5s, worst-news-first.
    let animate = app.tuning.mission_briefing_animate == 1;
    let elapsed = if animate { rep.shown_at.elapsed().as_millis() } else { u128::MAX };
    let rev = crate::briefing::reveal_at(elapsed, rep.rows.len());

    let mut lines: Vec<Line> = Vec::new();
    // The MARS wordmark (instant — the console's always-on identity), centered as
    // a block so its internal art stays aligned while the whole mark sits mid-page.
    let logo_rows = &crate::banner::BANNER_LINES[1..=9];
    if inner.height as usize > rep.rows.len() + logo_rows.len() + 14 {
        let logo: Vec<(Line, u16)> = logo_rows.iter().map(|r| ansi_to_line(r)).collect();
        let logo_w = logo.iter().map(|(_, wd)| *wd as usize).max().unwrap_or(0);
        let logo_pad = " ".repeat(cw.saturating_sub(logo_w) / 2);
        for (line, _) in logo {
            let mut spans = vec![Span::raw(logo_pad.clone())];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(""));
    }
    // Caption: MISSION BRIEFING · T+HH:MM:SS mission clock · status ribbon.
    let (nf, nb, nd, nr) = (
        rep.rows.iter().filter(|r| r.verdict == crate::briefing::Verdict::Failed).count(),
        rep.rows.iter().filter(|r| r.verdict == crate::briefing::Verdict::Blocked).count(),
        rep.rows.iter().filter(|r| r.verdict == crate::briefing::Verdict::Done).count(),
        rep.rows.iter().filter(|r| r.verdict == crate::briefing::Verdict::Running).count(),
    );
    let ribbon = if rep.rows.is_empty() {
        " · all quiet".to_string()
    } else {
        let mut parts = Vec::new();
        if nf > 0 { parts.push(format!("✗{nf}")); }
        if nb > 0 { parts.push(format!("⏸{nb}")); }
        if nd > 0 { parts.push(format!("✓{nd}")); }
        if nr > 0 { parts.push(format!("●{nr}")); }
        format!(" · {}", parts.join(" "))
    };
    let title = "MISSION BRIEFING";
    let caption = format!("   T+ {}{ribbon}", crate::briefing::fmt_clock(rep.away_secs));
    let cap_len = title.chars().count() + caption.chars().count();
    lines.push(centered(
        vec![
            Span::styled(title.to_string(), Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            Span::styled(caption, dim),
        ],
        cap_len,
    ));
    if let Some(m) = &rep.mission {
        lines.push(center1(format!("mission: {m}"), dim));
    }
    lines.push(Line::from(""));

    // The briefing prose. The model emits four blocks — greeting, summary, action
    // items, sign-off — and the first three fill a FIXED vessel above the manifest
    // (the sign-off is held for below it, the peak-end beat). Three moves make the
    // fill feel deliberate rather than jittery:
    //   · while the call is in flight, a mission-control word flashes in the slot
    //     the greeting will take — so the prose never visibly swaps a backup stub;
    //   · the model text is revealed at a steady rate behind a cursor (a typewriter),
    //     not in the ragged bursts the network delivers;
    //   · the prose region is padded to a reserved height, so it is a shaped vessel
    //     the text fills top-down and nothing below it shifts as more arrives.
    let type_ms = app.tuning.mission_briefing_type_ms.max(1) as u128;
    let prose_rows = (app.tuning.mission_briefing_prose_rows as usize)
        .min((inner.height as usize).saturating_sub(6))
        .max(2);
    let loading = animate && rep.narrative_streaming && !rep.narrative_from_model;
    let target_len = rep.narrative.chars().count();
    // How many chars of the model text have been revealed: everything, unless we're
    // animating an in-flight model stream, in which case it advances on the clock.
    let show_n = if !animate || !rep.narrative_from_model {
        target_len
    } else {
        rep.stream_started_at
            .map(|s| (s.elapsed().as_millis() / type_ms) as usize)
            .unwrap_or(0)
            .min(target_len)
    };
    let typing = animate && rep.narrative_from_model && (show_n < target_len || rep.narrative_streaming);
    let shown: String = rep.narrative.chars().take(show_n).collect();

    let mut prose: Vec<Line> = Vec::new();
    let mut signoff: Option<String> = None;
    if loading {
        let idx = (rep.shown_at.elapsed().as_millis() / crate::briefing::LOADING_FLASH_MS) as usize
            % crate::briefing::BRIEF_LOADING.len();
        // Anchored at the block's left edge — where the greeting's first letter
        // will land — so the swap to real text has no lateral jump.
        prose.push(Line::from(Span::styled(
            format!("{block_pad}{}…", crate::briefing::BRIEF_LOADING[idx]),
            Style::default().fg(accent).add_modifier(Modifier::ITALIC),
        )));
    } else {
        let cursor = if typing { "▏" } else { "" };
        let full = format!("{shown}{cursor}");
        let paras: Vec<&str> = full.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).collect();
        let npar = paras.len();
        let has_signoff = npar >= 4;
        let above = if has_signoff { &paras[..npar - 1] } else { &paras[..] };
        signoff = has_signoff.then(|| paras[npar - 1].to_string());
        let above_n = above.len();
        for (pi, para) in above.iter().enumerate() {
            let style = if pi == 0 {
                Style::default().fg(accent).add_modifier(Modifier::BOLD) // greeting
            } else if above_n >= 3 && pi == above_n - 1 {
                Style::default().fg(bright).add_modifier(Modifier::BOLD) // action items
            } else {
                white // the summary
            };
            // Every prose line is anchored at the block's left edge and left ragged
            // — no word is ever moved to justify it, so nothing shifts as the
            // typewriter advances.
            for l in wrap(para, bw) {
                prose.push(Line::from(Span::styled(format!("{block_pad}{l}"), style)));
            }
            prose.push(Line::from(""));
        }
        // A quiet return still feels intentional: one dim radar line under the prose.
        if rep.rows.is_empty() {
            prose.push(center1("·   ·   ◜   ·   ·".to_string(), dim));
            prose.push(Line::from(""));
        }
    }
    // Pad the prose to its reserved height so the vessel is a fixed shape (only when
    // animating; instant mode packs tight). Overflow just grows the vessel.
    if animate {
        while prose.len() < prose_rows {
            prose.push(Line::from(""));
        }
    }
    lines.extend(prose);
    // Everything above here is a fixed height across frames; the manifest and
    // sign-off below reveal into reserved space without moving it.
    let fixed_head = lines.len();
    let manifest_full: usize = if rep.rows.is_empty() {
        0
    } else {
        1 + rep
            .rows
            .iter()
            .map(|r| {
                let needsyou = matches!(
                    r.verdict,
                    crate::briefing::Verdict::Failed | crate::briefing::Verdict::Blocked
                );
                let has_detail =
                    needsyou && (r.cwd.is_some() || r.exit.is_some() || r.error_excerpt.is_some());
                1 + usize::from(has_detail)
            })
            .sum::<usize>()
    };
    let signoff_full = if rep.rows.is_empty() { 2 } else { 6 };

    // The manifest as a systems board, hung off the same centered measure: a left
    // severity stripe, needs-you rows bright and concluded ones receding. Rows
    // cascade in, failures first. Wins render in teal — never the danger hue.
    if !rep.rows.is_empty() && rev.rows > 0 {
        lines.push(Line::from(Span::styled(format!("{block_pad}{}", "─".repeat(bw)), dim)));
    }
    for r in rep.rows.iter().take(rev.rows) {
        let needsyou = matches!(r.verdict, crate::briefing::Verdict::Failed | crate::briefing::Verdict::Blocked);
        let goodnews = r.verdict == crate::briefing::Verdict::Done
            && r.dur_secs.map(|d| d > crate::briefing::GOODNEWS_SECS).unwrap_or(false);
        // Danger keeps the warm hues; wins (done/running, and the good-news ★) are
        // always teal — a success never wears the failure colour.
        let hue = match r.verdict {
            crate::briefing::Verdict::Failed => bright,
            crate::briefing::Verdict::Blocked => accent,
            crate::briefing::Verdict::Running => green, // healthy work — calm, dismissible
            crate::briefing::Verdict::Done => teal,     // the win keeps its teal

            _ => Color::DarkGray,
        };
        let body_style = if needsyou || goodnews { white } else { dim };
        let glyph = if goodnews { "★" } else { r.verdict.glyph() };
        let tab = if r.tab.is_empty() { String::new() } else { format!("[{}] ", r.tab) };
        let mut meta = Vec::new();
        if let Some(d) = r.dur_secs.filter(|d| *d > 0) {
            meta.push(format!("ran {}", crate::briefing::fmt_secs(d)));
        }
        if let Some(a) = r.ago_secs.filter(|a| *a > 0) {
            meta.push(format!("{} ago", crate::briefing::fmt_secs(a)));
        }
        let meta = if meta.is_empty() { String::new() } else { format!("  ({})", meta.join(", ")) };
        let body: String = format!("{tab}{}{meta}", r.text).chars().take(bw).collect();
        lines.push(Line::from(vec![
            Span::raw(block_pad.clone()),
            Span::styled("▎ ".to_string(), Style::default().fg(hue)),
            Span::styled(format!("{glyph} "), Style::default().fg(hue).add_modifier(Modifier::BOLD)),
            Span::styled(body, body_style),
        ]));
        // The "why" under failed/blocked rows: cwd · exit · first error line.
        if needsyou {
            let mut detail = Vec::new();
            if let Some(c) = &r.cwd { detail.push(c.clone()); }
            if let Some(x) = r.exit { detail.push(format!("exit {x}")); }
            if let Some(e) = &r.error_excerpt {
                if let Some(first) = e.lines().find(|l| !l.trim().is_empty()) {
                    detail.push(format!("“{}”", first.trim()));
                }
            }
            if !detail.is_empty() {
                let d: String = format!("{block_pad}   {}", detail.join(" · ")).chars().take(cw).collect();
                lines.push(Line::from(Span::styled(d, dim)));
            }
        }
    }

    // The sign-off (the last word) and footer arrive once the board is up.
    if rev.signoff {
        if !rep.rows.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(format!("{block_pad}{}", "─".repeat(bw)), dim)));
        }
        if let Some(s) = &signoff {
            // Left-anchored like the prose, so the last word types in left-to-right.
            let so = Style::default().fg(accent).add_modifier(Modifier::ITALIC);
            for l in wrap(s, bw) {
                lines.push(Line::from(Span::styled(format!("{block_pad}{l}"), so)));
            }
            lines.push(Line::from(""));
        }
        lines.push(center1("any key resumes exactly where you left off".to_string(), dim));
    }

    // Center once against the RESERVED height (fixed head + full manifest + tail),
    // not the live line count — so the composition holds still while the manifest
    // cascades and the prose types in. In instant mode there's no reveal, so the
    // live height is exact.
    let total = if animate {
        (fixed_head + manifest_full + signoff_full) as u16
    } else {
        lines.len() as u16
    };
    let top_pad = inner.height.saturating_sub(total) / 2;
    let area = Rect {
        x: inner.x,
        y: inner.y + top_pad,
        width: inner.width,
        height: (lines.len() as u16).max(total).min(inner.height),
    };
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn render_terminal_pane(
    frame: &mut Frame,
    app: &App,
    pane_id: PaneId,
    rect: Rect,
    focused: bool,
) -> Option<(u16, u16)> {
    let pane = app.panes.get(&pane_id)?;
    let term_id = match pane.content {
        PaneContent::Terminal(id) => id,
        _ => return None,
    };

    let exited = app.terms.get(&term_id).map(|t| t.exited).unwrap_or(true);
    let offset = app.terms.get(&term_id).map(|t| t.view_offset()).unwrap_or(0);
    // The pane border/title carry NO status glyph — status lives in the tab bar, so
    // the divider line stays uncluttered. Border color is focus/exited only.
    let border_style = if exited {
        Style::default().fg(rgb(app.tuning.theme_accent_dark))
    } else if focused {
        Style::default().fg(rgb(app.tuning.theme_terminal))
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let base = pane.title.as_deref().unwrap_or("terminal");
    let title = if exited {
        format!(" {base} · exited ")
    } else if offset > 0 {
        format!(" {base} ↑{offset} ")
    } else {
        format!(" {base} ")
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().add_modifier(if focused { Modifier::BOLD } else { Modifier::empty() }),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let term = match app.terms.get(&term_id) {
        Some(t) => t,
        None => {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "(terminal closed)",
                    Style::default().fg(Color::DarkGray),
                )),
                inner,
            );
            return None;
        }
    };

    // Render the vt100 screen grid into the pane.
    let screen = term.screen();
    let (rows, cols) = screen.size();
    let vh = inner.height.min(rows);
    let vw = inner.width.min(cols);

    // A live mouse selection in THIS terminal → highlight its cells.
    let sel = app.term_sel.filter(|s| s.tid == term_id).map(|s| {
        let (mut a, mut b) = (s.anchor, s.end);
        if b < a { std::mem::swap(&mut a, &mut b); }
        (a, b, s.vw.saturating_sub(1))
    });
    let [sr, sg, sb] = app.tuning.selection_bg;
    let sel_bg = Color::Rgb(sr, sg, sb);

    let mut lines: Vec<Line> = Vec::with_capacity(vh as usize);
    for row in 0..vh {
        let mut spans: Vec<Span> = Vec::with_capacity(vw as usize);
        for col in 0..vw {
            if let Some(cell) = screen.cell(row, col) {
                let contents = cell.contents();
                let ch = if contents.is_empty() { " ".to_string() } else { contents };
                let mut style = Style::default()
                    .fg(conv_color(cell.fgcolor()))
                    .bg(conv_color(cell.bgcolor()));
                if cell.bold()      { style = style.add_modifier(Modifier::BOLD); }
                if cell.italic()    { style = style.add_modifier(Modifier::ITALIC); }
                if cell.underline() { style = style.add_modifier(Modifier::UNDERLINED); }
                if cell.inverse()   { style = style.add_modifier(Modifier::REVERSED); }
                if let Some((a, b, last)) = sel {
                    let c0 = if row == a.0 { a.1 } else { 0 };
                    let c1 = if row == b.0 { b.1 } else { last };
                    if row >= a.0 && row <= b.0 && col >= c0 && col <= c1 {
                        style = style.bg(sel_bg);
                    }
                }
                spans.push(Span::styled(ch, style));
            } else {
                spans.push(Span::raw(" "));
            }
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);

    // Dead shell: overlay the dismissal hint on the bottom row.
    if exited && inner.height > 0 {
        let notice = Rect { x: inner.x, y: inner.y + inner.height - 1, width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(
                " process exited — Enter closes this pane ",
                Style::default()
                    .fg(rgb(app.tuning.theme_chip_fg))
                    .bg(rgb(app.tuning.theme_accent_dark))
                    .add_modifier(Modifier::BOLD),
            )),
            notice,
        );
        return None;
    }

    // Report the terminal's own cursor position when focused.
    if focused && !screen.hide_cursor() {
        let (cr, cc) = screen.cursor_position();
        let cx = inner.x + cc.min(vw.saturating_sub(1));
        let cy = inner.y + cr.min(vh.saturating_sub(1));
        return Some((cx, cy));
    }
    None
}

/// Map a vt100 cell color to a ratatui color.
fn conv_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default    => Color::Reset,
        vt100::Color::Idx(i)     => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

// ── Status bar ───────────────────────────────────────────────────────────────

/// Hint pairs for the status bar. Edit-mode hints are derived live from the
/// keymap so they stay honest after a remap; other modes are fixed UI keys.
fn status_hints(app: &App) -> Vec<(String, String)> {
    if matches!(app.mode, Mode::Edit) {
        let mut v = vec![(bar_open_keys(app), "⌕ commands".to_string())];
        for (action, label) in [
            (Action::Save, "save"),
            (Action::ToggleFileTree, "open"),
            (Action::Search, "search"),
        ] {
            if let Some(b) = app.keys.binding_for(&action) {
                v.push((b, label.to_string()));
            }
        }
        v.push(("C-g".to_string(), "cancel".to_string()));
        v
    } else if matches!(app.mode, Mode::Terminal) {
        // Live-derived like Edit: the bar-open chord is remappable, and C-g here
        // means "leave the terminal for the editor" — NOT session detach (which
        // is C-x C-d). Naming it "detach" scared tmux refugees.
        vec![
            (bar_open_keys(app), "commands".to_string()),
            ("C-g".to_string(), "to editor".to_string()),
            ("type".to_string(), "to shell".to_string()),
        ]
    } else {
        app.mode
            .hints()
            .iter()
            .map(|(k, a)| (k.to_string(), a.to_string()))
            .collect()
    }
}

/// The chords that open the command bar, rendered (e.g. "C-Spc / M-x").
fn bar_open_keys(app: &App) -> String {
    app.keys
        .bar_open
        .iter()
        .map(|c| crate::config::render_chords(std::slice::from_ref(c)))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let accent = rgb(app.tuning.theme_accent);
    let sand   = rgb(app.tuning.theme_accent_bright);
    let chipfg = rgb(app.tuning.theme_chip_fg);
    // Brand lives in chrome; green stays semantic (a live shell process).
    let (mode_fg, mode_bg, key_bg, key_fg) = match &app.mode {
        Mode::Edit     => (chipfg, accent,       accent, chipfg),
        Mode::Prompt   => (chipfg, sand,         sand,   chipfg),
        Mode::Tab      => (chipfg, accent,       accent, chipfg),
        Mode::Bar      => (chipfg, accent,       accent, chipfg),
        Mode::Tree     => (chipfg, accent,       accent, chipfg),
        Mode::Undo     => (chipfg, sand,         sand,   chipfg),
        Mode::Terminal => {
            let teal = rgb(app.tuning.theme_terminal);
            (Color::White, teal, teal, Color::White)
        }
    };

    // Left side: mode label + hints
    let mut spans: Vec<Span> = vec![
        Span::styled(
            format!(" {} ", app.mode.label()),
            Style::default()
                .fg(mode_fg)
                .bg(mode_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
    ];

    // Recording indicator: when LLM debug logging is on, an unmistakable red chip
    // (+ the active memory variant) so a full day of eval capture is never silently
    // lost. Derived live from llm_log::enabled().
    if crate::llm_log::enabled() {
        let mem = crate::retrieval::MemoryMode::from_env();
        let label = if matches!(mem, crate::retrieval::MemoryMode::None) {
            " ● REC ".to_string()
        } else {
            format!(" ● REC mem:{} ", mem.as_str())
        };
        spans.push(Span::styled(
            label,
            Style::default().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ", Style::default()));
    }

    for (key, action) in status_hints(app) {
        spans.push(Span::styled(
            format!(" {} ", key),
            Style::default()
                .fg(key_fg)
                .bg(key_bg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(":{} ", action),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ", Style::default()));
    }

    // Transient info (pending prefix / status message) trails the hints on the
    // left, so the position readout on the right is never displaced.
    if !app.pending_prefix.is_empty() {
        spans.push(Span::styled(
            format!(" {}- ", crate::config::render_chords(&app.pending_prefix)),
            Style::default().fg(rgb(app.tuning.theme_accent_bright)).add_modifier(Modifier::BOLD),
        ));
    } else if let Some(msg) = &app.status_msg {
        spans.push(Span::styled(
            format!(" {msg} "),
            Style::default().fg(rgb(app.tuning.theme_accent_bright)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    // Position readout — ALWAYS right-aligned on top, so it can't be truncated
    // by the left hints or hidden by a status message. Ln/Col for editor panes.
    let pane = app.focused_pane();
    let session = app.session_name.as_ref().map(|s| format!("  ⚡{s}")).unwrap_or_default();
    let readout = match pane.content {
        PaneContent::Editor(buf_id) => {
            let name = app
                .buffers
                .get(&buf_id)
                .map(|b| format!("{}{}", b.name, if b.modified { " ●" } else { "" }))
                .unwrap_or_default();
            format!("{name}   Ln {}, Col {}{session} ", pane.cursor_row + 1, pane.cursor_col + 1)
        }
        PaneContent::Terminal(_) => format!("terminal{session} "),
    };
    // The cross-workspace status aggregate lives in the top-right corner counter
    // (render_tab_bar), not down here — the bottom bar stays the position readout.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            readout,
            Style::default().fg(rgb(app.tuning.theme_accent_bright)).add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Right),
        area,
    );
}

// ── Control bar (bottom row, always visible) ──────────────────────────────────

fn render_control_bar(frame: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Bar => {
            // Show current query with mode prefix
            let palette = match app.palette.as_ref() {
                Some(p) => p,
                None => return,
            };
            let mode_label = match palette.bar_mode {
                BarMode::Command => "CMD",
                BarMode::Ask     => "ASK",
                BarMode::Shell   => "SH !",
            };
            let prompt = format!("[{}] › {}▎", mode_label, palette.query);
            let style = Style::default()
                .fg(rgb(app.tuning.theme_accent))
                .add_modifier(Modifier::BOLD);
            // In-bar quick keys, taught right where they work — only while the
            // query is empty, because that's the only time they fire.
            let legend = if palette.bar_mode == BarMode::Command && palette.query.is_empty() {
                let keys: Vec<String> = crate::palette::bar_quick_legend()
                    .iter()
                    .map(|(k, what)| format!("{k} {what}"))
                    .collect();
                format!("   {}", keys.join(" · "))
            } else {
                String::new()
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prompt.clone(), style),
                    Span::styled(legend, Style::default().fg(Color::DarkGray)),
                ])),
                area,
            );
            // Set cursor at end of prompt
            let cx = area.x + prompt.chars().count() as u16 - 1; // before the ▎
            if cx < area.x + area.width {
                frame.set_cursor_position((cx, area.y));
            }
        }
        Mode::Prompt => {
            if let Some(p) = app.prompt.as_ref() {
                // Live search shows an `n/m` match counter (and a Tab hint).
                let extra = if p.kind == crate::app::PromptKind::Search {
                    match app.isearch_status() {
                        Some((cur, total)) if total > 0 => {
                            let pick = if total >= 2 { "  ⇥ jump" } else { "" };
                            format!("  {cur}/{total}{pick}")
                        }
                        _ if !p.input.is_empty() => "  (no match)".to_string(),
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                let text = format!("{}{}{}", p.label, p.input, extra);
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        text.clone(),
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                    )),
                    area,
                );
                // Cursor sits after the query itself, before the counter suffix.
                let cx = area.x + (p.label.chars().count() + p.input.chars().count()) as u16;
                if cx < area.x + area.width {
                    frame.set_cursor_position((cx, area.y));
                }
            }
        }
        _ => {
            // Idle hint — derived from the live keymap, never hardcoded.
            let open = app.keys.binding_for(&Action::ToggleFileTree).unwrap_or_default();
            let search = app.keys.binding_for(&Action::Search).unwrap_or_default();
            let hint = format!(
                "  {}  commands    {}  open    {}  search    C-g  cancel",
                bar_open_keys(app), open, search
            );
            frame.render_widget(
                Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
                area,
            );
        }
    }
}

// ── Bar dropdown (grows upward from control bar) ──────────────────────────────

/// Returns the rect it drew (None when nothing rendered) so the cursor-anchored
/// shell overlay can yield to it instead of drawing on top.
/// Truncate to a column width with an ellipsis, so names/why-lines never hard-clip.
fn ellip(s: &str, w: usize) -> String {
    if w == 0 { return String::new(); }
    if s.chars().count() <= w { return s.to_string(); }
    let t: String = s.chars().take(w.saturating_sub(1)).collect();
    format!("{t}…")
}

/// The Commands list — the classic launcher rows: in-bar quick key, live binding
/// badge, label, dim description. `active` gates whether the selection highlights.
fn command_lines(app: &App, rows: &[crate::palette::PaletteRow], sel: usize, navigated: bool, active: bool, max_rows: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let body_max = max_rows.max(1);
    let scroll = if sel >= body_max { sel + 1 - body_max } else { 0 };
    for (idx, row) in rows.iter().enumerate().skip(scroll).take(body_max) {
        let selected = active && navigated && idx == sel;
        let bg = if selected { Color::DarkGray } else { Color::Reset };
        let has_sub = matches!(row.kind, ItemKind::Submenu(_));
        let quick = match &row.kind { ItemKind::Run(a) => crate::palette::bar_quick_key(a), _ => None };
        let binding = match &row.kind { ItemKind::Run(a) => app.keys.binding_for(a).unwrap_or_default(), _ => String::new() };
        let desc = if row.description.is_empty() { String::new() } else { format!(" — {}", row.description) };
        let type_mark = if has_sub { " ▸" } else { "" };
        let quick_span = match quick {
            Some(q) => Span::styled(format!(" {q} "), Style::default().fg(rgb(app.tuning.theme_chip_fg)).bg(rgb(app.tuning.theme_accent)).add_modifier(Modifier::BOLD)),
            None => Span::styled("   ", Style::default().bg(bg)),
        };
        out.push(Line::from(vec![
            quick_span,
            Span::styled(format!(" {:<w$}", binding, w = app.tuning.binding_badge_width),
                Style::default().fg(rgb(app.tuning.theme_accent_bright)).bg(bg).add_modifier(Modifier::BOLD)),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(format!("{}{}", row.label, type_mark),
                if selected { Style::default().fg(rgb(app.tuning.theme_accent)).bg(bg).add_modifier(Modifier::BOLD) }
                else { Style::default().fg(Color::White).bg(bg) }),
            Span::styled(desc, Style::default().fg(Color::DarkGray).bg(bg)),
        ]));
    }
    out
}

/// One row per workspace: current marker ‹, verdict glyph (class color), id · name
/// (ellipsized), age right-aligned. Scrolls to keep the selection visible.
fn workspace_lines(app: &App, rows: &[crate::palette::PaletteRow], sel: usize, active: bool, width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let body_max = max_rows.max(1);
    let scroll = if sel >= body_max { sel + 1 - body_max } else { 0 };
    for (idx, row) in rows.iter().enumerate().skip(scroll).take(body_max) {
        let s = active && idx == sel;
        let bg = if s { Color::DarkGray } else { Color::Reset };
        let (glyph, vcolor, id, cur) = match &row.kind {
            ItemKind::Surface(sr) => {
                let (g, c) = verdict_style(app, sr.verdict).unwrap_or(("·", Color::DarkGray));
                (if g.is_empty() { "·" } else { g }, c, sr.tab_index + 1, sr.tab_index == app.active_tab)
            }
            _ => ("·", Color::DarkGray, 0, false),
        };
        let marker = if cur { "‹" } else { " " };
        let age = match &row.kind {
            ItemKind::Surface(sr) if sr.age_secs > 0 => crate::briefing::fmt_secs(sr.age_secs),
            _ => String::new(),
        };
        // Budget: marker(1) glyph(2) "id · " age( ) — the name gets what's left.
        let name_budget = (width as usize).saturating_sub(3 + format!("{id} · ").len() + age.chars().count() + 2);
        let mut spans = vec![
            Span::styled(marker.to_string(), Style::default().fg(rgb(app.tuning.theme_accent_bright)).bg(bg).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{glyph} "), Style::default().fg(vcolor).bg(bg).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{id} · {}", ellip(&row.label, name_budget)),
                Style::default().fg(if s { vcolor } else { Color::White }).bg(bg)
                    .add_modifier(if s { Modifier::BOLD } else { Modifier::empty() }),
            ),
        ];
        let lw: usize = spans.iter().map(|x| x.content.chars().count()).sum();
        let pad = (width as usize).saturating_sub(lw + age.chars().count() + 1);
        spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
        spans.push(Span::styled(format!("{age} "), Style::default().fg(Color::DarkGray).bg(bg)));
        out.push(Line::from(spans));
    }
    out
}

/// The detail strip: the highlighted workspace in full — a rule separator, then
/// name · status · age, its why-line (ellipsized), and the ↵ verb. Detail follows
/// focus (no expand key), so moving the selection updates it in place.
fn detail_lines(app: &App, row: Option<&crate::palette::PaletteRow>, width: u16) -> Vec<Line<'static>> {
    use crate::briefing::Verdict;
    let w = width as usize;
    let mut out = vec![Line::from(Span::styled("─".repeat(w), Style::default().fg(Color::DarkGray)))];
    let Some(ItemKind::Surface(s)) = row.map(|r| &r.kind) else { return out };
    let (glyph, vcolor) = verdict_style(app, s.verdict).unwrap_or(("·", Color::DarkGray));
    let vlabel = match s.verdict {
        Verdict::Blocked => "blocked", Verdict::Failed => "failed",
        Verdict::Running => "running", Verdict::Done => "done", Verdict::Context => "idle",
    };
    let age = if s.age_secs > 0 { format!(" · {}", crate::briefing::fmt_secs(s.age_secs)) } else { String::new() };
    let wsname = row.map(|r| r.label.clone()).unwrap_or_default();
    let head = ellip(&format!("{wsname} · {vlabel}{age}"), w.saturating_sub(2));
    out.push(Line::from(vec![
        Span::styled(format!("{glyph} "), Style::default().fg(vcolor).add_modifier(Modifier::BOLD)),
        Span::styled(head, Style::default().fg(vcolor).add_modifier(Modifier::BOLD)),
    ]));
    let why = row.map(|r| r.description.clone()).unwrap_or_default();
    if !why.is_empty() {
        out.push(Line::from(Span::styled(ellip(&why, w), Style::default().fg(Color::White))));
    }
    out.push(Line::from(Span::styled("↵ jump", Style::default().fg(rgb(app.tuning.theme_accent_bright)))));
    out
}

/// The legend — the status class taxonomy + its colors, so the scheme is obvious
/// rather than remembered. Two compact lines to fit the navigator-width panel.
fn legend_lines(app: &App) -> Vec<Line<'static>> {
    use crate::briefing::Verdict;
    let chip = |v: Verdict, label: &str| -> Vec<Span<'static>> {
        match verdict_style(app, v) {
            Some((g, c)) => vec![
                Span::styled(format!("{g} "), Style::default().fg(c).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{label}  "), Style::default().fg(Color::DarkGray)),
            ],
            None => vec![],
        }
    };
    let mut l1 = vec![Span::raw(" ")]; l1.extend(chip(Verdict::Blocked, "blocked")); l1.extend(chip(Verdict::Failed, "failed"));
    let mut l2 = vec![Span::raw(" ")]; l2.extend(chip(Verdict::Running, "running")); l2.extend(chip(Verdict::Done, "done"));
    l2.push(Span::styled("· idle", Style::default().fg(Color::DarkGray)));
    vec![Line::from(l1), Line::from(l2)]
}

/// The Workspaces board — a separate bordered panel (navigator-width), reached from
/// the command bar with the ← arrow. Its own title, a rule separator before the
/// detail, and the legend pinned to the bottom.
fn render_workspaces_panel(frame: &mut Frame, app: &App, rect: Rect) {
    let active = app.palette.as_ref().map(|p| p.column == crate::palette::BarColumn::Workspaces).unwrap_or(false);
    let sel = app.palette.as_ref().map(|p| p.sel_ws).unwrap_or(0);
    let border = if active { rgb(app.tuning.theme_accent) } else { Color::DarkGray };
    let block = Block::default()
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .border_style(Style::default().fg(border))
        .title(Span::styled(" WORKSPACES ", Style::default().fg(rgb(app.tuning.theme_accent)).add_modifier(Modifier::BOLD)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let rows = app.bar_workspace_rows();
    let ih = inner.height as usize;
    let legend_h = 2.min(ih);
    let detail_h = if ih > legend_h + 3 { 4 } else { 0 };
    let list_h = ih.saturating_sub(legend_h + detail_h);
    frame.render_widget(Paragraph::new(Text::from(workspace_lines(app, &rows, sel, active, inner.width, list_h))),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: list_h as u16 });
    if detail_h > 0 {
        frame.render_widget(Paragraph::new(Text::from(detail_lines(app, rows.get(sel), inner.width))),
            Rect { x: inner.x, y: inner.y + list_h as u16, width: inner.width, height: detail_h as u16 });
    }
    if legend_h > 0 {
        frame.render_widget(Paragraph::new(Text::from(legend_lines(app))),
            Rect { x: inner.x, y: inner.y + inner.height - legend_h as u16, width: inner.width, height: legend_h as u16 });
    }
}

/// The Commands launcher — the classic single-column dropdown, in its own bordered
/// box. `left_border` is dropped when the Workspaces panel sits to its left (that
/// panel's right border serves as the divider).
fn render_command_panel(frame: &mut Frame, app: &App, rect: Rect, left_border: bool, active: bool) {
    let palette = match app.palette.as_ref() { Some(p) => p, None => return };
    let borders = if left_border { Borders::TOP | Borders::LEFT | Borders::RIGHT } else { Borders::TOP | Borders::RIGHT };
    let border = if active { rgb(app.tuning.theme_accent) } else { Color::DarkGray };
    let block = Block::default()
        .borders(borders)
        .border_style(Style::default().fg(border));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let rows = app.bar_rows();
    let lines = command_lines(app, &rows, palette.selected, palette.navigated, active, inner.height as usize);
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_bar_dropdown(
    frame: &mut Frame,
    app: &App,
    pane_area: Rect,
    bar_area: Rect,
) -> Option<Rect> {
    let palette = app.palette.as_ref()?;
    let show_ws = app.bar_show_workspaces();
    let cmd_rows = app.bar_rows();
    if cmd_rows.is_empty() && !show_ws {
        return None;
    }

    let max_height = ((pane_area.height * app.tuning.panel_max_height_pct / 100) as usize)
        .min(app.tuning.dropdown_max_rows as usize) as u16;
    // The command list drives the height; the workspaces panel needs room for a few
    // rows + its detail(4) + legend(2). +1 for the top border.
    let cmd_need = cmd_rows.len() as u16 + 1;
    let ws_need = if show_ws { app.bar_workspace_rows().len().max(3) as u16 + 4 + 2 + 1 } else { 0 };
    let drop_h = cmd_need.max(ws_need).min(max_height).max(2);
    let full = Rect { x: bar_area.x, y: bar_area.y.saturating_sub(drop_h), width: bar_area.width, height: drop_h };
    frame.render_widget(Clear, full);

    if !show_ws {
        // No fleet to survey → the plain single-column launcher (previous behaviour).
        render_command_panel(frame, app, full, true, true);
        return Some(full);
    }

    // A separate WORKSPACES panel (navigator-width) beside the classic command
    // launcher. ← focuses the panel, → the launcher.
    let ws_w = app.tuning.tree_width.clamp(20, full.width.saturating_sub(30).max(20));
    let ws_rect = Rect { x: full.x, y: full.y, width: ws_w, height: drop_h };
    let cmd_rect = Rect { x: full.x + ws_w, y: full.y, width: full.width - ws_w, height: drop_h };
    render_workspaces_panel(frame, app, ws_rect);
    render_command_panel(frame, app, cmd_rect, false, palette.column == crate::palette::BarColumn::Commands);
    Some(full)
}

// ── Proactive notice line (W6 watch verdicts) ────────────────────────────────

fn render_notice(frame: &mut Frame, app: &App, pane_area: Rect) {
    let Some(n) = app.notices.first() else { return };
    if pane_area.height == 0 {
        return;
    }
    let row = Rect { x: pane_area.x, y: pane_area.bottom() - 1, width: pane_area.width, height: 1 };
    let (glyph, fg) = match n.kind {
        crate::app::NoticeKind::Failure => ("✗", rgb(app.tuning.theme_accent_bright)),
        crate::app::NoticeKind::Blocked => ("⏸", rgb(app.tuning.theme_accent)),
        crate::app::NoticeKind::Info => ("✓", rgb(app.tuning.theme_terminal)),
    };
    let more = if app.notices.len() > 1 { format!("  (+{} more)", app.notices.len() - 1) } else { String::new() };
    let text = format!(" {glyph} {}{more}   Esc dismiss ", n.text);
    frame.render_widget(Clear, row);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(Color::Black).bg(fg).add_modifier(Modifier::BOLD),
        ))),
        row,
    );
}

// ── Left file-tree sidebar (@ / C-x d) ───────────────────────────────────────

fn render_file_tree(frame: &mut Frame, app: &App, area: Rect) {
    let accent = rgb(app.tuning.theme_accent);
    let focused = matches!(app.mode, Mode::Tree);
    let border = if focused { rgb(app.tuning.theme_accent_bright) } else { Color::DarkGray };

    frame.render_widget(Clear, area);
    // Header: the filter query (⌕) while filtering, else the root folder name.
    let (root_name, filter) = app
        .file_tree
        .as_ref()
        .map(|t| {
            let name = t
                .root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| t.root.to_string_lossy().to_string());
            (name, t.filter.clone())
        })
        .unwrap_or_default();
    let title = if filter.is_empty() {
        format!(" Navigator · {root_name}/ ")
    } else {
        format!(" Navigator · ⌕ {filter} ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let selected = app.file_tree.as_ref().map(|t| t.selected).unwrap_or(0);
    let max_show = inner.height as usize;
    if max_show == 0 {
        return;
    }
    let scroll = if selected >= max_show { selected + 1 - max_show } else { 0 };

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (idx, row) in app.tree_rows.iter().enumerate().skip(scroll).take(max_show) {
        let is_sel = idx == selected && focused;
        // Selected row: a full-width accent band (unmistakable), like a chip.
        let bg = if is_sel { accent } else { Color::Reset };
        let indent = "  ".repeat(row.depth);
        let glyph = if row.updir {
            "↑ "
        } else if row.is_dir {
            if row.expanded { "▾ " } else { "▸ " }
        } else {
            "  "
        };
        let label = if row.is_dir && !row.updir {
            format!("{}/", row.label)
        } else {
            row.label.clone()
        };
        // Foreground: readable on the band when selected; folders bold+accent,
        // files white, `../` dim — otherwise.
        let chip = rgb(app.tuning.theme_chip_fg);
        let name_fg = if is_sel {
            chip
        } else if row.updir {
            rgb(app.tuning.theme_accent_bright) // visible "go up" affordance
        } else if row.is_dir {
            accent
        } else {
            Color::White
        };
        let glyph_fg = if is_sel { chip } else { accent };
        let mut modifier = Modifier::empty();
        if row.is_dir && !row.updir {
            modifier |= Modifier::BOLD;
        }
        // Pad to the full inner width so the selection band spans the row.
        let used = indent.chars().count() + glyph.chars().count() + label.chars().count();
        let pad = " ".repeat(width.saturating_sub(used));
        lines.push(Line::from(vec![
            Span::styled(format!("{indent}{glyph}"), Style::default().fg(glyph_fg).bg(bg)),
            Span::styled(label, Style::default().fg(name_fg).bg(bg).add_modifier(modifier)),
            Span::styled(pad, Style::default().bg(bg)),
        ]));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

// ── Shell-translate overlay (W3, anchored at the cursor — no eye-jump) ─────────

fn render_shell_overlay(frame: &mut Frame, app: &App, pane_area: Rect, avoid: Option<Rect>) {
    let query = app.palette.as_ref().map(|p| p.query.as_str()).unwrap_or("");
    let chipfg = rgb(app.tuning.theme_chip_fg);
    let accent = rgb(app.tuning.theme_accent);

    // The input line begins EXACTLY where the cursor was (no label prefix), so
    // it reads as typing in place. A tiny chip sits just left of it: `!` for the
    // pure-shell mode, `›` for the unified composer (shell OR a picked command).
    let shell_mode = app
        .palette
        .as_ref()
        .map(|p| matches!(p.bar_mode, BarMode::Shell))
        .unwrap_or(true);
    let navigated = app.palette.as_ref().map(|p| p.navigated).unwrap_or(false);
    // Empty query: show a placeholder so the composer is unmistakably present.
    let input = if query.is_empty() {
        format!("{} run a command… ", if shell_mode { "!" } else { "›" })
    } else {
        format!("{} {query} ", if shell_mode { "!" } else { "›" })
    };
    let configured = crate::agent::AgentConfig::from_env().is_configured();
    let err = app.agent_answer.as_deref().filter(|a| a.starts_with('⚠'));
    let hint = if app.agent_pending {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let sp = frames[(app.frame_tick / app.tuning.spinner_speed_ticks % 10) as usize];
        format!(" {sp} translating…")
    } else if app.shell_ready {
        " ✓ Enter runs · edit to change · Esc cancel".to_string()
    } else if let Some(e) = err {
        format!(" {e} · Enter runs literally · Esc")
    } else if !configured {
        " Enter runs (set GEMINI_API_KEY to type English) · Esc".to_string()
    } else if shell_mode {
        " type English, Enter translates → command · Esc".to_string()
    } else if navigated {
        " Enter runs the highlighted command · no match → shell · Esc".to_string()
    } else {
        " type a command (English ok) · ! pure shell · Esc".to_string()
    };

    let width = ((input.chars().count().max(hint.chars().count())) as u16)
        .min(pane_area.width.max(4));

    // Anchor the INPUT row on the cursor row; hint goes below (or above at the
    // bottom edge). Clamp inside the pane.
    let (cx, cy) = app.cursor_screen.unwrap_or((pane_area.x, pane_area.y));
    let max_x = pane_area.x + pane_area.width.saturating_sub(width);
    let x = cx.min(max_x).max(pane_area.x);
    let hint_below = cy + 1 < pane_area.y + pane_area.height;
    let (input_y, hint_y) = if hint_below { (cy, cy + 1) } else { (cy, cy.saturating_sub(1)) };

    let input_rect = Rect { x, y: input_y, width, height: 1 };
    let hint_rect = Rect { x, y: hint_y, width, height: 1 };
    // The dropdown outranks the anchored composer: when they'd collide (cursor
    // near the bottom), skip the overlay entirely so the menu stays readable.
    if let Some(d) = avoid {
        if input_rect.intersects(d) || hint_rect.intersects(d) {
            return;
        }
    }
    frame.render_widget(Clear, input_rect);
    frame.render_widget(Clear, hint_rect);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            input,
            Style::default().fg(chipfg).bg(accent).add_modifier(Modifier::BOLD),
        ))),
        input_rect,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray).bg(rgb(app.tuning.selection_bg)),
        ))),
        hint_rect,
    );
    // Text cursor sits right after the query (input is "! {query} ").
    let curx = x + 2 + query.chars().count() as u16;
    if curx < x + width {
        frame.set_cursor_position((curx, input_y));
    }
}

// ── Ask panel (LLM answer, grows upward from control bar) ─────────────────────

fn render_ask_panel(frame: &mut Frame, app: &App, pane_area: Rect, bar_area: Rect) {
    let width = bar_area.width.saturating_sub(2).max(10) as usize;
    let sand = rgb(app.tuning.theme_accent_bright);
    let mut lines: Vec<Line> = Vec::new();

    // The conversation transcript.
    for (role, text) in &app.agent_history {
        let (tag, tag_style) = if role == "user" {
            ("you  › ", Style::default().fg(sand).add_modifier(Modifier::BOLD))
        } else {
            ("mars › ", Style::default().fg(rgb(app.tuning.theme_accent)).add_modifier(Modifier::BOLD))
        };
        for (i, wrapped) in wrap_text(text, width.saturating_sub(7)).into_iter().enumerate() {
            let prefix = if i == 0 { tag } else { "       " };
            lines.push(Line::from(vec![
                Span::styled(prefix, tag_style),
                Span::styled(wrapped, Style::default().fg(Color::White)),
            ]));
        }
    }
    // The streamed reply-in-progress renders as a live assistant turn; the
    // final Answer replaces it (directive stripped, pushed into history).
    if let Some(partial) = app.agent_partial.as_ref().filter(|p| !p.is_empty()) {
        let tag_style =
            Style::default().fg(rgb(app.tuning.theme_accent)).add_modifier(Modifier::BOLD);
        for (i, wrapped) in wrap_text(partial, width.saturating_sub(7)).into_iter().enumerate() {
            let prefix = if i == 0 { "mars › " } else { "       " };
            lines.push(Line::from(vec![
                Span::styled(prefix, tag_style),
                Span::styled(wrapped, Style::default().fg(Color::White)),
            ]));
        }
    }
    if app.agent_pending {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let speed = app.tuning.spinner_speed_ticks;
        let sp = frames[(app.frame_tick / speed % frames.len() as u64) as usize];
        lines.push(Line::from(Span::styled(
            format!(" {} thinking…", sp),
            Style::default().fg(sand).add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(notice) = &app.agent_answer {
        for wrapped in wrap_text(notice, width) {
            lines.push(Line::from(Span::styled(
                wrapped,
                Style::default().fg(rgb(app.tuning.theme_accent_dark)),
            )));
        }
    }
    // A pending selection-refactor takes the confirm slot (Enter replaces the
    // selection — or inserts at the cursor when nothing was selected — reversibly).
    if app.refactor_replacement.is_some() {
        let n = app.refactor_replacement.as_deref().map(|c| c.lines().count()).unwrap_or(0);
        let verb = match app.refactor_target {
            Some((_, s, e)) if s == e => "insert at the cursor",
            _ => "replace the selection",
        };
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            format!(" ▶ Enter to {verb} ({n} lines) · C-l cancel "),
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD),
        )));
    } else if let Some(d) = &app.agent_directive {
        let label = match d {
            crate::agent::AgentDirective::Run(name) => format!(" ▶ Enter to run: {name} "),
            crate::agent::AgentDirective::Type(cmd) => {
                format!(" ▶ Enter to type into terminal: {cmd} ")
            }
            crate::agent::AgentDirective::Open(loc) => format!(" ▶ Enter to open: {loc} "),
            crate::agent::AgentDirective::Need(_) => String::new(), // auto-satisfied, never shown
        };
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            label,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " Ask about what's on your screen — Enter sends · C-l new chat",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Adaptive height: grow to the content, cap at ask_panel_max_pct — the
    // chat hugs the bottom of the screen and never buries the workspace;
    // older turns are one scroll (Up / wheel) away, not more panel.
    let max_h = ((pane_area.height as u32 * app.tuning.ask_panel_max_pct as u32 / 100)
        as u16)
        .max(3);
    let content_h = lines.len() as u16 + 1; // +1 for top border
    let panel_h = content_h.clamp(2, max_h);
    let visible = panel_h.saturating_sub(1) as usize;

    // Scroll: pin to the bottom, offset by ask_scroll (lines up from the end).
    let total = lines.len();
    if total > visible {
        let max_scroll = total - visible;
        let scroll = app.ask_scroll.min(max_scroll);
        let start = max_scroll - scroll;
        let mut view: Vec<Line> = lines.drain(start..start + visible).collect();
        if scroll > 0 {
            // Replace the last line with a "more below" marker.
            if let Some(last) = view.last_mut() {
                *last = Line::from(Span::styled(
                    format!(" ↓ {} more (Down to scroll) ", scroll),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
        if start > 0 {
            if let Some(first) = view.first_mut() {
                *first = Line::from(Span::styled(
                    format!(" ↑ {} more (Up to scroll) ", start),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
        lines = view;
    }

    let panel_y = bar_area.y.saturating_sub(panel_h);
    let rect = Rect {
        x: bar_area.x,
        y: panel_y,
        width: bar_area.width,
        height: panel_h,
    };

    frame.render_widget(Clear, rect);
    let provider = crate::agent::AgentConfig::from_env().provider;
    let title = if provider == "none" {
        " ✦ ask ".to_string()
    } else {
        format!(" ✦ ask · {} ", provider)
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(rgb(app.tuning.theme_accent))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .border_style(Style::default().fg(rgb(app.tuning.theme_accent)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

/// Word-wrap `text` to `width` columns, preserving explicit newlines.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        if para.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            if line.is_empty() {
                line = word.to_string();
            } else if line.chars().count() + 1 + word.chars().count() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}
