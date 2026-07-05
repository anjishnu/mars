# Self-Reflection: Mars's Agent Learns to See — and Edit — Itself

*Design doc for giving the built-in agent introspective capabilities: answering
questions about Mars accurately (a knowledge repo fed into its context) and making
internal changes safely (theme, keybindings, tuning) through the existing directive
seam.*

Status: proposal. Grounded in the v0.1.0 codebase as of 2026-07; every mechanism cited
below exists at the file:function given.

---

## 1. First principles — why an editor must know itself

Mars's central bet (key_design.md §0) is that **natural language is the new Elisp**:
users extend and operate the editor by asking the agent, and the config surfaces
(keys.json, tuning.json) are data *precisely so the agent can read and edit them*.
That bet is already half-cashed — the decision log records tuning.json as "the first
agent-editable config surface (a Context-Bus precursor)." This doc designs the other
half: the loop where the agent actually reads and writes those surfaces on request.

Three first-principles reasons this is not a nice-to-have:

1. **Discoverability is trust.** Mars's whole pedagogy (the graduation engine,
   which-key, the honesty invariant) rests on the editor never lying about itself.
   The agent is one of the four actors (key_design.md §0) and currently the *only*
   surface that can lie: it answers "how do I X in Mars?" from model priors, not from
   the registry. A user who gets one hallucinated keybinding from the agent stops
   trusting every hint surface — the exact failure mode the honesty invariant
   (key_design.md §2) exists to prevent. The agent must be held to the same standard
   as a menu row: **derived from the live keymap, structurally incapable of lying.**

2. **The agent is the settings UI.** Mars deliberately has no preferences dialog;
   config is two JSON files (`~/.config/mars/keys.json`, `tuning.json` —
   `main.rs:HELP` documents this). Users arriving from Cursor and Claude Code will
   type "change my theme to something blue" and expect it to work. Today that request
   dead-ends: the agent has no directive that can touch config. The knob file was
   *designed* for this — every knob in `tuning.rs:default_knobs()` carries a
   `description` field whose stated purpose (tuning.rs header comment) is that "a
   human or an agent editing the file can see what each number does." The write path
   is the missing 20% of a mechanism that is 80% built.

3. **Knowing your limits is a capability.** An agent that says "Mars can't do that
   yet — closest thing is `WatchPane`" is more useful than one that invents a
   plausible feature. Honest refusal requires a ground-truth capability manifest, and
   Mars uniquely has one: the `Action` enum (palette.rs) plus `binding_for()`
   (config.rs) plus `default_knobs()` (tuning.rs) *is* the complete, typed, always-
   current answer to "what can Mars do?" — no prose needs to be trusted. The design
   below leans hard on deriving from code, treating hand-written docs as second-class
   evidence.

The safety story is also already half-built, which keeps this proposal lean:
`mars reset` (main.rs, `config::reset_keys()` + `tuning::reset()`) restores defaults
with `.bak` backups; `Tuning::load()` (tuning.rs) self-heals — unknown keys are
ignored and unparseable values fall back to per-field defaults; and
`RawBindings::into_bindings()` (config.rs) refuses to let a broken `bar_open` lock
the user out. A bad agent write is therefore already recoverable *before we add
anything*. Per key_design.md §3.4: reversibility, not permission dialogs, is what
makes autonomy adoptable.

---

## 2. Top 10 self-reflective features, ranked

Ranked by impact × frequency × trust-building. The top four are the product; the rest
compound it.

1. **Capability Q&A — "how do I split the screen?"** *(daily; the core loop)*
   Already the ask-bar's main job, but today's answers are grounded only in
   `registry_context()` — which (checked: palette.rs:registry_context) contains
   action names, labels, and descriptions but **no keybindings**. key_design.md §3.6
   requires every agent answer to cite "the command *and its binding*"; the agent
   literally cannot comply today. Fixing this (one line: join `binding_for()` into
   the registry text) plus doc retrieval (§3 below) upgrades the agent from
   tip-of-the-tongue rescue to the editor's help system.

2. **Theme/appearance editing by natural language.** *(first-session wow; the
   Cursor-refugee litmus test)* "Make the accent orange," "give me a blue theme."
   Five `theme_*` RGB knobs + `selection_bg`/`search_match_bg` already exist as
   described knobs (tuning.rs:default_knobs). An LLM is genuinely good at
   name→RGB. This is the single most demo-able feature and exercises the whole
   write path (§4).

3. **Keybinding remap by natural language — "make cmd-p open the file finder."**
   *(weekly; deep trust-builder)* The honesty invariant makes this uniquely safe in
   Mars: after a rebind, every menu row, which-key panel, and nudge updates
   automatically because all hints render through `KeyBindings::binding_for()`
   (config.rs) at draw time. The validator already exists (`parse_sequence`,
   config.rs) and live reload already exists (`Action::RestoreKeybindings` handler,
   app.rs:3228 — `self.keys = config::load()`, no restart).

4. **The capability manifest — never hallucinate a feature.** *(invisible;
   multiplies trust in everything else)* A tiny always-injected header derived from
   code (version, directive vocabulary, config paths, action count) ending with an
   explicit instruction: *if it's not in the registry or this manifest, say Mars
   can't do it.* Cheap (~150 tokens) and it converts every "no" into evidence the
   agent's "yes" can be trusted.

5. **Tuning behavior by natural language — "autosave more often," "make the watch
   quieter."** *(monthly; long-tail power)* Same write path as #2, different knobs.
   The descriptions are the retrieval surface: "watch quieter" → the agent reads
   `watch_quiet_secs`'s description ("Seconds a watched terminal must be silent
   before Mars summarizes it") and proposes the right knob. The knob file was
   written for exactly this reader.

6. **Undo/revert agent-made config changes.** *(rare but existential)* One-chunk
   recovery (key_design.md §3.4) applied to config: every agent write snapshots a
   `.bak` first, and an `UndoConfigChange` action swaps it back. Without this,
   features #2/#3/#5 are adopted timidly; with it, users experiment freely.
   `mars reset` remains the nuclear backstop.

7. **Self-diagnosis — "why is the agent slow / not working?"** *(rare; rescues the
   worst moments)* Half-built: `agent.rs:chat()` already translates 429s into
   "rate limit reached — wait ~14s, or switch model with MARS_LLM_MODEL" and
   401/403 into "check your API key." What's missing is making provider/model/key
   state and the last error *queryable* — a derived "agent status" section in the
   knowledge pack. When the agent itself is down, this must degrade to a local
   answer (the status line / `mars ask`'s error path already prints actionable
   text without a model round-trip).

8. **Action transparency — "what did you just do / why?"** *(grows with autonomy)*
   A small ring-buffer journal of fired directives and actions (actor, action,
   timestamp, outcome), injectable on demand. Today's agent is single-shot and
   confirm-gated so the user *saw* everything; this becomes load-bearing when W6
   watches and future multi-step runs act while the user looks away. The journal is
   also the seed of key_design.md §6.2's transaction journal — design the entry
   format once. (`AwayDigest`, palette.rs, is the same data need from the human
   side.)

9. **Agent-driven onboarding tour.** *(once per user)* "Show me around" → a scripted
   sequence of Q&A turns grounded in the manifest + registry, each answer citing a
   real binding. No new machinery — it's capability Q&A (#1) with a curated opening
   prompt. Ships free once #1 and #4 land.

10. **"What changed in this version?"** *(once per release)* A `CHANGELOG` section
    embedded in the knowledge pack at build time, keyed by `CARGO_PKG_VERSION`.
    Lowest frequency; include because it costs one `include_str!` once the pack
    (§3) exists.

Explicitly deferred: agent edits to its *own* prompt/provider config beyond described
knobs (`agent_max_tokens`, `agent_temperature` are knobs and thus already covered;
API keys are env vars and stay out of reach — never write secrets).

---

## 3. The knowledge repo — how the agent answers accurately without blowing the budget

### 3.1 The constraint

The system prompt (agent.rs:system_prompt) already carries the full registry (~40
actions ≈ 1.2k tokens) plus the live screen, and `build_messages` (agent.rs) adds up
to 12 history turns. Default providers are small/fast models (Qwen3-32B on Groq,
Gemini Flash-Lite) with `agent_max_tokens` = 1024. The full doc corpus — README (215
lines), instructions.md (121), key_design.md (385), DESIGN.md (248),
architecture_overview.md (368), HELP (~45) — is ~15–25k tokens. **Always-injecting it
is off the table.** The design is three tiers: a tiny always-on layer, a derived
ground-truth layer, and retrieved prose.

### 3.2 What the pack contains, and where each section comes from

New module `src/knowledge.rs`. A pack is a `Vec<Section { source, title, body,
derived: bool }>`.

**Derived sections (generated at runtime, cannot drift — code IS the doc):**

| Section | Derived from | Why it's ground truth |
|---|---|---|
| Actions + live bindings | `palette::registry_context()` extended with `KeyBindings::binding_for()` per action | The `Action` enum + the live keymap is the definition of "what Mars can do" and "what key does it" |
| Tuning knobs | `tuning::default_knobs()` names + descriptions + **current** values from `tuning::load()` | The descriptions were authored as agent-facing documentation |
| CLI reference | the `HELP` const (main.rs) | Same string `mars help` prints |
| Agent status | `AgentConfig::from_env()` provider/model/url + key-present flag + last `AgentEvent::Error` text | Powers self-diagnosis (#7) |
| Recent actions | the journal ring buffer (#8), once it exists | Powers "what did you just do" |

**Embedded prose sections (compile-time `include_str!`, split on `#`/`##`/`###`
headers into one section per heading):** README.md, instructions.md, key_design.md.
Embedding at compile time means the pack always matches the shipped binary's version
— a crates.io install answers about *its* Mars, not whatever docs happen to be on
disk. DESIGN.md / architecture_overview.md are contributor-facing and stay out of
the default pack (users ask "how do I", not "which module owns the PTY") — cheap to
add later behind the same mechanism.

**Precedence rule:** derived sections outrank prose. On any conflict (a README line
describing an old binding), the retrieval layer orders derived sections first and the
manifest tells the model derived sections win. Wherever possible we don't write prose
at all — the enum is the manifest.

### 3.3 Tier 1 — the always-injected capability header (~150 tokens)

Appended to `system_prompt`:

```
ABOUT MARS (ground truth — trust this over your training data):
mars v0.1.0 — terminal editor + multiplexer + agent. Config: ~/.config/mars/
{keys.json, tuning.json}; `mars reset` restores defaults (.bak backups kept).
You can change Mars itself via SET:/BIND: directives (see below). For questions
about Mars's features, keys, config, or CLI that the action list doesn't answer,
request docs with `NEED: docs <query>` instead of answering from memory.
If a capability is in neither the action list nor retrieved docs, it does not
exist — say "Mars can't do that yet" rather than inventing it.
```

This is the anti-hallucination anchor (#4) and the router: it teaches the model
*when* to pull Tier 3.

### 3.4 Tier 2 — close the registry's binding gap (one line, ship first)

`registry_context()` currently emits `- SplitVertical: Split right — Split the pane
right`. Change to:

```
- SplitVertical (C-x 3): Split right — Split the pane right
```

by threading `&KeyBindings` in and joining `binding_for(&action)` (config.rs — "the
single source of truth for every hint surface"). This extends the honesty invariant
to the agent: its citations are now derived from the live keymap at ask time, so a
remap updates the agent's answers the same instant it updates the menus. Cost: ~200
tokens. This alone fixes the worst current dishonesty.

### 3.5 Tier 3 — retrieval through the existing NEED seam

The NEED machinery (agent.rs: `NeedKind::{Scrollback, Tab}`; "Mars re-asks once with
the extra source", never user-gated, not shown to the user) is already the model-
driven context-selector chosen in workflows_design.md over always-dumping. Extend it:

- **New variant:** `NeedKind::Docs(String)` — parsed from `NEED: docs <query>` in
  `match_directive` (agent.rs), advertised in the system prompt next to
  `NEED: scrollback`.
- **Retrieval:** lexical keyword scoring, no embeddings — key_design.md §6's
  retrieval ladder is explicit that embeddings don't pay rent below ~1k items, and
  the pack is ~60–80 sections. Score per section = Σ over lowercased query terms of
  (3 × title hits + 1 × body hits), × 2 if `derived`. Take top sections until a line
  budget is hit; budget is a described knob (`agent_docs_context_lines`, default
  ~120 — tuning.rs convention: behavioral numbers are named knobs, not literals).
- **Injection + re-ask:** the winning sections are appended as a
  `MARS DOCS (retrieved for "<query>"):` block and the question is re-asked once,
  riding the same one-shot re-ask guard the W4/W5 NEEDs use. Failure mode is
  honest: if nothing scores, the block says "no documentation matched — if unsure,
  say so," which combined with the header's refusal instruction produces "Mars
  can't do that yet" instead of confabulation.

Why model-driven pull instead of Mars-side classification ("does this question
mention Mars?"): the model sees the question *and* the screen and already
demonstrates the request-more pattern with NEED; a Mars-side keyword trigger would
misfire both directions. The header (Tier 1) is what makes the pull reliable — it
names the tool and the rule for using it.

### 3.6 Staying honest as features change

- Derived sections **cannot** go stale — they're computed from the same enum/keymap/
  knob-map the editor runs on, at ask time.
- Prose sections match the binary by construction (`include_str!`).
- One new `--selfcheck` case keeps prose from rotting *within* a release: extract
  every backtick-quoted `Action`-shaped token from the embedded docs and assert
  `Action::from_name()` resolves it (palette.rs) — a doc that names a deleted action
  fails the build's test suite. (`--selfcheck` is already the enforced post-change
  gate per AGENTS.md.)

---

## 4. The write path — the agent edits Mars, safely

### 4.1 Two directives, riding the existing seam

The directive vocabulary (trailing line, parsed by `match_directive`, confirm-gated,
model-portable — the recorded ruling from workflows_design.md) gets two verbs:

```
SET: <knob> = <value> [; <knob> = <value> …]     e.g.  SET: theme_accent = #ff8800
BIND: <chord sequence> = <ActionName>            e.g.  BIND: cmd-p = QuickOpen
```

Two `AgentDirective` variants (agent.rs): `Set(Vec<(String, serde_json::Value)>)`
and `Bind(String, String)`. The multi-pair `SET` form exists for one reason: "change
my theme to solarized" is five knobs, the protocol allows exactly one directive per
reply (agent.rs:parse_directive), and five confirm round-trips would be hostile. One
`SET` line = one confirmation = one `.bak` = one undo chunk — the transaction shape
of key_design.md §3.3 in miniature.

### 4.2 Validate before write — the parser is the gatekeeper

Nothing touches disk until the proposal parses. All validators already exist:

- **BIND:** `parse_sequence(chord)` must succeed (config.rs — the same parser the
  live keymap uses, so "if it validates, it binds") and `Action::from_name(name)`
  must resolve (palette.rs). If the sequence already maps to something, the confirm
  line says what it displaces (`lookup`, config.rs). Rebinding `bar_open` chords
  gets an extra warning, though config.rs:into_bindings already guarantees the bar
  can never become unreachable.
- **SET:** knob name must exist in `default_knobs()` (tuning.rs) — unknown knobs are
  rejected with the nearest match suggested, never written (even though `load()`
  would ignore them: a silent no-op write is a lie to the user). Value must
  type-check against the default's JSON shape; colors accept `#rrggbb` or
  `[r,g,b]` and normalize to the array form `get_rgb` (tuning.rs:load) reads.
  Numeric floors that `load()` enforces (e.g. `poll_interval_ms.max(1)`) are applied
  at validation so the confirm bar shows the value that will actually take effect.

An invalid directive degrades exactly like a malformed `RUN:` today: the answer text
still displays, no action bar appears, and the status line says why.

### 4.3 Confirm — config writes are always gated

`RUN:` gates only destructive actions (`Action::is_destructive`, palette.rs). SET and
BIND gate **unconditionally**: they persist across sessions, which is a bigger blast
radius than any single destructive action, and key_design.md §3.1–3.2 demands
visibility-before-action and inert-by-default. The proposal renders in the existing
`▶ Enter to run` bar (the same `PromptKind::ConfirmAction`/pending-refactor pattern,
app.rs) showing the *diff*, not just the target:

```
▶ Enter: theme_accent  #D97757 → #FF8800    (Esc / C-g cancels)
▶ Enter: bind cmd-p → QuickOpen  (currently: C-x p; cmd-p was unbound)
```

### 4.4 Apply — reuse the load/save/self-heal machinery, then live-reload

On confirm:

1. **Snapshot:** copy the target file to `keys.json.bak` / `tuning.json.bak` — the
   same convention `reset_keys()` (config.rs) and `tuning::reset()` already
   established, so `mars reset`'s messaging ("backed up alongside as *.bak")
   stays true.
2. **Write surgically:** read the JSON file (writing annotated defaults first if
   absent — `write_default_knobs`, tuning.rs), mutate only the targeted entries
   (preserving each knob's `description`), write pretty-printed. Never regenerate
   the whole file from struct state: user hand-edits and comments-via-description
   survive.
3. **Live reload:** `self.tuning = tuning::load()` / `self.keys = config::load()` —
   the exact pattern `Action::RestoreKeybindings` already ships (app.rs:3228,
   "apply immediately, no restart"). For keys, the honesty invariant then does the
   rest *for free*: every menu badge, which-key panel, and graduation nudge renders
   from `binding_for()` at draw time, so the new binding is visible everywhere on
   the next frame — no cache to invalidate, no surface to forget.
4. **Journal + notify:** append to the action journal (#8) and post a status-line
   notice: `theme_accent set — undo: "undo config change" in the bar`.

If the write itself fails (disk, permissions), the `.bak` is untouched and the error
surfaces in the status line — never a silent partial state.

### 4.5 Revert — three rungs of recovery

1. **`Action::UndoConfigChange`** (new, in the palette per the new-action checklist
   in AGENTS.md: menu entry, `label()`, not destructive, `run_action` arm): swaps
   the config file with its `.bak` and reloads — so it is its own inverse (undo the
   undo = redo). Scope: last write per file, which matches the `.bak` depth that
   `mars reset` established; a deeper history waits for the §6.2 transaction
   journal rather than inventing a parallel one now.
2. **`RUN: UndoConfigChange`** — the agent can propose the revert ("actually, put it
   back"), gated like any RUN.
3. **`mars reset`** (main.rs) — the existing factory backstop, unchanged, still
   advertised in HELP.

### 4.6 What the agent may not touch

Env vars (API keys — secrets stay out of every write path), `state.json`
(frecency/nudge counters are the *user's* practice record), session files, and
arbitrary paths — SET/BIND address knobs and chords by name only; there is no
file-path parameter anywhere in the grammar, so the write surface is closed by
construction.

---

## 5. Final proposal — build order and seams

Lean by design: zero new subsystems. One new module (`knowledge.rs`), two directive
variants, one action, one knob.

**P1 — Honest citations + the write path** *(the visible product: features #1-half,
#2, #3, #5, #6)*

1. Thread live bindings into `registry_context()` (palette.rs + call sites in
   app.rs / main.rs:ask_cli). *Selfcheck:* registry text contains `(C-x 3)` next to
   `SplitVertical`; rebind, regenerate, assert it updated.
2. `AgentDirective::{Set, Bind}` + parsing in `match_directive` + system-prompt
   lines (agent.rs). *Selfcheck:* parse good/bad SET and BIND lines — bad knob, bad
   color, bad chord, unknown action all rejected.
3. Confirm-gated apply path in app.rs (validate → `.bak` → surgical write → reload →
   notice), `Action::UndoConfigChange` with its full palette surface. *Selfcheck:*
   in the temp config dir the suite already uses (main.rs:selfcheck), fire a SET,
   assert tuning.json changed + `.bak` exists + `self.tuning` updated; fire a BIND,
   assert `binding_for()` reports the new chord (extending the existing "bar fuzzy
   + live binding" case); run UndoConfigChange, assert restoration.
4. Files touched: `agent.rs`, `palette.rs`, `app.rs`, minor `config.rs`/`tuning.rs`
   helpers (single-entry write).

**P2 — The knowledge repo** *(features #1-full, #4, #7)*

5. `knowledge.rs`: section type, `include_str!` prose ingestion + header splitting,
   derived sections (registry, knobs+current values, HELP, agent status), keyword
   scorer. *Selfcheck:* "how do I detach a session" retrieves the sessions section;
   a nonsense query retrieves nothing and says so; doc-mentioned action names all
   resolve via `Action::from_name`.
6. Capability header appended in `system_prompt`; `NeedKind::Docs(String)` + the
   one-shot re-ask (agent.rs, app.rs NEED handler); `agent_docs_context_lines` knob
   (tuning.rs). *Manual pass:* `mars ask "can Mars edit over ssh?"` should answer
   "not yet" — the refusal test.

**P3 — Transparency** *(features #8, #9, #10; sequenced last because they ride P2's
pack)*

7. Action journal ring buffer in app.rs (also feeds `AwayDigest`), exposed as a
   derived section + `NEED: recent`; CHANGELOG `include_str!`; a curated
   onboarding prompt behind an `Action::TourMars` palette entry.

**Non-goals (recorded):** embeddings or any index build (retrieval ladder, §6
key_design.md); a settings UI; agent-initiated (unconfirmed) config writes; profile
switching (`emacs.json`/`vscode.json` keymap profiles are the zoning-law roadmap and
arrive as data files, at which point BIND inherits them for free); editing
`state.json` or env secrets.

The through-line: Mars already owns the two hard assets — a typed, complete
capability registry and a self-healing, described, reset-safe config surface. This
proposal never duplicates them into prose; it wires the agent to read the first and
write the second, with the honesty invariant doing the propagation and `mars reset`
holding the floor.
