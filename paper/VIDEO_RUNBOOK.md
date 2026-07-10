# MARS demo video — 2:30 runbook

Mirrors the paper's narrative: generic + amnesiac assistant → embedded agent with local
memory → corrective memory (the headline) → self-knowledge/reconfigure → persistence →
key broker → ownership close. Six scenes; record each separately and cut together.

## Pre-flight (do once, ~30 min before recording)

- [ ] `cargo build --release`; verify `./target/release/mars --selfcheck` passes.
- [ ] Terminal: 120×32, 16–18 pt font, dark theme, hide the OS menu bar. 1080p capture
      (QuickTime or OBS). Record voiceover separately; scratch-narrate while recording
      to keep pace, replace in the edit.
- [ ] Env: `export ANTHROPIC_API_KEY=...` and `MARS_LLM_MODEL=claude-haiku-4-5` (the
      paper's model; ~1–2 s latency, no free-tier stalls mid-take).
- [ ] Demo project: a repo whose commands are unguessable from context — the eval's
      trap is ideal: `python -m benchmarks.run` for "run the benchmark suite",
      `python train.py --config configs/base.yaml` for "run the training job".
      Create stub files so the commands actually run and print something.
- [ ] **Dry-run the corrective beat now.** Cold-ask your chosen request once; confirm
      the model actually gets it wrong live (on-screen cwd context can tip it right —
      if so, pick a harder request). Then delete `~/.mars/cmd_memory` so the recorded
      take starts cold.
- [ ] Kill stale sessions: `mars ls`, `mars kill <name>` until clean.
- [ ] Reset `~/.config/mars/tuning.json` to defaults (the accent-color beat changes it).

## Shot list

### Scene 1 — Hook (0:00–0:15)
Screen: title card 2 s → `cargo install mars-terminal` → `mars` launches into editor +
shell pane.
> "Terminal assistants are generic and amnesiac: they don't know your project's
> commands, and they forget your corrections. MARS is a terminal that remembers.
> One binary: editor, multiplexer, agent."

### Scene 2 — The bar (0:15–0:35)
Screen: `Ctrl+Space`, type a few letters → fuzzy action search; then `!ls` (shell),
then `?` prefix (agent). Keep it brisk — three keystroke demos, no dwelling.
> "One command bar. Plain text finds actions, bang runs shell, question mark asks the
> agent — and anything that matches no action falls through to the agent as plain
> English."

### Scene 3 — Corrective memory: the headline (0:35–1:15) ← the money shot
Screen, in the demo project:
1. `Ctrl+Space` → "run the benchmark suite" → agent proposes `cargo bench` (wrong).
2. Edit the proposal in place to `python -m benchmarks.run` → Enter → it runs.
   (The accepted correction enters `~/.mars/cmd_memory` — say so.)
3. `Ctrl+Space` → **reworded**: "kick off the benchmarks" → proposes
   `python -m benchmarks.run`. Pause one beat on the correct proposal.
> "Watch it be wrong. I fix the command once — and that correction is stored locally.
> Now I ask again, in different words. Right. No fine-tuning, no embeddings: BM25 over
> a local file. In our leakage-controlled eval this takes a small model from 38 to 96
> percent on project-specific commands."

### Scene 4 — It knows itself (1:15–1:40)
Screen: `?how do I split the screen` → answer with the real keybinding; then
`?make the accent orange` → proposes the `theme_accent` edit in `tuning.json` →
confirm → **the UI accent changes live** (end the shot on the color change).
> "The same retrieval, pointed at the tool's own docs and registry: real keybindings,
> not guesses — and plain-English reconfiguration. Sixteen to ninety-three percent on
> that task."

### Scene 5 — Persistence + the key stays home (1:40–2:10)
Screen: detach (`C-x C-d`) → close the terminal window entirely → new terminal →
`mars attach` → everything returns, agent conversation included. Then a quick
`mars ssh gpu-box` clip: ask the agent something on the remote; cut to a one-line
overlay: **"API key never left the laptop — calls proxy home over SSH."**
> "A daemon keeps the session — and the memory — alive across disconnects. And on a
> remote GPU box, the agent works without exporting your API key: requests tunnel
> home, the key never lands on the box."

### Scene 6 — Close (2:10–2:30)
Screen: side-by-side of `~/.mars/cmd_memory` (plain JSON lines) and the editor;
end card: `cargo install mars-terminal` + paper title.
> "Your memory is a local file you can read, edit, and own. Simple memory works
> surprisingly well — and the same substrate points at agents that learn your skills
> and understand your work. MARS is on crates.io."

## Timing + fallbacks

- Voiceover budget ≈ 350 words total (≈2.4 words/s). The script above is ~330.
- Scene 3 is the only take with real failure risk (the model may be right cold).
  Fallbacks, in order: (a) harder request ("deploy the model"), (b) `cd /tmp` first so
  no project context leaks, (c) worst case, cut the cold miss from a dry-run take —
  never fake the proposal itself.
- If a model call stalls >4 s on camera, cut and re-take the scene; don't wait it out.
- Trim dead air between keystroke and proposal to ~1 s in the edit; keep one honest
  pause on each correct proposal so viewers can read it.
- Export: 1080p H.264 MP4, target <100 MB; check the venue's upload limit.
