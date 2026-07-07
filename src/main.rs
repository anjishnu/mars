mod agent;
mod app;
mod banner;
mod broker;
mod buffer;
mod config;
mod layout;
mod mode;
mod palette;
mod pane;
mod project;
mod session;
mod tab;
mod terminal;
mod tuning;
mod ui;

use anyhow::Result;
use app::{App, InputEvent};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{env, io};

const HELP: &str = "\
mars — mission control for your terminal

USAGE
  mars                           new session, opens a terminal (like tmux)
  mars FILE                      new session, editing FILE
  mars -s [FILE]                 quick standalone edit, no daemon (scripts)
  mars <COMMAND> [ARGS]

  By default every `mars` is a session: detachable, survives disconnects,
  auto-named (a number first, then an AI label). See below to manage them.

SESSIONS  (work survives closed windows and disconnects)
  mars new <name> [FILE]         start an explicitly-named session
                                 (aliases: session, --session)
  mars attach [name]             reattach — most recent session if unnamed
                                 (aliases: a, resume, --resume)
  mars ls                        list sessions and their attach state
                                 (aliases: list, --list)
  mars rename <old> <new>        rename a running session
  mars kill <name>               end a session (autosaves first)

  Inside a session:  C-t D or C-x C-d  detaches (keeps everything running)
                     C-x C-c  quits and ends the session
  Closing the terminal window just detaches — nothing is lost.
  Reattach greets you with a \"while away\" line if anything happened;
  C-x g opens the full Away Digest (timeline + durations).

AGENT
  mars ask \"<question>\"          one-shot answer from the LLM agent
                                 (needs GEMINI_API_KEY, GROQ_API_KEY,
                                  or MARS_LLM_KEY + MARS_LLM_URL)

REMOTE  (the agent works on every box — the key never leaves home)
  mars keyd                      run once on your machine: holds the key,
                                 serves it over a forwarded socket
  mars ssh <host> [ssh args]     ssh in with the auth socket forwarded;
                                 the remote agent asks home, no key on the box

INSIDE THE EDITOR
  Ctrl+Space   search every command        !   run a shell command
  ?            ask the agent               C-t tabs / panes / splits
  @ or C-x d   file tree (browse/filter)   C-x C-s save   C-g cancel anything

MORE
  mars help                      this text          (aliases: -h, --help)
  mars version                   version            (aliases: -V, --version)
  mars reset                     restore default keybindings + tuning (backs up old)
  mars --selfcheck               run the built-in test suite

  Config: ~/.config/mars/keys.json (bindings), tuning.json (behavior knobs)
  Session logs: ~/.local/state/mars/<name>.log";

fn main() -> Result<()> {
    // A previously killed client may have left this TTY in raw mode — repair
    // it before printing anything (and before crossterm snapshots "original").
    session::sanitize_tty();

    let mut args = env::args().skip(1);
    let first = args.next();

    match first.as_deref() {
        Some("help") | Some("--help") | Some("-h") => {
            println!("{HELP}");
            return Ok(());
        }
        Some("version") | Some("--version") | Some("-V") => {
            banner::print_banner();
            println!("\n  mars {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        // Headless self-check (no TTY needed) — render, bar, PTY, and sessions.
        Some("--selfcheck") => return selfcheck(),
        // Headless ask — verify the agent provider end-to-end from the shell.
        Some("ask") | Some("--ask") => {
            let question: String = args.collect::<Vec<_>>().join(" ");
            return ask_cli(question);
        }
        // Session daemon (internal — spawned by `new`).
        Some("--server") => {
            let name = args.next().ok_or_else(|| anyhow::anyhow!("--server needs a name"))?;
            // stderr is the session log file (set up by `mars new`).
            eprintln!("[mars] session '{name}' starting (pid {})", std::process::id());
            let result = session::server_main(&name, args.next());
            match &result {
                Ok(_) => eprintln!("[mars] session '{name}' ended cleanly"),
                Err(e) => eprintln!("[mars] session '{name}' died: {e}"),
            }
            return result;
        }
        // Create-or-attach a named session.
        Some("new") | Some("session") | Some("--session") => {
            let name = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: mars new <name> [file]"))?;
            return session::session_main(&name, args.next());
        }
        // Reattach: named, or the most recently active session.
        Some("attach") | Some("a") | Some("resume") | Some("--resume") => {
            return session::resume_main(args.next());
        }
        Some("ls") | Some("list") | Some("--list") => {
            let interactive = !args.any(|a| a == "--no-prompt");
            return session::list_main(interactive);
        }
        // The key-never-leaves-home broker: run once on your machine.
        Some("keyd") => return broker::keyd_main(),
        // SSH to a host with the auth socket forwarded — the agent works there
        // with no key on the box.
        Some("ssh") => {
            let host = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: mars ssh <host> [ssh args…]   (needs: mars keyd)"))?;
            return broker::ssh_main(host, args.collect());
        }
        Some("kill") | Some("--kill") => {
            let name = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: mars kill <name>   (see: mars ls)"))?;
            return session::kill_main(&name);
        }
        Some("rename") | Some("--rename") => {
            let (old, new) = (args.next(), args.next());
            let (Some(old), Some(new)) = (old, new) else {
                anyhow::bail!("usage: mars rename <old> <new>   (see: mars ls)");
            };
            return session::rename_main(&old, &new);
        }
        // Factory reset: restore default keybindings + tuning (backs up the old files).
        Some("reset") | Some("--reset") => {
            let path = config::reset_keys()?;
            tuning::reset();
            println!(
                "Restored default keybindings and tuning.\n  {}\n  (your previous files were \
                 backed up alongside as *.bak)",
                path.display()
            );
            return Ok(());
        }
        // Quick standalone edit — no daemon (scripts, throwaway).
        Some("-s") | Some("--standalone") => {} // handled below
        // Unknown flags are errors, not filenames.
        Some(s) if s.starts_with('-') => {
            eprintln!("unknown option: {s}\n");
            eprintln!("{HELP}");
            std::process::exit(2);
        }
        _ => {}
    }

    // Default: sessions-by-default (like tmux). `mars [file]` spins up an
    // auto-numbered session (terminal if no file, editor if a file); the AI
    // renames it later. `-s`/`--standalone` opts out for a quick no-daemon edit.
    let standalone = matches!(first.as_deref(), Some("-s") | Some("--standalone"));
    let file = if standalone { args.next() } else { first };
    if !standalone {
        let name = session::next_auto_name()?;
        return session::session_main(&name, file);
    }

    // ── Standalone mode (no daemon; `mars -s [file]`) ────────────────────────
    session::install_panic_restore();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;

    // Kitty keyboard protocol where supported (kitty/WezTerm/Ghostty/iTerm2 3.5+):
    // unlocks chords legacy encoding can't express (C-{, C-}, C--, C-|).
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // TTY-reader thread → the source-agnostic input channel.
    let (tx, rx) = std::sync::mpsc::channel::<InputEvent>();
    std::thread::spawn(move || loop {
        match crossterm::event::read() {
            Ok(Event::Key(k)) => { if tx.send(InputEvent::Key(k)).is_err() { break; } }
            Ok(Event::Mouse(m)) => { if tx.send(InputEvent::Mouse(m)).is_err() { break; } }
            Ok(Event::Paste(s)) => { if tx.send(InputEvent::Paste(s)).is_err() { break; } }
            Ok(_) => {} // Resize handled by ratatui autoresize on the real TTY
            Err(_) => break,
        }
    });

    let had_file = file.is_some();
    let mut app = App::new(file)?;
    if !had_file {
        app.open_terminal(); // no file → open a shell, not a scratch buffer
    }
    let result = app.run(&mut terminal, &rx);

    disable_raw_mode()?;
    if enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    result
}

/// Headless one-shot question through the real agent path (provider detection,
/// registry context, RUN: parsing) — the live verification `--selfcheck` can't do.
fn ask_cli(question: String) -> Result<()> {
    if question.trim().is_empty() {
        anyhow::bail!("usage: mars --ask \"<question>\"");
    }
    let cfg = agent::AgentConfig::from_env();
    if !cfg.is_configured() {
        anyhow::bail!("no API key: export GROQ_API_KEY, GEMINI_API_KEY, or MARS_LLM_KEY");
    }
    println!("provider: {}   model: {}", cfg.provider, cfg.model);
    let (tx, rx) = std::sync::mpsc::channel();
    agent::ask(
        cfg,
        question,
        palette::registry_context(),
        String::new(), // no live screen in headless mode
        Vec::new(),
        tx,
    );
    match rx.recv_timeout(std::time::Duration::from_secs(60))? {
        agent::AgentEvent::Answer { text, directive } => {
            println!("{}", text);
            match directive {
                Some(agent::AgentDirective::Run(name)) => println!("[would run: {name}]"),
                Some(agent::AgentDirective::Type(cmd)) => {
                    println!("[would type into terminal: {cmd}]")
                }
                Some(agent::AgentDirective::Open(loc)) => println!("[would open: {loc}]"),
                Some(agent::AgentDirective::Need(_)) => {}
                None => {}
            }
            Ok(())
        }
        agent::AgentEvent::AutoName { .. }
        | agent::AgentEvent::SessionName { .. }
        | agent::AgentEvent::WatchSummary { .. }
        | agent::AgentEvent::BgDone
        | agent::AgentEvent::ShellTranslation { .. } => Ok(()),
        agent::AgentEvent::Error(e) => anyhow::bail!("agent error: {}", e),
    }
}

/// Headless verification of the core paths, runnable without a real terminal.
fn selfcheck() -> Result<()> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    // Hermetic: an inherited agent key would flip no-key code paths (e.g. the
    // shell composer translates instead of running). Clear them so the suite is
    // deterministic regardless of the caller's environment.
    for key in [
        "GEMINI_API_KEY", "GOOGLE_API_KEY", "GROQ_API_KEY",
        "MARS_LLM_KEY", "MARS_LLM_URL", "ARES_LLM_KEY", "ARES_LLM_URL",
    ] {
        std::env::remove_var(key);
    }

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn kc(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }
    fn typ(app: &mut App, s: &str) -> Result<()> {
        for c in s.chars() { app.handle_key(k(KeyCode::Char(c)))?; }
        Ok(())
    }
    fn screen_text(term: &Terminal<TestBackend>) -> String {
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    // Never touch the user's real clipboard from tests (also makes the
    // C-c → C-v round-trip deterministic via the kill-ring fallback).
    std::env::set_var("MARS_NO_SYSTEM_CLIPBOARD", "1");
    // Isolate config: fresh defaults in a temp dir, immune to the user's real
    // remaps/tuning — and this exercises the default-file writers.
    let cfg_dir = std::env::temp_dir().join(format!("mars-selfcheck-{}", std::process::id()));
    std::fs::create_dir_all(&cfg_dir)?;
    std::env::set_var("XDG_CONFIG_HOME", &cfg_dir);

    let mut app = App::new(None)?;
    let mut term = Terminal::new(TestBackend::new(120, 40))?;

    // 1. Renders, starts non-modal (EDIT), and shows the MARS splash.
    term.draw(|f| ui::render(f, &mut app))?;
    let t1 = screen_text(&term);
    assert!(t1.contains("EDIT"), "status bar missing EDIT mode");
    assert!(t1.contains("scratch"), "scratch buffer not shown");
    assert!(t1.contains("control for your terminal"), "MARS splash missing on first render");
    println!("[selfcheck] render + splash ............ PASS");

    // 2. Typing dismisses the splash and inserts text (no command side-effects).
    typ(&mut app, "hello world")?;
    term.draw(|f| ui::render(f, &mut app))?;
    let t2 = screen_text(&term);
    assert!(t2.contains("hello world"), "typing did not insert");
    assert!(!t2.contains("control for your terminal"), "splash did not dismiss on keypress");
    assert!(app.mode == mode::Mode::Edit, "typing changed mode");
    println!("[selfcheck] non-modal insert ........... PASS");

    // 2b. Idle-render gating (the SSH no-op-flush fix): after a draw, an idle
    //     tick must NOT request a redraw; a background agent event and an active
    //     spinner MUST. Zero idle flushes = a quiet link.
    {
        let mut app = App::new(None)?;
        app.needs_redraw = false; // pretend we just drew
        app.tick();
        assert!(!app.needs_redraw, "idle tick asked for a redraw (would flush over SSH every tick)");
        app.agent_tx.send(agent::AgentEvent::BgDone)?; // a background event landed
        app.tick();
        assert!(app.needs_redraw, "an agent event did not request a redraw");
        app.needs_redraw = false;
        app.agent_pending = true; // spinner animates → redraw each tick
        app.tick();
        assert!(app.needs_redraw, "the thinking spinner did not request a redraw");
    }
    println!("[selfcheck] idle render gating ........ PASS");

    // 3. Kill-ring round-trip: C-k kills the line, C-y yanks it back.
    app.handle_key(k(KeyCode::Enter))?;
    typ(&mut app, "KILLME")?;
    app.handle_key(kc(KeyCode::Char('a')))?; // C-a: line start
    app.handle_key(kc(KeyCode::Char('k')))?; // C-k: kill line
    assert_eq!(app.kill_ring.last().map(String::as_str), Some("KILLME"), "kill-ring empty");
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(!screen_text(&term).contains("KILLME"), "C-k did not remove text");
    app.handle_key(kc(KeyCode::Char('y')))?; // C-y: yank
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("KILLME"), "C-y did not restore text");
    println!("[selfcheck] kill/yank (C-k/C-y) ........ PASS");

    // 4. Chord normalization: ALT|SHIFT+'<' (what terminals send for M-<)
    //    must equal the parsed "M-<" binding; C-_ must undo (C-/ alias).
    let raw = KeyEvent::new(KeyCode::Char('<'), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(
        config::chord_of(&raw),
        config::parse_key("M-<").unwrap(),
        "chord_of failed to normalize ALT|SHIFT+'<'"
    );
    typ(&mut app, "abc")?;
    app.handle_key(kc(KeyCode::Char('_')))?; // C-_ → Undo
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(!screen_text(&term).contains("abc"), "C-_ did not undo");
    println!("[selfcheck] chord normalize + C-_ ...... PASS");

    // 5. M-< (as the real ALT|SHIFT event) goes to top; Shift+Right selects;
    //    typing REPLACES the selection (Mac contract).
    app.handle_key(raw)?; // M-< → GoTop
    assert_eq!((app.focused_pane().cursor_row, app.focused_pane().cursor_col), (0, 0));
    for _ in 0..5 {
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT))?;
    }
    typ(&mut app, "X")?;
    term.draw(|f| ui::render(f, &mut app))?;
    let t5 = screen_text(&term);
    assert!(t5.contains("X world"), "typing did not replace the selection");
    assert!(!t5.contains("hello world"), "selection text survived replacement");
    println!("[selfcheck] select + replace-on-type ... PASS");

    // 6. Live isearch: typing jumps to the match, C-g restores the origin.
    let origin = (app.focused_pane().cursor_row, app.focused_pane().cursor_col);
    app.handle_key(kc(KeyCode::Char('s')))?; // C-s → isearch
    assert!(app.mode == mode::Mode::Prompt, "C-s did not open isearch");
    typ(&mut app, "world")?;
    assert_eq!(
        (app.focused_pane().cursor_row, app.focused_pane().cursor_col),
        (0, 2),
        "isearch did not jump to the live match"
    );
    assert!(!app.search_hl.is_empty(), "isearch matches not highlighted");
    app.handle_key(kc(KeyCode::Char('g')))?; // C-g → cancel, restore origin
    assert_eq!(
        (app.focused_pane().cursor_row, app.focused_pane().cursor_col),
        origin,
        "C-g did not restore the origin"
    );
    assert!(app.search_hl.is_empty(), "highlights not cleared after cancel");
    println!("[selfcheck] live isearch (C-s/C-g) ..... PASS");

    // 6a. Search-as-teleport: match counter, Tab-label jump, land-on-any-key.
    {
        let mut app = App::new(None)?;
        typ(&mut app, "aa bb aa cc aa")?; // three "aa" matches at cols 0, 6, 12
        app.handle_key(kc(KeyCode::Char('a')))?; // C-a → line start
        app.handle_key(kc(KeyCode::Char('s')))?; // C-s → isearch
        typ(&mut app, "aa")?;
        assert_eq!(app.isearch_status(), Some((1, 3)), "counter should read 1/3 at first match");
        // Tab labels the matches; the 2nd label ('s') jumps to the 2nd match (col 6).
        app.handle_key(k(KeyCode::Tab))?;
        assert!(app.search_pick && app.search_labels.len() == 3, "Tab did not label matches");
        assert_eq!(app.search_labels[1].2, 's', "second label should be 's'");
        app.handle_key(k(KeyCode::Char('s')))?; // pick label 's' → 2nd match
        assert_eq!(app.focused_pane().cursor_col, 6, "label jump did not land on the 2nd match");
        assert!(matches!(app.mode, mode::Mode::Edit), "label pick did not accept the search");

        // Land-on-any-key: type target, then a motion key commits + applies.
        app.handle_key(kc(KeyCode::Char('s')))?; // isearch again
        typ(&mut app, "cc")?; // jumps to "cc" (col 9)
        assert_eq!(app.focused_pane().cursor_col, 9, "isearch did not land on cc");
        app.handle_key(kc(KeyCode::Char('a')))?; // C-a while searching → commit + line-start
        assert!(matches!(app.mode, mode::Mode::Edit), "land-on-key did not exit search");
        assert_eq!(app.focused_pane().cursor_col, 0, "C-a did not apply after committing search");
    }
    println!("[selfcheck] search-as-teleport ......... PASS");

    // 6c. Selection-aware refactor: code-block extraction + reversible apply.
    {
        assert_eq!(
            app::extract_code_block("here you go:\n```rust\nfn a() {}\n```\ndone"),
            Some("fn a() {}".to_string()),
            "code block not extracted"
        );
        assert_eq!(app::extract_code_block("no fences here"), None);

        let mut app = App::new(None)?;
        typ(&mut app, "old_code")?;
        app.handle_key(kc(KeyCode::Char('a')))?; // C-a → line start
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT))?; // select the line
        assert!(app.selection_range().is_some(), "selection not set");
        // Simulate an accepted refactor and apply it as one reversible edit.
        app.refactor_target = app.selection_range();
        app.refactor_replacement = Some("new_code".to_string());
        app.apply_refactor();
        term.draw(|f| ui::render(f, &mut app))?;
        let t = screen_text(&term);
        assert!(t.contains("new_code") && !t.contains("old_code"), "refactor not applied");
        app.handle_key(kc(KeyCode::Char('_')))?; // C-_ → Undo (one chunk)
        term.draw(|f| ui::render(f, &mut app))?;
        assert!(screen_text(&term).contains("old_code"), "one undo did not revert the refactor");
    }
    println!("[selfcheck] selection refactor (undo) .. PASS");

    // 6b. Fast motion: ⌘-token stops (code-aware), matching-bracket, symbol jump.
    {
        let mut app = App::new(None)?;
        typ(&mut app, "foo.bar(baz)")?;
        app.handle_key(kc(KeyCode::Char('a')))?; // C-a → line start (col 0)
        let col = |a: &App| a.focused_pane().cursor_col;
        app.move_token_forward(); assert_eq!(col(&app), 3, "token→ should stop at '.'");
        app.move_token_forward(); assert_eq!(col(&app), 4, "token→ should stop at 'bar'");
        app.move_token_forward(); assert_eq!(col(&app), 7, "token→ should stop at '('");
        app.move_token_backward(); assert_eq!(col(&app), 4, "token← should return to 'bar'");
        app.move_token_forward(); // back onto '(' at col 7
        app.match_bracket(); assert_eq!(col(&app), 11, "match_bracket → ')'");
        app.match_bracket(); assert_eq!(col(&app), 7, "match_bracket → '('");

        let mut app = App::new(None)?;
        typ(&mut app, "fn one() {")?; app.handle_key(k(KeyCode::Enter))?;
        typ(&mut app, "    body")?;   app.handle_key(k(KeyCode::Enter))?;
        typ(&mut app, "fn two() {")?; // cursor on row 2
        app.jump_symbol(false); assert_eq!(app.focused_pane().cursor_row, 0, "symbol← to fn one");
        app.jump_symbol(true);  assert_eq!(app.focused_pane().cursor_row, 2, "symbol→ to fn two");
    }
    println!("[selfcheck] token/bracket/symbol jumps . PASS");

    // 6d. Undo MVP: a run of typed chars is ONE undo step (typing was previously
    //     invisible to undo); a motion breaks the run into separate steps.
    {
        let mut app = App::new(None)?;
        typ(&mut app, "hello")?;
        term.draw(|f| ui::render(f, &mut app))?;
        assert!(screen_text(&term).contains("hello"), "typing did not appear");
        app.handle_key(kc(KeyCode::Char('_')))?; // C-_ → Undo (whole run)
        term.draw(|f| ui::render(f, &mut app))?;
        assert!(!screen_text(&term).contains("hello"), "undo did not reverse a typed run");

        let mut app = App::new(None)?;
        typ(&mut app, "AAA")?;
        app.handle_key(kc(KeyCode::Char('a')))?; // C-a (line start) breaks the run
        typ(&mut app, "BBB")?; // inserts before AAA → "BBBAAA", a second run
        app.handle_key(kc(KeyCode::Char('_')))?; // undo removes only the BBB run
        term.draw(|f| ui::render(f, &mut app))?;
        let t = screen_text(&term);
        assert!(t.contains("AAA") && !t.contains("BBB"), "undo did not stop at the run boundary");
    }
    println!("[selfcheck] undo (coalesced runs) ..... PASS");

    // 6d2. Undo time-travel mode: enter, ← steps back, Home undoes all, End
    //      redoes all, Esc exits.
    {
        let mut app = App::new(None)?;
        typ(&mut app, "one")?;
        app.handle_key(kc(KeyCode::Char('a')))?; // C-a breaks the run
        typ(&mut app, "two")?; // two undo steps
        let text = |a: &App| -> String {
            match a.focused_pane().content {
                pane::PaneContent::Editor(id) => a.buffers[&id].rope.to_string(),
                _ => String::new(),
            }
        };
        assert!(text(&app).contains("one"), "setup: text not present before undo");
        app.run_action(palette::Action::UndoMode);
        assert!(matches!(app.mode, mode::Mode::Undo), "UndoMode did not enter undo mode");
        app.handle_key(k(KeyCode::Home))?; // undo everything
        assert!(text(&app).trim().is_empty(), "Home did not undo to the start: {:?}", text(&app));
        app.handle_key(k(KeyCode::End))?; // redo everything
        assert!(text(&app).contains("one"), "End did not redo forward");
        app.handle_key(k(KeyCode::Esc))?;
        assert!(matches!(app.mode, mode::Mode::Edit), "Esc did not exit undo mode");
    }
    println!("[selfcheck] undo time-travel mode ..... PASS");

    // 6e. Horizontal motion wraps across lines: → at line end → next line start;
    //     ← at line start → previous line end.
    {
        let mut app = App::new(None)?;
        typ(&mut app, "ab")?;
        app.handle_key(k(KeyCode::Enter))?;
        typ(&mut app, "cd")?; // buffer "ab\ncd"
        app.handle_key(k(KeyCode::Up))?;   // row 0
        app.handle_key(k(KeyCode::End))?;  // (0, 2) end of "ab"
        let pos = |a: &App| (a.focused_pane().cursor_row, a.focused_pane().cursor_col);
        assert_eq!(pos(&app), (0, 2), "End did not reach line end");
        app.handle_key(k(KeyCode::Right))?;
        assert_eq!(pos(&app), (1, 0), "→ at line end did not wrap to the next line");
        app.handle_key(k(KeyCode::Left))?;
        assert_eq!(pos(&app), (0, 2), "← at line start did not wrap to the previous line");
    }
    println!("[selfcheck] cross-line arrow wrap ..... PASS");

    // 6f. Auto-indent: Enter carries the previous line's leading whitespace.
    {
        let mut app = App::new(None)?;
        typ(&mut app, "    x")?; // 4-space indent
        app.handle_key(k(KeyCode::Enter))?;
        assert_eq!(app.focused_pane().cursor_col, 4, "Enter did not auto-indent to match");
    }
    println!("[selfcheck] auto-indent on newline .... PASS");

    let text_of = |a: &App| -> String {
        match a.focused_pane().content {
            pane::PaneContent::Editor(id) => a.buffers[&id].rope.to_string(),
            _ => String::new(),
        }
    };

    // 6g. Tab indents / Shift-Tab dedents the selected lines (one undo step).
    {
        let mut app = App::new(None)?;
        typ(&mut app, "aa")?; app.handle_key(k(KeyCode::Enter))?; typ(&mut app, "bb")?;
        app.handle_key(kc(KeyCode::Char('x')))?; app.handle_key(k(KeyCode::Char('h')))?; // C-x h select all
        app.handle_key(k(KeyCode::Tab))?;
        assert_eq!(text_of(&app), "    aa\n    bb", "Tab did not indent the block");
        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))?;
        assert_eq!(text_of(&app), "aa\nbb", "Shift-Tab did not dedent the block");
    }
    println!("[selfcheck] indent/dedent selection ... PASS");

    // 6h. Query-replace: from/to prompts, then y replaces one, ! replaces the rest.
    {
        let mut app = App::new(None)?;
        typ(&mut app, "foo bar foo")?;
        app.run_action(palette::Action::QueryReplace);
        assert!(app.mode == mode::Mode::Prompt, "query-replace did not prompt");
        typ(&mut app, "foo")?; app.handle_key(k(KeyCode::Enter))?; // from
        typ(&mut app, "XYZ")?; app.handle_key(k(KeyCode::Enter))?; // to → begins stepping
        assert!(app.mode == mode::Mode::Prompt, "query-replace stepping prompt missing");
        app.handle_key(k(KeyCode::Char('y')))?; // replace first
        app.handle_key(k(KeyCode::Char('!')))?; // replace the rest
        assert_eq!(text_of(&app), "XYZ bar XYZ", "query-replace did not replace all matches");
        assert!(app.mode == mode::Mode::Edit, "query-replace did not finish");
    }
    println!("[selfcheck] query-replace (y/n/!/q) ... PASS");

    // 7. which-key: a pending prefix pops the continuation panel after a beat;
    //    C-x C-s on a pathless buffer opens Save-As (no ghost `:w` advice).
    app.handle_key(kc(KeyCode::Char('x')))?;
    assert_eq!(app.pending_prefix.len(), 1, "C-x did not arm a prefix");
    app.frame_tick += 30; // simulate the hesitation
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("C-x -"), "which-key panel missing");
    app.handle_key(kc(KeyCode::Char('s')))?; // completes C-x C-s → Save
    assert!(app.mode == mode::Mode::Prompt, "pathless save did not prompt Save-As");
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("Save as:"), "Save-As prompt not shown");
    app.handle_key(kc(KeyCode::Char('g')))?; // cancel
    println!("[selfcheck] which-key + Save-As ........ PASS");

    // 8. Dirty-quit guard: C-x C-c with modified buffers must confirm.
    app.handle_key(kc(KeyCode::Char('x')))?;
    app.handle_key(kc(KeyCode::Char('c')))?;
    assert!(!app.should_quit, "quit discarded unsaved work without asking");
    assert!(app.mode == mode::Mode::Prompt, "no quit confirmation prompt");
    app.handle_key(k(KeyCode::Char('q')))?; // quit anyway
    assert!(app.should_quit, "confirmed quit did not quit");
    app.should_quit = false;
    println!("[selfcheck] dirty-quit guard ........... PASS");

    // ── Fresh app: command bar surfaces ──────────────────────────────────────
    let mut app = App::new(None)?;

    // 9. Ctrl+Space opens the bar; fuzzy finds Split; the row shows the REAL
    //    binding (C-x 2), not a dead hotkey badge.
    app.handle_key(kc(KeyCode::Char(' ')))?;
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("CMD"), "Ctrl+Space did not open the bar");
    typ(&mut app, "split")?;
    term.draw(|f| ui::render(f, &mut app))?;
    let t9 = screen_text(&term);
    assert!(t9.contains("Split"), "fuzzy search lost Split");
    // Shortest binding wins the badge: SplitHorizontal → "C--" (not "C-x 2").
    assert!(t9.contains("C--"), "dropdown row missing its live keybinding");
    app.handle_key(k(KeyCode::Esc))?;
    println!("[selfcheck] bar fuzzy + live binding ... PASS");

    // 10. Graduation nudge: the 3rd bar-run of an action hints its binding.
    for _ in 0..3 {
        app.handle_key(kc(KeyCode::Char(' ')))?;
        typ(&mut app, "undo")?;
        app.handle_key(k(KeyCode::Enter))?;
    }
    assert!(
        app.status_msg.as_deref().unwrap_or("").contains("next time"),
        "no graduation nudge after 3 bar uses"
    );
    println!("[selfcheck] graduation nudge ........... PASS");

    // 11. `!` shell mode: the command reaches a real PTY in a pane.
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Char('!')))?;
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("SH !"), "! did not enter shell mode");
    typ(&mut app, "echo ares_shell_ok")?;
    app.handle_key(k(KeyCode::Enter))?;
    assert!(app.mode == mode::Mode::Terminal, "shell command did not attach terminal");
    std::thread::sleep(std::time::Duration::from_millis(900));
    let tid = match app.focused_pane().content {
        pane::PaneContent::Terminal(id) => id,
        _ => panic!("focused pane is not a terminal"),
    };
    assert!(
        app.terms[&tid].screen().contents().contains("ares_shell_ok"),
        "shell command output not found in PTY"
    );
    println!("[selfcheck] bar `!` → shell ............ PASS");

    // 12. Ctrl+Space works INSIDE the terminal, and closing returns to it.
    app.handle_key(kc(KeyCode::Char(' ')))?;
    assert!(app.mode == mode::Mode::Bar, "Ctrl+Space dead inside terminal");
    app.handle_key(k(KeyCode::Esc))?;
    assert!(app.mode == mode::Mode::Terminal, "bar did not return to terminal");
    app.handle_key(kc(KeyCode::Char('g')))?; // detach
    assert!(app.mode == mode::Mode::Edit, "C-g did not detach");
    println!("[selfcheck] bar from terminal .......... PASS");

    // 13. Ask mode with no key gives a friendly notice (or fires if a key is set).
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Tab))?; // → ASK
    typ(&mut app, "hi")?;
    app.handle_key(k(KeyCode::Enter))?;
    if agent::AgentConfig::from_env().is_configured() {
        assert!(app.agent_pending, "expected a pending request with key set");
        println!("[selfcheck] ask agent (key set) ........ PASS");
    } else {
        let ans = app.agent_answer.clone().unwrap_or_default();
        assert!(ans.contains("API key"), "expected no-key notice, got: {ans:?}");
        println!("[selfcheck] ask agent (no key) ......... PASS");
    }
    app.handle_key(k(KeyCode::Esc))?;

    // 14. kill_buffer with two panes on one buffer must not leave a dangling id.
    let mut app = App::new(None)?;
    typ(&mut app, "one")?;
    app.handle_key(kc(KeyCode::Char('x')))?;
    app.handle_key(k(KeyCode::Char('2')))?; // C-x 2 → split (same buffer)
    app.new_scratch(); // a second buffer to land on
    app.handle_key(kc(KeyCode::Char('x')))?;
    app.handle_key(k(KeyCode::Char('k')))?; // C-x k → kill buffer
    app.handle_key(kc(KeyCode::Char('x')))?;
    app.handle_key(k(KeyCode::Char('o')))?; // C-x o → other pane (would panic before)
    let _ = app.focused_buf(); // must not panic
    assert_eq!(app.buffers.len(), 1, "kill_buffer left extra buffers");
    println!("[selfcheck] kill_buffer retarget ....... PASS");

    // 15. A real terminal PTY spawns and echoes.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut sh = terminal::spawn(0, 24, 80, 1000, None, tx)?;
    sh.send_bytes(b"echo ares_pty_ok\n");
    std::thread::sleep(std::time::Duration::from_millis(700));
    while rx.try_recv().is_ok() {}
    assert!(sh.screen().contents().contains("ares_pty_ok"), "terminal echo not found");
    println!("[selfcheck] terminal PTY echo .......... PASS");

    // 15b. Scrollback: history survives past the viewport and the view can
    //      scroll back through it, then snap to live.
    sh.send_bytes(b"seq 1 100\n");
    std::thread::sleep(std::time::Duration::from_millis(700));
    while rx.try_recv().is_ok() {}
    let live = sh.screen().contents();
    assert!(live.contains("100"), "seq output missing");
    assert!(!live.contains("\n3\n"), "expected early lines scrolled out of view");
    sh.scroll_view(80); // back into history
    assert!(sh.view_offset() > 0, "view offset did not move");
    let back = sh.screen().contents();
    assert_ne!(live, back, "scrollback view identical to live view");
    assert!(back.contains("\n3\n") || back.contains("seq 1 100"),
        "history not visible after scrolling back");
    sh.scroll_to_live();
    assert_eq!(sh.view_offset(), 0, "snap-back failed");
    assert_eq!(sh.screen().contents(), live, "live view changed after snap-back");
    // W5: history_tail pages the scrollback (below the viewport) and restores the
    // live view — must reach early lines the live screen scrolled past.
    let tail = sh.history_tail(60);
    assert!(tail.contains("100"), "history_tail missing recent output");
    assert!(tail.lines().count() > 24, "history_tail did not exceed one screen");
    assert_eq!(sh.view_offset(), 0, "history_tail left the view scrolled back");
    println!("[selfcheck] terminal scrollback ........ PASS");

    // 15c. Dead-shell lifecycle: exit → Exited event → Enter recycles the pane.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Char('!')))?;
    typ(&mut app, "exit")?;
    app.handle_key(k(KeyCode::Enter))?; // attached; shell exits immediately
    let mut dead = false;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.tick(); // drains TermEvent::Exited
        if app.terms.values().any(|t| t.exited) { dead = true; break; }
    }
    assert!(dead, "shell exit not detected");
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(
        screen_text(&term).contains("process exited"),
        "dead-shell notice not rendered"
    );
    app.handle_key(k(KeyCode::Enter))?; // dismiss
    assert!(app.terms.is_empty(), "exited terminal not cleaned up");
    assert!(
        matches!(app.focused_pane().content, pane::PaneContent::Editor(_)),
        "pane not recycled to an editor"
    );
    assert!(app.mode == mode::Mode::Edit, "mode not restored after dismissal");
    println!("[selfcheck] dead-shell lifecycle ....... PASS");

    // 15d. Autosave: a modified path-backed buffer hits disk on the timer.
    let autosave_file = cfg_dir.join("autosave_probe.txt");
    std::fs::write(&autosave_file, "original")?;
    let mut app = App::new(Some(autosave_file.to_string_lossy().to_string()))?;
    app.tuning.autosave_secs = 1;
    app.handle_key(kc(KeyCode::Char('e')))?; // line end
    typ(&mut app, " EDITED")?;
    let ticks = (1000 / app.tuning.poll_interval_ms).max(1) + 2;
    for _ in 0..ticks {
        app.tick();
    }
    assert!(
        std::fs::read_to_string(&autosave_file)?.contains("EDITED"),
        "autosave did not write the buffer to disk"
    );
    println!("[selfcheck] autosave (crash safety) .... PASS");

    // ── Movement audit ───────────────────────────────────────────────────────
    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    // 16. Tabs: C-t t = new tab (travel mode); M-{ / M-} switch (sent as
    //     ALT|SHIFT, normalized); M-1 jumps directly.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char('t')))?; // C-t → travel mode
    assert!(app.mode == mode::Mode::Tab, "C-t did not enter travel mode");
    app.handle_key(k(KeyCode::Char('t')))?; // t → new tab, exits mode
    assert_eq!(app.tabs.len(), 2, "C-t t did not open a new tab");
    assert_eq!(app.active_tab, 1);
    assert!(app.mode == mode::Mode::Edit, "new tab did not exit travel mode");
    app.handle_key(KeyEvent::new(KeyCode::Char('{'), KeyModifiers::ALT | KeyModifiers::SHIFT))?;
    assert_eq!(app.active_tab, 0, "M-{{ did not switch to the previous tab");
    app.handle_key(KeyEvent::new(KeyCode::Char('}'), KeyModifiers::ALT | KeyModifiers::SHIFT))?;
    assert_eq!(app.active_tab, 1, "M-}} did not switch to the next tab");
    app.handle_key(alt(KeyCode::Char('1')))?;
    assert_eq!(app.active_tab, 0, "M-1 did not jump to tab 1");
    println!("[selfcheck] tab movement ............... PASS");

    // 17. Panes: C-\ splits; Ctrl-arrows focus directionally (Alt-arrows now move
    //     by token/page); M-o cycles; C-x x moves.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char('\\')))?; // C-\ → split right
    assert_eq!(app.tab().layout.count(), 2, "C-\\ did not split");
    let right = app.focused_pane_id();
    term.draw(|f| ui::render(f, &mut app))?; // populate pane geometry
    app.handle_key(kc(KeyCode::Left))?; // C-← → focus left pane
    let left = app.focused_pane_id();
    assert_ne!(left, right, "C-Left did not move focus");
    app.handle_key(kc(KeyCode::Right))?; // C-→ → back right
    assert_eq!(app.focused_pane_id(), right, "C-Right did not move focus back");
    app.handle_key(alt(KeyCode::Char('o')))?; // M-o → cycle
    assert_eq!(app.focused_pane_id(), left, "M-o did not cycle panes");
    // Move (swap): give the focused pane its own buffer, then C-x x.
    let buf1 = app.new_scratch();
    app.panes.get_mut(&left).unwrap().content = pane::PaneContent::Editor(buf1);
    app.handle_key(kc(KeyCode::Char('x')))?;
    app.handle_key(k(KeyCode::Char('x')))?; // C-x x → swap with next pane
    assert_eq!(app.focused_pane_id(), right, "swap did not follow the moved content");
    assert!(
        matches!(app.focused_pane().content, pane::PaneContent::Editor(id) if id == buf1),
        "pane content did not move on swap"
    );
    println!("[selfcheck] pane movement + swap ....... PASS");

    // 18. M-g goes to a line.
    let mut app = App::new(None)?;
    typ(&mut app, "l1")?;
    app.handle_key(k(KeyCode::Enter))?;
    typ(&mut app, "l2")?;
    app.handle_key(k(KeyCode::Enter))?;
    typ(&mut app, "l3")?;
    app.handle_key(alt(KeyCode::Char('g')))?; // M-g → goto-line prompt
    assert!(app.mode == mode::Mode::Prompt, "M-g did not prompt");
    typ(&mut app, "1")?;
    app.handle_key(k(KeyCode::Enter))?;
    assert_eq!(app.focused_pane().cursor_row, 0, "goto-line did not jump");
    println!("[selfcheck] goto line (M-g) ............ PASS");

    // 19. Paste: bracketed-paste text inserts; C-v is bound to Paste.
    app.paste_text("PASTED");
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("PASTED"), "paste_text did not insert");
    let cv = config::parse_key("C-v").unwrap();
    assert_eq!(
        app.keys.lookup(&[cv]),
        Some(palette::Action::Paste),
        "C-v is not bound to Paste"
    );
    println!("[selfcheck] paste (C-v + bracketed) .... PASS");

    // 20. Ctrl+C / Ctrl+V round-trip: select → C-c → move → C-v duplicates;
    //     C-c with no selection copies the whole line.
    let mut app = App::new(None)?;
    typ(&mut app, "hello world")?;
    app.handle_key(kc(KeyCode::Char('a')))?; // line start
    for _ in 0..5 {
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT))?;
    }
    app.handle_key(kc(KeyCode::Char('c')))?; // C-c → copy selection
    assert_eq!(app.kill_ring.last().map(String::as_str), Some("hello"), "C-c did not copy");
    app.handle_key(kc(KeyCode::Char('e')))?; // line end
    app.handle_key(kc(KeyCode::Char('v')))?; // C-v → paste
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("hello worldhello"), "C-v did not paste the copy");
    app.handle_key(kc(KeyCode::Char('c')))?; // no selection → copy line
    assert_eq!(
        app.kill_ring.last().map(String::as_str),
        Some("hello worldhello"),
        "C-c without selection did not copy the line"
    );
    println!("[selfcheck] Ctrl+C / Ctrl+V ............ PASS");

    // 21. C-t travel mode: cheat panel renders; navigation stays, creation exits.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char('t')))?; // enter travel mode
    term.draw(|f| ui::render(f, &mut app))?;
    let t21 = screen_text(&term);
    assert!(t21.contains("travel"), "travel cheat panel missing");
    assert!(t21.contains("split right"), "cheat panel missing split hint");
    app.handle_key(k(KeyCode::Char('-')))?; // split below, exits
    assert_eq!(app.tab().layout.count(), 2, "travel '-' did not split");
    assert!(app.mode == mode::Mode::Edit, "split did not exit travel mode");
    app.handle_key(kc(KeyCode::Char('t')))?;
    app.handle_key(k(KeyCode::Char('o')))?; // next pane — stays in mode
    assert!(app.mode == mode::Mode::Tab, "navigation should stay in travel mode");
    app.handle_key(k(KeyCode::Char('|')))?; // split right, exits
    assert_eq!(app.tab().layout.count(), 3, "travel '|' did not split");
    app.handle_key(kc(KeyCode::Char('t')))?;
    app.handle_key(k(KeyCode::Esc))?; // Esc leaves
    assert!(app.mode == mode::Mode::Edit, "Esc did not leave travel mode");
    println!("[selfcheck] C-t travel mode ............ PASS");

    // 22. Modern-terminal chords are bound (fire under the kitty protocol).
    for (seq, action) in [
        ("C-{", palette::Action::PrevTab),
        ("C-}", palette::Action::NextTab),
        ("C--", palette::Action::SplitHorizontal),
        ("C-|", palette::Action::SplitVertical),
        ("M--", palette::Action::SplitHorizontal),
    ] {
        let chord = config::parse_key(seq).unwrap();
        assert_eq!(app.keys.lookup(&[chord]), Some(action), "{seq} not bound");
    }
    println!("[selfcheck] modern chords (kitty) ...... PASS");

    // 23. Pane movement without Meta: C-o cycles; Ctrl+arrows move directionally.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char('\\')))?; // split right
    let right = app.focused_pane_id();
    term.draw(|f| ui::render(f, &mut app))?;
    app.handle_key(kc(KeyCode::Left))?; // Ctrl+← → left pane
    let left = app.focused_pane_id();
    assert_ne!(left, right, "Ctrl+Left did not move focus");
    app.handle_key(kc(KeyCode::Char('o')))?; // C-o → cycle
    assert_eq!(app.focused_pane_id(), right, "C-o did not cycle panes");
    println!("[selfcheck] pane nav sans Meta ......... PASS");

    // 24. Terminal chrome: navigation chords work INSIDE a terminal pane.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Char('!')))?;
    typ(&mut app, "true")?;
    app.handle_key(k(KeyCode::Enter))?; // now attached to a shell pane
    assert!(app.mode == mode::Mode::Terminal);
    app.handle_key(kc(KeyCode::Char('t')))?; // C-t → travel mode, even here
    assert!(app.mode == mode::Mode::Tab, "C-t dead inside terminal");
    app.handle_key(k(KeyCode::Char('t')))?; // new tab (editor) — exits travel
    assert_eq!(app.tabs.len(), 2);
    assert!(app.mode == mode::Mode::Edit);
    app.handle_key(KeyEvent::new(KeyCode::Char('{'), KeyModifiers::ALT | KeyModifiers::SHIFT))?;
    assert_eq!(app.active_tab, 0, "M-{{ did not switch tab from editor");
    // Back on the terminal tab: M-} from INSIDE the terminal switches tabs.
    app.handle_key(k(KeyCode::Enter))?; // re-attach (terminal pane focused)
    assert!(app.mode == mode::Mode::Terminal);
    app.handle_key(KeyEvent::new(KeyCode::Char('}'), KeyModifiers::ALT | KeyModifiers::SHIFT))?;
    assert_eq!(app.active_tab, 1, "M-}} dead inside terminal");
    println!("[selfcheck] terminal chrome layer ...... PASS");

    // 25. Cmd (super) bindings parse and are bound.
    let cmd_c = config::parse_key("cmd-c").unwrap();
    assert!(cmd_c.modifiers.contains(KeyModifiers::SUPER), "cmd- prefix not SUPER");
    assert_eq!(app.keys.lookup(&[cmd_c]), Some(palette::Action::CopyRegion));
    assert_eq!(
        app.keys.lookup(&[config::parse_key("cmd-v").unwrap()]),
        Some(palette::Action::Paste)
    );
    println!("[selfcheck] cmd-c / cmd-v bindings ..... PASS");

    // 26. Tuning: defaults written with descriptions; user overrides apply.
    let tuning_path = cfg_dir.join("mars").join("tuning.json");
    let written = std::fs::read_to_string(&tuning_path)?;
    assert!(written.contains("description"), "tuning defaults lack descriptions");
    assert!(written.contains("which_key_delay_ms"), "tuning defaults missing knobs");
    std::fs::write(
        &tuning_path,
        r#"{ "max_panes": { "value": 2, "description": "test override" } }"#,
    )?;
    let mut app = App::new(None)?;
    assert_eq!(app.tuning.max_panes, 2, "tuning override not applied");
    app.handle_key(kc(KeyCode::Char('\\')))?; // 2nd pane — at the cap
    app.handle_key(kc(KeyCode::Char('\\')))?; // refused
    assert_eq!(app.tab().layout.count(), 2, "max_panes override not enforced");
    assert!(
        app.status_msg.as_deref().unwrap_or("").contains("Max 2"),
        "cap message should reflect the tuned value"
    );
    std::fs::remove_file(&tuning_path)?; // restore defaults for any later checks
    println!("[selfcheck] tuning knobs + override .... PASS");

    // 26b. Default gutter is a slim pointer (no numbers); the knob restores
    //      the number column; the line/col lives in the status bar.
    let mut app = App::new(None)?;
    app.handle_key(k(KeyCode::Char('G')))?; // dismiss splash, type
    typ(&mut app, "UT")?;
    term.draw(|f| ui::render(f, &mut app))?;
    let t26 = screen_text(&term);
    assert!(t26.contains("GUT"), "typed text missing");
    assert!(!t26.contains("   1│"), "number gutter rendered despite line_numbers=false");
    assert!(t26.contains("▸"), "pointer gutter marker missing on the cursor line");
    assert!(t26.contains("Ln 1, Col 4"), "status bar missing line/col readout");
    app.tuning.line_numbers = true;
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("   1│"), "line_numbers knob did not restore numbers");
    println!("[selfcheck] pointer gutter + Ln/Col ... PASS");

    // 26b2. Project index: skip-list + cap.
    let proj = cfg_dir.join(format!("proj-{}", std::process::id()));
    {
        use std::io::Write as _;
        std::fs::create_dir_all(proj.join("src"))?;
        std::fs::create_dir_all(proj.join("target"))?; // must be skipped
        for f in ["src/main.rs", "src/session.rs", "README.md"] {
            let p = proj.join(f);
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::File::create(&p)?.write_all(b"x")?;
        }
        std::fs::File::create(proj.join("target/junk.rs"))?.write_all(b"x")?;
        let idx = project::Index::build(proj.clone(), 10_000, &["target".to_string()]);
        assert!(idx.files.iter().any(|f| f == "src/session.rs"), "index missing a real file");
        assert!(!idx.files.iter().any(|f| f.contains("target")), "index did not skip target/");
        assert!(project::Index::build(proj.clone(), 2, &["target".to_string()]).files.len() <= 2,
            "index did not honor the cap");
    }
    println!("[selfcheck] project index (skip/cap) .. PASS");

    // 26b3. Left file tree: `@` opens it, browse shows folders (not target/),
    //       expand reveals children, type-to-filter shortlists, Enter opens.
    let mut app = App::new(None)?;
    // Root the tree at the temp project (browse reads the real filesystem).
    app.set_project_index_for_test(
        proj.clone(),
        vec!["src/main.rs".into(), "src/session.rs".into(), "README.md".into()],
    );
    app.handle_key(kc(KeyCode::Char(' ')))?; // command bar
    app.handle_key(k(KeyCode::Char('@')))?;  // → open the tree
    assert!(matches!(app.mode, mode::Mode::Tree) && app.tree_open, "@ did not open the tree");
    assert!(app.tree_rows.iter().any(|r| r.label == "src" && r.is_dir), "tree missing src/ folder");
    assert!(!app.tree_rows.iter().any(|r| r.label == "target"), "tree showed the ignored dir");
    // Move to src/ (row after the ../ row) and expand it.
    app.handle_key(k(KeyCode::Down))?;   // ../ → src
    app.handle_key(k(KeyCode::Enter))?;  // expand
    assert!(app.tree_rows.iter().any(|r| r.label == "session.rs" && r.depth == 1),
        "expanding src/ did not reveal its children");
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("src"), "tree sidebar did not render");
    // Type-to-filter → fuzzy shortlist over the index.
    typ(&mut app, "sesn")?;
    assert_eq!(app.tree_rows.first().map(|r| r.label.as_str()), Some("src/session.rs"),
        "filter did not shortlist session.rs on top");
    // → PREVIEWS the top file: shows it but stays in the tree (reversible).
    let bufs_before = app.buffers.len();
    app.handle_key(k(KeyCode::Right))?;
    assert!(matches!(app.mode, mode::Mode::Tree), "→ on a file left the tree (should preview)");
    // A second preview of the same file must not duplicate the buffer.
    app.handle_key(k(KeyCode::Right))?;
    assert_eq!(app.buffers.len(), bufs_before + 1, "preview duplicated the buffer");
    // Enter COMMITS: opens the top match → focus returns to the editor.
    app.handle_key(k(KeyCode::Enter))?;
    assert!(matches!(app.mode, mode::Mode::Edit), "Enter on a tree file did not focus the editor");
    assert!(app.tree_open, "sidebar should stay open after opening a file");
    println!("[selfcheck] file tree (preview/open) .. PASS");

    // 26b4. C-x d toggles the tree; `../` re-roots to the parent directory.
    let mut app = App::new(None)?;
    app.set_project_index_for_test(proj.clone(), vec!["README.md".into()]);
    app.handle_key(kc(KeyCode::Char('x')))?;
    app.handle_key(k(KeyCode::Char('d')))?; // C-x d
    assert!(app.tree_open && matches!(app.mode, mode::Mode::Tree), "C-x d did not open the tree");
    let root_before = app.file_tree.as_ref().map(|t| t.root.clone()).unwrap();
    app.handle_key(k(KeyCode::Enter))?; // selected row 0 is ../ → re-root up
    let root_after = app.file_tree.as_ref().map(|t| t.root.clone()).unwrap();
    assert_eq!(root_after, root_before.parent().unwrap(), "../ did not re-root to the parent");
    app.handle_key(k(KeyCode::Esc))?; // Esc closes the focused tree
    assert!(!app.tree_open && matches!(app.mode, mode::Mode::Edit), "Esc did not close the tree");
    // Closing forgets navigation state: reopening starts back at the project root.
    app.handle_key(kc(KeyCode::Char('x')))?;
    app.handle_key(k(KeyCode::Char('d')))?; // C-x d → reopen
    let reopened_root = app.file_tree.as_ref().map(|t| t.root.clone()).unwrap();
    assert_eq!(reopened_root, root_before, "tree did not reset to the project root on reopen");
    assert!(app.tree_open && matches!(app.mode, mode::Mode::Tree), "C-x d did not reopen the tree");
    println!("[selfcheck] file tree (reset/../) ..... PASS");

    // 26c. Conversation transcript: history renders, panel grows past the old
    //      16-row cap, scrolls, and C-l clears.
    let mut app = App::new(None)?;
    app.agent_history.push(("user".into(), "first question".into()));
    let long: String = (1..=30).map(|i| format!("L{i}")).collect::<Vec<_>>().join("\n");
    app.agent_history.push(("assistant".into(), long));
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Tab))?; // → ASK
    term.draw(|f| ui::render(f, &mut app))?;
    let t27 = screen_text(&term);
    // Bottom-pinned: the tail of a long answer is visible, well past the old
    // 16-row cap (L18 could not have rendered before).
    assert!(t27.contains("L18") && t27.contains("L30"),
        "panel capped too small for a long answer");
    // Scroll up to reach the start of the conversation.
    for _ in 0..15 {
        app.handle_key(k(KeyCode::Up))?;
    }
    term.draw(|f| ui::render(f, &mut app))?;
    let t27b = screen_text(&term);
    assert!(t27b.contains("first question"), "scroll-up did not reach the first turn");
    assert!(t27b.contains("more"), "scroll indicator missing");
    app.handle_key(kc(KeyCode::Char('l')))?; // new chat
    assert!(app.agent_history.is_empty(), "C-l did not clear the conversation");
    app.handle_key(k(KeyCode::Esc))?;
    println!("[selfcheck] ask transcript + scroll .... PASS");

    // 26d. History really reaches the provider; directives parse.
    let msgs = agent::build_messages(
        "reg", "screen",
        &[("user".into(), "q1".into()), ("assistant".into(), "a1".into())],
        "q2",
    );
    assert_eq!(msgs.len(), 4, "system + 2 history + question expected");
    assert!(msgs[1]["content"].as_str().unwrap_or("").contains("q1"));
    assert!(msgs[0]["content"].as_str().unwrap_or("").contains("screen"));
    let (d1, dir1) = agent::parse_directive("use ls.\nTYPE: ls -la");
    assert_eq!(d1, "use ls.");
    assert_eq!(dir1, Some(agent::AgentDirective::Type("ls -la".into())));
    let (_, dir2) = agent::parse_directive("split it\nRUN: SplitVertical");
    assert_eq!(dir2, Some(agent::AgentDirective::Run("SplitVertical".into())));
    assert_eq!(agent::parse_directive("plain answer").1, None);
    println!("[selfcheck] agent history + directives . PASS");

    // 26e. TYPE directive types into the terminal pane on Enter.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Char('!')))?;
    typ(&mut app, "true")?;
    app.handle_key(k(KeyCode::Enter))?; // attached shell pane
    app.handle_key(kc(KeyCode::Char(' ')))?; // Ctrl+Space → inline shell composer
    app.handle_key(kc(KeyCode::Char(' ')))?; // again → full command bar
    app.handle_key(k(KeyCode::Tab))?; // → ASK
    app.agent_directive = Some(agent::AgentDirective::Type("echo mars_type_ok".into()));
    app.handle_key(k(KeyCode::Enter))?; // confirm-fire
    assert!(app.mode == mode::Mode::Terminal, "TYPE did not land in the terminal");
    let mut typed = false;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.tick();
        if let pane::PaneContent::Terminal(tid) = app.focused_pane().content {
            if app.terms[&tid].screen().contents().contains("mars_type_ok") {
                typed = true;
                break;
            }
        }
    }
    assert!(typed, "TYPE'd command never reached the PTY");
    println!("[selfcheck] TYPE → terminal ............ PASS");

    // 26f. Renames: tab via travel `r`; pane via action; auto-name plumbing.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char('t')))?; // travel mode
    app.handle_key(k(KeyCode::Char('r')))?; // rename tab → prompt (prefilled "1")
    assert!(app.mode == mode::Mode::Prompt, "travel r did not prompt");
    app.handle_key(k(KeyCode::Backspace))?; // clear the "1"
    typ(&mut app, "build")?;
    app.handle_key(k(KeyCode::Enter))?;
    assert_eq!(app.tab().name, "build", "tab rename failed");
    // Auto-name must NOT override a user-chosen (non-numeric) name.
    let tid0 = app.tab().id;
    app.agent_tx.send(agent::AgentEvent::AutoName { tab_id: tid0, name: "x".into() })?;
    app.tick();
    assert_eq!(app.tab().name, "build", "auto-name overrode a manual rename");
    app.run_action(palette::Action::RenamePane);
    typ(&mut app, "logs")?;
    app.handle_key(k(KeyCode::Enter))?;
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains(" logs "), "pane title not rendered");
    // Positive auto-name path: a default-named tab accepts the label.
    let mut app = App::new(None)?;
    let tid1 = app.tab().id;
    app.agent_tx.send(agent::AgentEvent::AutoName { tab_id: tid1, name: "auto-label".into() })?;
    app.tick();
    assert_eq!(app.tab().name, "auto-label", "auto-name not applied");
    println!("[selfcheck] renames + auto-name ........ PASS");

    // ── Phase 1 agentic workflows ────────────────────────────────────────────

    // 26g. Directive parser: OPEN added; lenient to backticks + trailing lines.
    use agent::AgentDirective;
    assert_eq!(
        agent::parse_directive("Line 42 is the bug.\nOPEN: src/main.rs:42").1,
        Some(AgentDirective::Open("src/main.rs:42".into()))
    );
    assert_eq!(
        agent::parse_directive("try this\n`TYPE: git status`").1,
        Some(AgentDirective::Type("git status".into())),
        "parser should tolerate backtick-wrapped directives"
    );
    // Directive on the 2nd-to-last line (model added a sign-off).
    let (disp, dir) = agent::parse_directive("Here's the fix.\nRUN: SplitVertical\nHope that helps!");
    assert_eq!(dir, Some(AgentDirective::Run("SplitVertical".into())));
    assert!(!disp.contains("SplitVertical"), "directive line should be stripped from display");
    assert_eq!(agent::parse_directive("just prose").1, None);
    // Rate-limit retry hint parsing (rounds up).
    assert_eq!(agent::retry_secs("quota exceeded. Please retry in 14.89197552s."), Some(15));
    assert_eq!(agent::retry_secs("no hint here"), None);
    println!("[selfcheck] directive parse (OPEN/lenient) PASS");

    // 26h. OPEN lands the cursor at the cited line in the named file.
    let probe = cfg_dir.join("open_probe.txt");
    std::fs::write(&probe, "a\nb\nc\nd\ne\nf\n")?;
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char(' ')))?; // bar
    app.handle_key(k(KeyCode::Tab))?; // → ASK
    app.agent_directive = Some(AgentDirective::Open(format!("{}:4", probe.to_string_lossy())));
    app.handle_key(k(KeyCode::Enter))?; // fire the directive
    assert_eq!(app.focused_pane().cursor_row, 3, "OPEN did not land on line 4");
    assert!(app.focused_buf().name.contains("open_probe"), "OPEN did not open the file");
    println!("[selfcheck] OPEN directive lands ....... PASS");

    // 26i. W1/W2: ExplainThis and ExplainFailure open Ask pre-filled + submit.
    let mut app = App::new(None)?;
    typ(&mut app, "some code")?;
    app.run_action(palette::Action::ExplainThis);
    assert!(app.mode == mode::Mode::Bar, "ExplainThis did not open the bar");
    assert!(
        matches!(app.palette.as_ref().map(|p| &p.bar_mode), Some(palette::BarMode::Ask)),
        "ExplainThis is not in Ask mode"
    );
    // Pre-filled with a grounded question…
    assert!(
        app.palette.as_ref().map(|p| p.query.contains("Explain")).unwrap_or(false),
        "ExplainThis did not pre-fill a question"
    );
    // …and it submitted (no key in this env → the no-key notice proves the attempt).
    assert!(
        app.agent_answer.as_deref().unwrap_or("").contains("API key"),
        "ExplainThis did not auto-submit"
    );
    println!("[selfcheck] explain-this / failure .... PASS");

    // 26j. Pane resize changes the split ratio; zoom collapses then restores.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char('\\')))?; // C-\ split right (2 panes)
    term.draw(|f| ui::render(f, &mut app))?;
    let two_rects = app.pane_rects.clone();
    assert_eq!(two_rects.len(), 2, "expected 2 panes");
    // Enter travel mode once; resize + zoom all stay in it (navigation stays).
    app.handle_key(kc(KeyCode::Char('t')))?; // travel mode
    app.handle_key(k(KeyCode::Char('>')))?; // grow focused pane
    app.handle_key(k(KeyCode::Char('>')))?;
    term.draw(|f| ui::render(f, &mut app))?;
    let grown = app.pane_rects.clone();
    let focused = app.focused_pane_id();
    let w_before = two_rects.iter().find(|(id, _)| *id == focused).unwrap().1.width;
    let w_after = grown.iter().find(|(id, _)| *id == focused).unwrap().1.width;
    assert!(w_after > w_before, "resize did not grow the focused pane");
    // Zoom: only the focused pane renders full-area; toggling restores 2.
    app.handle_key(k(KeyCode::Char('z')))?; // zoom (still in travel)
    term.draw(|f| ui::render(f, &mut app))?;
    assert_eq!(app.pane_rects.len(), 1, "zoom did not collapse to one pane");
    app.handle_key(k(KeyCode::Char('z')))?; // unzoom
    term.draw(|f| ui::render(f, &mut app))?;
    assert_eq!(app.pane_rects.len(), 2, "unzoom did not restore both panes");
    app.handle_key(k(KeyCode::Esc))?; // leave travel
    println!("[selfcheck] pane resize + zoom ........ PASS");

    // 26k. Terminal Ctrl+Space → the inline shell composer directly (the `!`
    //      behavior in one keystroke); a second Ctrl+Space reaches the command bar;
    //      with no agent key, Enter runs the typed command → terminal.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Char('!')))?; // open a terminal via bar `!`…
    typ(&mut app, "true")?;
    app.handle_key(k(KeyCode::Enter))?; // …now attached to a terminal pane
    assert!(app.mode == mode::Mode::Terminal, "not in a terminal");
    app.handle_key(kc(KeyCode::Char(' ')))?; // Ctrl+Space → shell composer (one keystroke)
    assert!(
        matches!(app.palette.as_ref().map(|p| &p.bar_mode), Some(palette::BarMode::Shell)),
        "Ctrl+Space in terminal did not open the shell composer"
    );
    app.handle_key(kc(KeyCode::Char(' ')))?; // a second Ctrl+Space → the command bar
    assert!(
        matches!(app.palette.as_ref().map(|p| &p.bar_mode), Some(palette::BarMode::Command)),
        "second Ctrl+Space did not reach the command bar"
    );
    app.handle_key(k(KeyCode::Char('!')))?; // ! → back to shell mode
    typ(&mut app, "echo composer_ok")?;
    app.handle_key(k(KeyCode::Enter))?; // no key → runs the command directly
    assert!(app.mode == mode::Mode::Terminal, "shell composer Enter did not run the command");
    println!("[selfcheck] terminal shell composer .... PASS");

    // 26l. W6 watch: watching a pane + a verdict event → a failure notice that
    //      renders and is dismissed with Esc.
    let mut app = App::new(None)?;
    app.handle_key(kc(KeyCode::Char(' ')))?;
    app.handle_key(k(KeyCode::Char('!')))?;
    typ(&mut app, "true")?;
    app.handle_key(k(KeyCode::Enter))?; // attached to a terminal pane
    let tid = match app.focused_pane().content {
        pane::PaneContent::Terminal(id) => id,
        _ => panic!("focused pane is not a terminal"),
    };
    app.run_action(palette::Action::WatchPane);
    assert!(app.watches.get(&tid).map(|w| w.watched).unwrap_or(false), "pane not marked watched");
    // Simulate the background summary landing (the hermetic auto-name pattern).
    app.agent_tx.send(agent::AgentEvent::WatchSummary { term_id: tid, verdict: "failed: linker error".into() })?;
    app.tick();
    assert_eq!(app.notices.len(), 1, "verdict did not queue a notice");
    assert!(matches!(app.notices[0].kind, app::NoticeKind::Failure), "verdict not classified as failure");
    term.draw(|f| ui::render(f, &mut app))?;
    assert!(screen_text(&term).contains("linker error"), "notice not rendered");
    app.mode = mode::Mode::Edit; // Esc is a shell key in terminal mode; dismiss from edit
    app.handle_key(k(KeyCode::Esc))?;
    assert!(app.notices.is_empty(), "Esc did not dismiss the notice");
    // A failed background call must not wedge the gate: BgDone always clears it.
    app.bg_busy = true;
    app.agent_tx.send(agent::AgentEvent::BgDone)?;
    app.tick();
    assert!(!app.bg_busy, "BgDone did not release the bg_busy gate");
    println!("[selfcheck] watch pane + notice (W6) ... PASS");

    // 26l2. The quiet-timer actually fires: an old last_output_tick + a zero
    //       threshold trips maybe_fire_watches (no key → sets `triggered`, no LLM).
    {
        let mut app = App::new(None)?;
        app.tuning.watch_quiet_secs = 0;
        app.watches.insert(4242, app::WatchState { watched: true, last_output_tick: 0, ..Default::default() });
        app.tick();
        assert!(app.watches.get(&4242).map(|w| w.triggered).unwrap_or(false),
            "quiet timer did not fire maybe_fire_watches");
    }
    println!("[selfcheck] watch quiet-timer fires .... PASS");

    // 26m. Away Digest (W7+): quiet when idle; a watched task finishing while
    //      detached yields ONE duration-anchored headline (the W6 notice it
    //      subsumes is deduped), and the digest view renders sections — all
    //      deterministic, no API key (broker-ready: only verdict TEXT is LLM-made).
    {
        let mut app = App::new(None)?;
        app.on_detach();
        app.on_attach();
        assert!(app.notices.is_empty(), "briefing appeared when nothing changed");
        // A watched run finishes while detached: tick processes the verdict
        // (W6 notice + away-log event), then reattach builds the headline.
        app.on_detach();
        app.watches.insert(7, app::WatchState { watched: true, run_started_tick: 1, ..Default::default() });
        for _ in 0..20 { app.frame_tick += 1; } // time passes while away
        app.agent_tx.send(agent::AgentEvent::WatchSummary { term_id: 7, verdict: "failed: tests red".into() })?;
        app.tick();
        app.on_attach();
        assert_eq!(app.notices.len(), 1, "expected exactly one briefing (W6 dupe not subsumed?)");
        let n = &app.notices[0];
        assert!(n.text.contains("while away") && n.text.contains("tests red"),
            "headline missing duration/verdict: {}", n.text);
        assert!(matches!(n.kind, app::NoticeKind::Failure), "failing briefing not a Failure");
        // The digest view: sectioned, with the run duration, rendered with no key.
        app.run_action(palette::Action::AwayDigest);
        let d = app.agent_history.last().map(|(_, t)| t.clone()).unwrap_or_default();
        assert!(d.contains("needs you") && d.contains("tests red") && d.contains("ran "),
            "digest sections/duration missing:\n{d}");
    }
    println!("[selfcheck] away digest (W7+) ......... PASS");

    // 26n. W4/W5: NEED: parses; the first NEED re-asks (not surfaced), a second
    //      (depth capped) is surfaced normally.
    {
        assert_eq!(
            agent::parse_directive("looking…\nNEED: scrollback").1,
            Some(agent::AgentDirective::Need(agent::NeedKind::Scrollback)),
            "NEED: scrollback did not parse"
        );
        assert_eq!(
            agent::parse_directive("NEED: tab api").1,
            Some(agent::AgentDirective::Need(agent::NeedKind::Tab("api".into()))),
            "NEED: tab did not parse"
        );
        let mut app = App::new(None)?;
        let base = app.agent_history.len();
        let need = || agent::AgentEvent::Answer {
            text: "need more".into(),
            directive: Some(agent::AgentDirective::Need(agent::NeedKind::Scrollback)),
        };
        app.agent_tx.send(need())?;
        app.tick(); // depth 0→1, re-asks (no key → no-op), NOT surfaced
        assert_eq!(app.agent_history.len(), base, "first NEED should not reach the transcript");
        app.agent_tx.send(need())?;
        app.tick(); // depth capped → surfaced as a normal answer
        assert_eq!(app.agent_history.len(), base + 1, "capped NEED should surface");
    }
    println!("[selfcheck] NEED: expansion (W4/W5) ... PASS");

    // 27. Session daemon: detach → state + shells survive → reattach; takeover;
    //     version handshake; quit removes the socket. Fully headless.
    {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixStream;

        let sname = format!("selfcheck-{}", std::process::id());
        let spath = session::socket_path(&sname)?;
        let sname2 = sname.clone();
        let server = std::thread::spawn(move || session::server_main(&sname2, None));

        // Wait for the daemon socket.
        let mut up = false;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(30));
            if UnixStream::connect(&spath).is_ok() { up = true; break; }
        }
        assert!(up, "session server did not come up");

        // A persistent test client: one writer + one reader per connection
        // (mirrors the real client_main — never re-clone/drop per frame).
        // Output bytes are fed through a real ANSI parser (vt100 — the same
        // crate that renders terminal panes) so incremental cell diffs
        // (cursor repositions interleaved between changed characters) are
        // interpreted correctly instead of pattern-matched as raw bytes.
        struct TestClient {
            writer: UnixStream,
            reader: BufReader<UnixStream>,
            screen: vt100::Parser,
        }
        impl TestClient {
            fn connect(path: &std::path::Path, version: &str) -> Result<Self> {
                let stream = UnixStream::connect(path)?;
                let reader = BufReader::new(stream.try_clone()?);
                let mut me = TestClient { writer: stream, reader, screen: vt100::Parser::new(30, 100, 0) };
                session::write_frame(&mut me.writer, &session::ClientFrame::Hello {
                    cols: 100, rows: 30, version: version.to_string(),
                })?;
                Ok(me)
            }
            fn key(&mut self, key: KeyEvent) -> Result<()> {
                session::write_frame(&mut self.writer, &session::ClientFrame::Key(key))
                    .map_err(Into::into)
            }
            fn text(&mut self, s: &str) -> Result<()> {
                for c in s.chars() {
                    self.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))?;
                }
                Ok(())
            }
            /// Read Output frames until `needle` appears in the interpreted
            /// screen contents (or an Exit arrives), within `secs`.
            fn read_until(&mut self, needle: &str, secs: u64) -> Result<(bool, Option<String>)> {
                use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                self.reader.get_ref().set_read_timeout(Some(std::time::Duration::from_millis(200)))?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
                while std::time::Instant::now() < deadline {
                    let mut line = String::new();
                    match self.reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => match serde_json::from_str::<session::ServerFrame>(line.trim()) {
                            Ok(session::ServerFrame::Output { b64 }) => {
                                if let Ok(bytes) = B64.decode(b64) {
                                    self.screen.process(&bytes);
                                }
                                if self.screen.screen().contents().contains(needle) {
                                    return Ok((true, None));
                                }
                            }
                            Ok(session::ServerFrame::Exit { message }) => {
                                return Ok((self.screen.screen().contents().contains(needle), Some(message)));
                            }
                            Ok(session::ServerFrame::Status { .. }) => {}
                            Err(_) => {}
                        },
                        Err(_) => {} // timeout tick — keep waiting until deadline
                    }
                }
                Ok((self.screen.screen().contents().contains(needle), None))
            }
        }

        // c1 attaches, types a marker, sees it rendered.
        let mut c1 = TestClient::connect(&spath, env!("CARGO_PKG_VERSION"))?;
        c1.text("sessionmarker")?;
        let (found, _) = c1.read_until("sessionmarker", 5)?;
        assert!(found, "marker not rendered to first client");

        // Version handshake: bogus client is refused, c1 unaffected.
        let mut c_bad = TestClient::connect(&spath, "0.0.0-bogus")?;
        let (_, exit) = c_bad.read_until("\u{0}never\u{0}", 3)?;
        assert!(
            exit.map(|m| m.contains("version mismatch")).unwrap_or(false),
            "version mismatch not refused"
        );

        // Takeover + reattach: c2 attaches → c1 is dropped, c2 gets a full
        // redraw that still contains the marker (state survived).
        let mut c2 = TestClient::connect(&spath, env!("CARGO_PKG_VERSION"))?;
        let (_, c1_exit) = c1.read_until("\u{0}never\u{0}", 3)?;
        assert!(c1_exit.is_some(), "old client not notified on takeover");
        let (found2, _) = c2.read_until("sessionmarker", 5)?;
        assert!(found2, "state lost across reattach");

        // Shell pane survives a hard disconnect: start one, run a command,
        // drop the client entirely, reconnect, and find the output.
        c2.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL))?;
        c2.key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE))?;
        c2.text("echo daemon_pty_ok")?;
        c2.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))?;
        let (pty_ok, _) = c2.read_until("daemon_pty_ok", 8)?;
        assert!(pty_ok, "shell output not rendered in session");
        drop(c2); // hard disconnect — no Detach, just gone
        std::thread::sleep(std::time::Duration::from_millis(150));
        let mut c3 = TestClient::connect(&spath, env!("CARGO_PKG_VERSION"))?;
        let (pty_survived, _) = c3.read_until("daemon_pty_ok", 5)?;
        assert!(pty_survived, "PTY did not survive the disconnect");

        // `mars ls` sees it, including the attached state (c3 is attached).
        assert!(
            session::list_sessions()?
                .iter()
                .any(|(n, alive, attached)| n == &sname && *alive && *attached),
            "ls missing the live+attached session"
        );

        // Live rename: the socket moves, the attached client keeps working.
        let renamed = format!("{sname}-renamed");
        let rpath = session::socket_path(&renamed)?;
        {
            let ctl = UnixStream::connect(&spath)?;
            let mut w = ctl.try_clone()?;
            session::write_frame(&mut w, &session::ClientFrame::Rename { to: renamed.clone() })?;
        }
        let mut moved = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if rpath.exists() && !spath.exists() { moved = true; break; }
        }
        assert!(moved, "session rename did not move the socket");
        assert!(
            session::list_sessions()?.iter().any(|(n, alive, _)| n == &renamed && *alive),
            "renamed session missing from ls"
        );
        // c3 (attached before the rename) still drives the session.
        c3.text("post-rename")?;
        let (still_alive, _) = c3.read_until("post-rename", 5)?;
        assert!(still_alive, "attached client broke across the rename");

        // Quit ends the session: detach the shell, C-x C-c, confirm 'q'.
        c3.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL))?; // detach PTY
        c3.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL))?;
        c3.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))?;
        c3.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))?; // quit anyway
        let (_, quit_exit) = c3.read_until("\u{0}never\u{0}", 5)?;
        assert!(
            quit_exit.map(|m| m.contains("session ended")).unwrap_or(false),
            "quit did not end the session"
        );
        server.join().expect("server thread panicked")?;
        assert!(!rpath.exists(), "socket not removed after quit");
        println!("[selfcheck] session daemon ............ PASS");

        // 27b. Session management: Status reports detached; `kill` ends a
        //      session cleanly from outside.
        let kname = format!("selfcheck-kill-{}", std::process::id());
        let kpath = session::socket_path(&kname)?;
        let kname2 = kname.clone();
        let kserver = std::thread::spawn(move || session::server_main(&kname2, None));
        let mut up = false;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(30));
            if UnixStream::connect(&kpath).is_ok() { up = true; break; }
        }
        assert!(up, "kill-test server did not come up");
        assert!(
            session::list_sessions()?
                .iter()
                .any(|(n, alive, attached)| n == &kname && *alive && !*attached),
            "fresh session should be alive and detached"
        );
        session::kill_main(&kname)?;
        kserver.join().expect("kill-test server panicked")?;
        assert!(!kpath.exists(), "socket not removed after kill");
        println!("[selfcheck] session status + kill ..... PASS");
    }

    // 27c. Auto session name is a lowest-free number; session AI-name applies
    //      only while numeric (explicit names win).
    assert!(
        session::next_auto_name()?.parse::<u32>().is_ok(),
        "auto session name should be numeric"
    );
    {
        let mut app = App::new(None)?;
        app.session_name = Some("0".into()); // numeric → AI name may apply
        app.agent_tx.send(agent::AgentEvent::SessionName { name: "mars-dev".into() })?;
        app.tick();
        assert_eq!(app.rename_session_to.as_deref(), Some("mars-dev"), "numeric session not renamed");
        let mut app = App::new(None)?;
        app.session_name = Some("work".into()); // explicit → AI name ignored
        app.agent_tx.send(agent::AgentEvent::SessionName { name: "auto".into() })?;
        app.tick();
        assert!(app.rename_session_to.is_none(), "explicit session name overridden by AI");
    }
    println!("[selfcheck] session auto-naming ....... PASS");

    // 28. Config migration: a pre-rename ~/.config/ares is copied to mars/.
    {
        let mig_dir = std::env::temp_dir().join(format!("mars-migrate-{}", std::process::id()));
        let ares_dir = mig_dir.join("ares");
        std::fs::create_dir_all(&ares_dir)?;
        std::fs::write(
            ares_dir.join("keys.json"),
            r#"{ "edit": {}, "bar_open": ["ctrl-space", "M-x"] }"#,
        )?;
        std::fs::write(
            ares_dir.join("tuning.json"),
            r#"{ "max_panes": { "value": 3, "description": "migrated" } }"#,
        )?;
        std::env::set_var("XDG_CONFIG_HOME", &mig_dir);
        let app = App::new(None)?;
        assert_eq!(app.tuning.max_panes, 3, "ares→mars migration did not carry tuning");
        assert!(mig_dir.join("mars").join("keys.json").exists(), "keys.json not migrated");
        std::env::set_var("XDG_CONFIG_HOME", &cfg_dir); // back to the isolated dir
        let _ = std::fs::remove_dir_all(&mig_dir);
        println!("[selfcheck] ares→mars migration ....... PASS");
    }

    // 29. Gemini provider detection (env-based, no network).
    for v in ["MARS_LLM_KEY", "MARS_LLM_URL", "MARS_LLM_MODEL",
              "ARES_LLM_KEY", "ARES_LLM_URL", "ARES_LLM_MODEL"] {
        std::env::remove_var(v);
    }
    std::env::remove_var("GROQ_API_KEY");
    std::env::set_var("GEMINI_API_KEY", "test-key");
    let cfg = agent::AgentConfig::from_env();
    assert!(cfg.is_configured(), "GEMINI_API_KEY not detected");
    assert_eq!(cfg.provider, "gemini");
    assert!(cfg.url.contains("generativelanguage"), "wrong Gemini endpoint: {}", cfg.url);
    assert!(cfg.model.starts_with("gemini"), "wrong Gemini model: {}", cfg.model);
    std::env::remove_var("GEMINI_API_KEY");
    println!("[selfcheck] gemini provider ............ PASS");

    // 30. SSH broker: detection + precedence + honest availability + proxy round-trip.
    {
        use std::io::{BufRead, BufReader};
        for v in ["GROQ_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY",
                  "MARS_LLM_KEY", "ARES_LLM_KEY", "MARS_LLM_MODEL", "ARES_LLM_MODEL"] {
            std::env::remove_var(v);
        }
        // A looping responder standing in for `mars keyd`.
        let dir = std::env::temp_dir().join(format!("mars-broker-sc-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let sock = dir.join("auth.sock");
        let sock_s = sock.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&sock);
        let listener = std::os::unix::net::UnixListener::bind(&sock)?;
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue; // a bare connect probe (is_configured) — no request
                }
                let mut w = stream;
                let _ = session::write_frame(
                    &mut w,
                    &broker::BrokerResponse::Chat { text: "broker-ok".into() },
                );
            }
        });

        // Detection: a live forwarded socket ⇒ provider "broker", configured.
        std::env::set_var("MARS_AUTH_SOCK", &sock_s);
        let c = agent::AgentConfig::from_env();
        assert_eq!(c.provider, "broker", "MARS_AUTH_SOCK not detected as broker");
        assert!(c.is_configured(), "live broker socket should be configured");

        // Precedence: an explicit key outranks the forwarded socket.
        std::env::set_var("MARS_LLM_KEY", "explicit");
        assert_ne!(agent::AgentConfig::from_env().provider, "broker",
            "explicit key should outrank the broker socket");
        std::env::remove_var("MARS_LLM_KEY");

        // Honest availability: a dead socket path ⇒ not configured.
        let dead = agent::AgentConfig {
            url: String::new(), key: String::new(), model: String::new(),
            provider: "broker", max_tokens: 512, temperature: 0.3,
            broker_sock: Some("/tmp/mars-nope-does-not-exist.sock".into()),
        };
        assert!(!dead.is_configured(), "dead broker socket reported configured");

        // Proxy round-trip: chat() in broker mode returns the broker's reply,
        // and NEVER constructs a key/Authorization header on this side.
        let out = agent::chat(&c, vec![]).unwrap_or_default();
        assert_eq!(out, "broker-ok", "broker proxy did not return the reply: {out:?}");

        std::env::remove_var("MARS_AUTH_SOCK");
        let _ = std::fs::remove_file(&sock);
    }
    println!("[selfcheck] ssh broker (proxy/detect) . PASS");

    // 31. Fleet cache + `mars ls` follow-up resolver (ordinal + name/prefix).
    {
        let hosts = vec!["gpubox".to_string(), "prod-7".to_string()];
        assert_eq!(broker::resolve_target(&hosts, "2"), Some("prod-7".into()), "ordinal");
        assert_eq!(broker::resolve_target(&hosts, "gpubox"), Some("gpubox".into()), "exact name");
        assert_eq!(broker::resolve_target(&hosts, "prod"), Some("prod-7".into()), "unique prefix");
        assert_eq!(broker::resolve_target(&hosts, ""), None, "empty skips");
        assert_eq!(broker::resolve_target(&hosts, "9"), None, "out-of-range ordinal");
        // Fleet round-trip under an isolated HOME (upsert dedupes, recency orders).
        let saved = std::env::var("HOME").ok();
        let tmp = std::env::temp_dir().join(format!("mars-fleet-sc-{}", std::process::id()));
        std::fs::create_dir_all(&tmp)?;
        std::env::set_var("HOME", &tmp);
        broker::fleet_record("prod-7", None);
        broker::fleet_record("gpubox", None); // touched last → most recent
        broker::fleet_record("prod-7", None); // upsert, not a dup
        let f = broker::fleet_load();
        assert_eq!(f.len(), 2, "fleet upsert duplicated a host");
        assert_eq!(f[0].host, "prod-7", "fleet not ordered most-recent-first");
        match saved {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
    println!("[selfcheck] fleet cache + ls resolver . PASS");

    println!("\nALL SELFCHECKS PASSED ✓");
    Ok(())
}
