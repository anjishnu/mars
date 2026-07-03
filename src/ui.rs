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
    render_status(frame, app, status_area);
    render_control_bar(frame, app, bar_area);

    // Bar dropdown / ask-panel drawn last so it sits on top (grows upward).
    if app.palette.is_some() && matches!(app.mode, Mode::Bar) {
        match app.palette.as_ref().map(|p| p.bar_mode.clone()) {
            Some(BarMode::Ask)     => render_ask_panel(frame, app, pane_area, bar_area),
            Some(BarMode::Command) => render_bar_dropdown(frame, app, pane_area, bar_area),
            // Shell: an inline composer anchored at the cursor (no eye-jump).
            Some(BarMode::Shell)   => render_shell_overlay(frame, app, pane_area),
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
        ("t / n",   "new tab"),
        ("r",       "rename tab"),
        ("h l ← →", "switch tab"),
        ("1-9",     "jump to tab"),
        ("H L",     "move tab"),
        ("d",       "close tab"),
        ("",        ""),
        ("o / Tab", "next pane"),
        ("|",       "split right"),
        ("-",       "split below"),
        ("z",       "zoom pane (toggle)"),
        ("< >",     "resize pane"),
        ("x",       "move pane"),
        ("q / 0",   "close pane"),
        ("",        ""),
        ("?",       "why did this fail? (triage)"),
        ("D",       "detach session (keeps running)"),
        ("Esc ⏎",   "done  ·  creation exits, navigation stays"),
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
            " C-t · travel ",
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

fn render_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    for (i, tab) in app.tabs.iter().enumerate() {
        let buf_name = {
            let pane = app.panes.get(&tab.focused_pane);
            pane.and_then(|p| {
                if let PaneContent::Editor(buf_id) = p.content {
                    app.buffers.get(&buf_id).map(|b| b.name.as_str())
                } else {
                    Some("terminal")
                }
            })
            .unwrap_or(&tab.name)
        };
        let label = format!(" {} {} ", tab.name, buf_name);
        if i == app.active_tab {
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(rgb(app.tuning.theme_chip_fg))
                    .bg(rgb(app.tuning.theme_accent))
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                label,
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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

    // Day-0 splash: MARS banner in the untouched scratch, until the first key.
    if app.show_splash && buf.rope.len_chars() == 0 {
        render_splash(frame, app, inner);
        return None;
    }

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
            if focused {
                for &(hr, hc, hlen) in &app.search_hl {
                    if hr == row {
                        let end = (hc + hlen).min(chars.len());
                        for h in hl.iter_mut().take(end).skip(hc.min(chars.len())) { *h = 2; }
                    }
                }
            }

            if hl.iter().all(|&h| h == 0) {
                spans.push(Span::raw(content));
            } else {
                let mut i = 0;
                while i < chars.len() {
                    let kind = hl[i];
                    let mut j = i;
                    while j < chars.len() && hl[j] == kind { j += 1; }
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

    let hint = |key: &str, what: &str| {
        Line::from(vec![
            Span::styled(
                format!("{key}  "),
                Style::default().fg(rgb(t.theme_accent_bright)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(what.to_string(), Style::default().fg(Color::DarkGray)),
        ])
        .centered()
    };
    lines.push(hint("C-Spc", "search every command"));
    lines.push(hint("!", "run a shell command  ·  ? ask the agent"));
    lines.push(hint("C-t", "travel: tabs, panes, splits"));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "just start typing",
        Style::default().fg(Color::DarkGray),
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
            (Action::FindFile, "open"),
            (Action::Search, "search"),
        ] {
            if let Some(b) = app.keys.binding_for(&action) {
                v.push((b, label.to_string()));
            }
        }
        v.push(("C-g".to_string(), "cancel".to_string()));
        v
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
            frame.render_widget(
                Paragraph::new(Span::styled(prompt.clone(), style)),
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
                let text = format!("{}{}", p.label, p.input);
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        text.clone(),
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                    )),
                    area,
                );
                let cx = area.x + text.chars().count() as u16;
                if cx < area.x + area.width {
                    frame.set_cursor_position((cx, area.y));
                }
            }
        }
        _ => {
            // Idle hint — derived from the live keymap, never hardcoded.
            let open = app.keys.binding_for(&Action::FindFile).unwrap_or_default();
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

fn render_bar_dropdown(frame: &mut Frame, app: &App, pane_area: Rect, bar_area: Rect) {
    let palette = match app.palette.as_ref() {
        Some(p) => p,
        None => return,
    };

    let items = palette.visible_items(&app.frecency);
    if items.is_empty() {
        return;
    }

    let max_height = ((pane_area.height * app.tuning.panel_max_height_pct / 100) as usize)
        .min(app.tuning.dropdown_max_rows as usize) as u16;
    let drop_h = (items.len() as u16 + 1).min(max_height); // +1 for potential title line
    let drop_w = bar_area.width;

    // Position: just above the control bar
    let drop_y = bar_area.y.saturating_sub(drop_h);
    let drop_rect = Rect {
        x: bar_area.x,
        y: drop_y,
        width: drop_w,
        height: drop_h,
    };

    frame.render_widget(Clear, drop_rect);

    let block = Block::default()
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .border_style(Style::default().fg(rgb(app.tuning.theme_accent)));

    let inner = block.inner(drop_rect);
    frame.render_widget(block, drop_rect);

    let max_show = inner.height as usize;

    // Scroll offset so the selected item is visible
    let scroll = if palette.selected >= max_show {
        palette.selected + 1 - max_show
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    for (idx, row) in items.iter().enumerate().skip(scroll).take(max_show) {
        let selected = idx == palette.selected;
        let item_bg  = if selected { Color::DarkGray } else { Color::Reset };
        let has_sub  = matches!(row.kind, ItemKind::Submenu(_));

        // The row's REAL keybinding, looked up live — the passive teacher
        // (§5.3: show the key on every menu row).
        let binding = match &row.kind {
            ItemKind::Run(a) => app.keys.binding_for(a).unwrap_or_default(),
            ItemKind::Submenu(_) => String::new(),
        };

        let desc = if row.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", row.description)
        };
        let type_mark = if has_sub { " ▸" } else { "" };

        let line = Line::from(vec![
            Span::styled(
                format!(" {:<w$}", binding, w = app.tuning.binding_badge_width),
                Style::default()
                    .fg(rgb(app.tuning.theme_accent_bright))
                    .bg(item_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().bg(item_bg)),
            Span::styled(
                format!("{}{}", row.label, type_mark),
                if selected {
                    Style::default()
                        .fg(rgb(app.tuning.theme_accent))
                        .bg(item_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White).bg(item_bg)
                },
            ),
            Span::styled(
                desc,
                Style::default().fg(Color::DarkGray).bg(item_bg),
            ),
        ]);
        lines.push(line);
    }

    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
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
        format!(" {root_name}/ ")
    } else {
        format!(" ⌕ {filter} ")
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

    let mut lines: Vec<Line> = Vec::new();
    for (idx, row) in app.tree_rows.iter().enumerate().skip(scroll).take(max_show) {
        let is_sel = idx == selected;
        let bg = if is_sel && focused { Color::DarkGray } else { Color::Reset };
        let indent = "  ".repeat(row.depth);
        // Folders: a disclosure caret + bold, accent color. Files: plain.
        let (glyph, label_style) = if row.updir {
            ("  ", Style::default().fg(Color::DarkGray).bg(bg))
        } else if row.is_dir {
            (
                if row.expanded { "▾ " } else { "▸ " },
                Style::default().fg(accent).bg(bg).add_modifier(Modifier::BOLD),
            )
        } else {
            ("  ", Style::default().fg(Color::White).bg(bg))
        };
        let label = if row.is_dir && !row.updir {
            format!("{}/", row.label)
        } else {
            row.label.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{indent}{glyph}"), Style::default().fg(accent).bg(bg)),
            Span::styled(label, label_style),
        ]));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

// ── Shell-translate overlay (W3, anchored at the cursor — no eye-jump) ─────────

fn render_shell_overlay(frame: &mut Frame, app: &App, pane_area: Rect) {
    let query = app.palette.as_ref().map(|p| p.query.as_str()).unwrap_or("");
    let chipfg = rgb(app.tuning.theme_chip_fg);
    let accent = rgb(app.tuning.theme_accent);

    // The input line begins EXACTLY where the cursor was (no label prefix), so
    // it reads as typing in place. A tiny `!` chip sits just left of it.
    let input = format!("! {query} ");
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
    } else {
        " type English, Enter translates → command · Ctrl+Space = command bar".to_string()
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
    if let Some(d) = &app.agent_directive {
        let label = match d {
            crate::agent::AgentDirective::Run(name) => format!(" ▶ Enter to run: {name} "),
            crate::agent::AgentDirective::Type(cmd) => {
                format!(" ▶ Enter to type into terminal: {cmd} ")
            }
            crate::agent::AgentDirective::Open(loc) => format!(" ▶ Enter to open: {loc} "),
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

    // Adaptive height: grow to the content, cap at panel_max_height_pct.
    let max_h = ((pane_area.height as u32 * app.tuning.panel_max_height_pct as u32 / 100)
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
