# Agentic Inline

*An opinionated brief on what Mars should build next, and why the answer is not
"more AI" but one specific kind of AI: the kind that can see.*

---

## The observation everyone is stepping over
|spl
Every terminal user we care about already has two things: a terminal full of state,
and an AI subscription. What nobody has is a connection between them.

Watch a competent engineer work for ten minutes and count the copy-pastes. A stack
trace gets selected, copied, pasted into a chat tab, prefaced with three sentences of
context the AI needs because it can't see the screen: *"I'm in this repo, I ran this
command, here's what happened."* The answer comes back, gets read in one window,
retyped into another. This person owns a 2026-grade reasoning engine and is employing
themselves as its clipboard.

The industry's response has been to make the AI smarter. That's solving the wrong
variable. The AI was already smart enough to fix that build failure eight months ago.
What it couldn't do — what it still can't do in a tmux pane running Claude Code,
aider, or anything else — is *look at the pane next door*. Every AI tool in the
terminal today is an amnesiac in a rectangle: brilliant, and blind.

Mars's thesis, stated plainly: **the next unit of value in developer AI is not
intelligence, it's line of sight.** Mars owns the screen — the buffers, the PTYs, the
scrollback, the layout, the session that survives the weekend. That makes it the one
place where "the AI can see what I see" is an architecture fact rather than a demo.
As of this week it's literal: every question through the bar ships with the live
screen — editor buffers, terminal contents, layout — and the model can answer with a
confirm-gated command typed straight into your shell. The substrate exists. This
piece is about what deserves to be built on it.

## Who actually shows up

Taste in features starts with honesty about users. Four people hire Mars:

**Remote Rae** lives on three SSH boxes: tmux everywhere, vim over a laggy link, a
20-minute build always running somewhere. Her real enemy isn't the editor — it's the
*seams*: two key languages, sessions that die with the Wi-Fi, and a stack trace on
box two that has to travel by clipboard to the AI on her laptop. She hires Mars to
delete seams.

**Agentic Sam** already went all-in: Claude Code in one pane, editor in another,
tests in a third. Sam's irony is exquisite — he assembled the most AI-forward setup
possible and became its message bus, shuttling errors and file context between
rectangles by hand. He hires Mars so the rectangles can see each other.

**Graybeard Priya** has thirty years of chords in her hands and zero patience for
AI theater. She hires Mars for one tool, one key language, sessions that persist —
and she is the taste-check on every feature below. If a feature would make Priya
roll her eyes, it ships behind a knob or it doesn't ship. The day her build fails
mysteriously and `?` names the exact cause from her own scrollback is the day she
stops rolling her eyes. Design for that day; never force it.

**Novice Noor** was conscripted into the terminal by a deploy script. She doesn't
know tmux exists and shouldn't have to. `Ctrl+Space` means never memorizing, `?`
means never being stuck. Noor is who the graduation engine was built for — and the
features below must teach her, not carry her.

Common thread: none of them want a chatbot. All of them want the thing on their
screen *handled*.

> **Implementation design:** the first seven of these workflows are specced in detail,
> with the enables/disables trade-off of every choice, in
> [`workflows_design.md`](./workflows_design.md).

## The slate — six features, one spine

Ranked by value against effort. Every one rides machinery that already exists —
the action registry, the screen context, the RUN/TYPE directive gates, the PTY
panes, the session daemon, the described-knob config. Nothing here is a month.

### 1. "Why did this fail?" — one key, no clipboard  *(the wedge)*

The moment: a build, test, or deploy just dumped 200 lines of failure. Today, every
tool on earth asks the user to *do something* with that output. Mars shouldn't.
One key. The agent reads the pane — including scrollback the user hasn't — and
answers in three lines: what failed, why, and a confirm-gated `TYPE:` with the next
command. Ship rule, non-negotiable: **every triage ends in a runnable command or a
cited file:line.** An explanation without a next action is an essay, and nobody
hired Mars for essays.

This is the wedge because it is *structurally impossible* for the incumbents. tmux
has no AI. The AI CLIs can't see adjacent panes. The pitch is one breath — *"your
terminal errored; press one key"* — and it lands on the highest-frequency,
highest-emotion moment in a terminal user's day.

### 2. The shell that speaks English

`! files over 100MB modified this week` — if it doesn't parse as a command, translate
it, show the real incantation in the confirm bar, type it on Enter. The `!` route
and the TYPE gate already exist; this is a fallback branch and a prompt. What makes
it Mars rather than a gimmick: cwd and recent scrollback make the translation
situational, and the shown-before-run command *teaches* — Noor learns `find` by
reading what she approves. The graduation doctrine, applied to shell literacy.
Never auto-run. The showing is the product.

### 3. Jump-to-error — the `OPEN:` directive

`src/session.rs:412: assertion failed` scrolls past. One key later the cursor is on
line 412 in a split. One new directive, parsed like TYPE, handled like RUN — and a
deliberate down payment on the parameterized-actions substrate the vision doc calls
the #1 blocker. The AI earns its place here doing the thing regex tmux plugins
can't: picking the *right* frame out of a 40-line traceback — your code, not the
framework's.

### 4. The config concierge — proof of the "new Elisp"

`? make autosave every 10 seconds and double the scrollback` → a diff of
tuning.json → apply on confirm → every hint surface updates instantly (the honesty
invariant does the work). This is nearly free — every knob was given a plain-English
description *precisely so an agent could edit it safely* — and it's the punchline of
the whole extensibility thesis: Emacs users extended their editor by learning Elisp;
Mars users extend theirs by asking. Ten-second demo. Ship it for launch day.

### 5. Watch this pane — the first proactive feature

`> watch` on the build pane. The agent notices exit-or-quiet, reads the tail, posts
*one status line*: "build failed — linker error in session.rs (pane 3)". Because
PTYs are parsed on daemon threads whether anyone's attached, this works **while
detached** — start a build, close the laptop, and the session watched it for you.
tmux structurally cannot offer that sentence. This one sets the trust precedent for
all future proactivity, so it obeys the interruption budget with religious
strictness: never modal, never a chime, silence when nothing happened. Err quiet.
The first false alarm costs more than the feature earns.

### 6. The reattach briefing

`mars attach` after lunch: three prioritized lines — failures first, then changes,
then nothing. "While you were away: build in pane 2 failed; dev.log grew 3 errors;
notes.md has unsaved changes." Scannable in two seconds, dismissed by any key, and
*absent* when nothing changed. Sessions already made state survive absence; this
makes *understanding* survive absence.

## What we will not build — taste is subtraction

**Ghost-text completion.** Commodity, latency-hostile on free tiers, months of work
in someone else's moat — and visual noise in an editor whose whole aesthetic is
calm. Mars's differentiation is the workspace, not the keystroke.

**Inline "rewrite this function" edits.** Not forever — *for now*, and for a reason
the vision doc already wrote down: multi-file agent edits without the transaction
journal means no one-chunk undo, no one-chunk undo means broken trust, and a user
burned by an un-undoable AI edit does not come back. Ship the journal, then revisit.
Until then, Sam runs Claude Code in a Mars pane and both tools do what they're for.

**A generic chat pane.** A ChatGPT rectangle without screen context is strictly
worse than the ChatGPT tab the user already has. Every agent surface in Mars is
workspace-aware or it doesn't exist. This is the line that keeps "AI-forward" from
decaying into "AI-flavored."

## The loop

Why the wedge compounds instead of churning:

Something breaks — daily, guaranteed — and one key resolves it. Habits formed on
pain triggers are the durable kind. Every rescue cites the command and the binding,
so the graduation engine converts rescues into fluency: **the user gets faster, not
more dependent** — the exact opposite of the learned helplessness most AI tools
cultivate. The rescue only works on panes that live inside Mars, so shells migrate
in, which widens the agent's field of view, which improves the answers: the
workspace itself becomes the context moat. And those panes sit in sessions that
survive disconnects, so leaving Mars now means abandoning live state *and* an agent
that knows your situation. That's tmux's lock-in, plus sight.

Sequence: **1 and 2 together** (same machinery, one sprint — the reactive rescue and
the proactive incantation are the two halves of terminal pain), **4 for launch day**
(the demo), then 3, 6, 5 — the seeing-agent extended across space, then time.

Six features. One sentence to remember all of them by:

**Mars is the first terminal where the AI can see — and the point of an AI that can
see is that you stop being its clipboard.**
