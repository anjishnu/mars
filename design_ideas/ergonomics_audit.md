# Mars ergonomics audit (2026-07) — ranked punch-list

*Fresh-eyes sweep judged against Mars's own doctrine (key_design.md): recognition-over-recall,
the honesty invariant, interruption budget, spatial stability, "the costlier the error, the
taller the gate." Analysis only — nothing here is implemented yet. P0 claims spot-verified
in code (close_pane/close_tab touch `terms` zero times; zero `C-g` arms in bar handlers).*

## P0 — severe

**P0.1 — Closing a pane/tab with a live terminal silently orphans the running PTY, no confirm.**
`close_pane` (app.rs:1507), `close_tab` (:1636), `delete_other_windows` (:1226) never touch
`self.terms` — cleanup exists only in `close_terminal_pane` (:3525), which runs only for
already-exited shells. The comment at :1231 ("any terminal owned by that pane is dropped with
it") is false. The shell keeps running, unreachable forever; its watch can still fire notices
for a pane that no longer exists. AGENTS.md promises destructive actions are confirm-gated "for
direct user input" too — but `Action::is_destructive` gates only the agent RUN: path (:2562).
*Fix: reap Terms whose panes vanish + confirm on user-initiated close when a live terminal is inside.*

**P0.2 — Space-warp mode puts unconfirmed destruction next to navigation.**
`d` closes a tab (:2302) beside `h`/`l` switching; `0` closes a pane (:2335) on the same digit
row as `1-9` tab-jump; `D` (detach) vs `d` (close) differ by a Shift. Holding `d` machine-guns
through every tab; the last close triggers quit. Maximal error cost exactly where slips are
likeliest. *Fix: confirm (or double-press) for d/q/0; move close-pane off `0`.*

**P0.3 — "C-g cancels anything" is false in the command bar.**
Promised in README:47, HELP, splash, idle bar — but `handle_bar_command`/`_ask`/`_shell` have
no C-g arm; it's silently swallowed. The one overlearned recovery chord is dead on the most-used
surface. *Fix: C-g = Esc in all three bar submodes.*

## P1 — significant

**P1.1 — `binding_for` teaches chords that don't work on the user's terminal.** Shortest-then-
lexicographic pick (config.rs:105) yields Save → "⌘-s", Select-all → "⌘-a" (kitty-only), Split
→ "C--" (arrives as C-_ = Undo on legacy), Search → "C-r". Every teaching surface (status bar,
dropdown badges, nudges) inherits the lie. *Fix: capability-tiered, canonical-preferring pick.*

**P1.2 — Notice "Esc dismiss" hint lies when a terminal pane is focused** — Esc is sent to the
shell (app.rs:3513); dismissal is reachable only via C-g-then-Esc, hinted nowhere. Watch
verdicts mostly appear over terminals. *Fix: intercept Esc-with-notice in handle_terminal, or
mode-aware hint.*

**P1.3 — Reattach digest hint "C-x g" is dead in the most common reattach state** (focus lands
in a terminal; prefix chords aren't intercepted there — the bytes go to bash). *Fix: mode-aware
hint ("C-g, then C-x g") or make AwayDigest chrome-tier.*

**P1.4 — A plain click in a terminal clobbers the clipboard** — mouse-up copies even a
zero-drag 1-char selection (app.rs:4171). *Fix: require anchor != end.*

**P1.5 — Ctrl+Space dead in Tree/Prompt/Warp/Undo modes** — "one gesture rules everything"
fails intermittently by mode. *Fix: handle bar_open at the top of handle_key before dispatch.*

**P1.6 — Terminal scroll offset winds into phantom scrollback** — clamped to the 10k limit,
not actual history (terminal.rs:139); title shows false "↑500"; wheel-down feels frozen.
*Fix: clamp to vt100's real scrollback length.*

**P1.7 — "Detach" means two things** — TERM hint "C-g detach" (unfocus pane) vs Action::Detach
(disconnect client). Also mode.rs hardcodes "C-Spc". *Fix: rename hint to "to editor"; derive
from keys.bar_open.*

**P1.8 — No mouse drag-selection in editor panes** despite key_design §7's recorded ruling
("Selection = Shift+arrows + mouse"). Drag does nothing; users conclude selection is broken.

**P1.9 — TabMode and KillBuffer invisible to menu + agent registry** — not bar-searchable, not
RUN:-able; violates "a capability that exists for one actor exists for all four."

**P1.10 — Critical failures ride status_msg, which any keypress erases** — "⚠ autosave FAILED"
can vanish unread mid-typing-burst (handle_edit clears it at :1705). *Fix: failures → notices
queue.*

## P2 — polish
- Bar-mode hints static/wrong per submode (Tab=translate in Shell; C-l only in Ask; Esc pops submenus).
- Splash hardcodes five chords (bypasses binding_for); shown in standalone where detach can't work.
- Menu descriptions hardcode keys ("(also C-t z)", "(also M-1..9)", "(also @)").
- Provider teaching inconsistent — overlay/translate errors say "set GEMINI_API_KEY" only.
- Warp-mode rule drift: digit-jump exits, h/l stays; panel omits +/= and s/v/\ aliases.
- `run_shell_command` types into whatever owns the reused terminal (vim!) and doesn't scroll_to_live.
- `agent_scrollback_context` (200) mostly a no-op — watch reads the visible screen (~40 lines), not history_tail.
- Away-digest durations tick-derived (early recv returns inflate them); 200-cap truncation is silent.
- Tab bar has no overflow handling — active tab can render off-screen.
- Notices dequeue one Esc at a time; "(+N more)" hides the queue.
- `DeleteOtherWindows` missing from is_destructive.
- Graduation nudge never backs off (ratio stored, never read).
- Status messages teach dead CLI ("mars --session <name>") vs the real `mars new`.
- Mouse dead outside Edit/Terminal (no wheel in ask panel/tree; tree rows unclickable).

## Fix-first five
1. P0.1 (orphaned PTYs + close gates) 2. P0.2 (space-warp slip-destruction) 3. P0.3 (C-g in bar)
4. P1.1 (capability-tiered binding_for — every surface inherits it) 5. P1.2+P1.3 (mode-aware hints).

## The systemic risk (one paragraph)
The honesty invariant is enforced at exactly one layer — which chord maps to which action — and
is honest there to a fault. But a hint is only true if the chord can physically reach Mars from
where the user is standing, which depends on two dimensions the hint system is blind to:
**terminal capability tier** (⌘-s / C-- on legacy terminals) and **current mode** ("Esc dismiss",
"C-x g digest" rendered over a terminal that swallows both). The same one-layer thinking shows in
safety: `is_destructive` gates the agent but not the human, so the tallest gates guard the rarest
actor. One concept, applied twice, fixes the class: hints and gates must be computed against the
*situated* action — this mode, this pane, this terminal — not the abstract keymap.
