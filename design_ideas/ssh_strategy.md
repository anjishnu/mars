# Mars — SSH Secret Strategy

*How a developer supplies their LLM key **once**, on their home machine, and has Mars's
agent work on every box they SSH into — without ever copying the key to a remote host,
and without a plaintext key landing in `~/.bash_history` or `~/.bashrc` on machines they
don't fully trust.*

*Companion to [`strategy.md`](./strategy.md) scenario #2 ("Remote/SSH dev — the daemon +
agent run on the box"). This document is about the one thing that scenario quietly
assumes and today does not deliver: that the agent has a key **where it runs**.*

---

## 1. The core UX principle: **supply once, works everywhere**

A secret is a fact about *you*, not about a machine. You should tell Mars your key exactly
once — on the machine that is *yours* — and every Mars you ever attach to should inherit
that fact automatically, the way `ssh -A` lets every box you hop into borrow your identity
without your private key ever leaving the laptop.

Today Mars violates this at the root. `AgentConfig::from_env()` (`src/agent.rs`) reads the
key out of the **process environment of whatever machine the daemon runs on**:

```rust
// src/agent.rs — from_env()
if let Ok(k) = env_var("LLM_KEY") { … }               // MARS_LLM_KEY / ARES_LLM_KEY
else if let Ok(k) = std::env::var("GROQ_API_KEY") { … }
else if let Ok(k) = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY")) { … }
```

And the actual API call (`chat()`) fires from **that same machine**:

```rust
// src/agent.rs — chat()
ureq::post(&url)                                       // url = cfg.url on the box
    .set("Authorization", &format!("Bearer {}", cfg.key))   // key on the box
    .send_json(body)
```

Per scenario #2 the daemon *and* the agent run on the remote box (that is the whole point —
sight and persistence live where the state lives). So the key must be **on the remote box**.
Which means the ritual, on every host, forever:

```bash
ssh box2
export GEMINI_API_KEY=AIza…        # again — and now it's in this box's shell history
mars new work                      # ok, the agent works here
# …then box3, box4, the on-call jump host, a colleague's staging server…
```

Two things are wrong, and they compound:

- **Friction (the ergonomics smell).** N boxes × every-new-shell = a paper cut that never
  heals. It's precisely the boxes you touch *rarely* — the incident jump host at 2am — where
  you least want to be fishing a key out of a password manager.
- **Blast radius (the security smell).** The key is now sitting in cleartext in the process
  environment, the shell history, and often the dotfiles of **every box you ever configured**,
  including ones you don't own and won't harden. Compromise any one of them and your key —
  and your quota, and whatever that key can reach — is gone. A secret's exposure should be
  bounded by *your* trust in a machine; the export model makes it the union of your trust in
  *all* machines.

The design tenet Mars already lives by — *"the agent is an input device, and must be safer
than one"* (`key_design.md` §3) — extends cleanly here: **the agent's credential is part of
the trusted core, and must not be scattered to the leaves.**

---

## 2. The options

Five architectures, from "automate the copy" to "the key never moves." The axis that
matters most is **where the key physically lives when the agent makes a call**, because that
determines the blast radius when a remote box is owned.

Throughout, note the load-bearing reuse: Mars *already* runs a client/server protocol over a
Unix socket (`session.rs`: `ClientFrame`/`ServerFrame`, `write_frame`, JSON-lines, one-frame-
per-`read_line`). Every option below that involves a channel back home is the **same
machinery pointed at a second socket** — not new infrastructure.

### (a) SSH agent-forwarding analogy — the "Mars secret agent"

Mirror `ssh-agent` + `ssh -A`. A small daemon on your home machine (`mars keyd`) holds the
key and listens on a Unix socket. When you SSH, you forward that socket to the remote host,
and Mars on the remote asks the socket instead of reading `GEMINI_API_KEY`.

- **Transport:** OpenSSH remote-forwards a Unix socket:
  `ssh -R /run/user/1000/mars-auth.sock:$HOME/.mars/auth.sock box2`, and `MARS_AUTH_SOCK`
  is exported on the remote pointing at the forwarded socket. (This is the exact shape of
  `SSH_AUTH_SOCK`.)
- **Detection:** `AgentConfig::from_env()` gains one branch above the provider checks: if
  `MARS_AUTH_SOCK` is set, `provider = "forwarded"` and the key comes from the socket, not
  from env.
- **Two sub-variants, and they matter:**
  - **(a1) forward the *key*** — the socket hands the remote the bearer string, which the
    remote then uses in its own `ureq::post`. Simple, but the key now lives (in memory) on
    the remote box. Better than `.bashrc` (no disk, no history, gone on disconnect) but a
    rooted remote can still scrape it.
  - **(a2) forward the *call*** — the socket never yields the key; the remote ships the
    request *up* the socket and gets a completion *down* it. This is option (b). It is
    strictly better and barely more code, so **(a2) is the one to build.**

**Tradeoffs:** great ergonomics; leans on a standard, already-audited transport (SSH Unix-
socket forwarding); requires the tunnel be live at query time. (a1) leaks the key to remote
memory; (a2) does not.

### (b) Client-side proxy — **the key never leaves home** *(the recommendation)*

The remote daemon does not make the LLM call at all. It **forwards the fully-formed request
back through the SSH tunnel to a broker on the home machine**, which holds the key, injects
`Authorization: Bearer …`, makes the real `ureq::post`, and streams the completion back.
The key never touches the remote host — not on disk, not in env, not in memory.

This is (a2) named for what it is. Concretely it relocates the body of `chat()`:

- **Home broker (`mars keyd`):** binds `$HOME/.mars/auth.sock`, loads the key once (from the
  OS keychain or env), and on each request frame runs today's `chat()` body verbatim —
  `ureq::post(url).set("Authorization", Bearer key).send_json(messages)` — and writes the
  result back as a response frame. It is `server_main`'s shape with a one-variant protocol.
- **Remote agent:** `chat()` grows a fork. In broker mode it does **not** hit the network; it
  serializes `{model, messages, max_tokens, temperature}` to `MARS_AUTH_SOCK` using the very
  `write_frame` / `read_line` pair from `session.rs`, and blocks for the response frame. The
  30-second timeout, the 429/401 error unwrapping — all of that stays home, where the key is.

**Tradeoffs:** best security by construction (see §3); reuses existing socket + frame code;
adds one network hop of latency (remote → home → provider → home → remote), which is
invisible against a multi-second LLM completion; **does not work while detached** (the tunnel
is down when you close the laptop) — a real limitation for the watch-while-detached scenario,
addressed in §4's phasing.

### (c) Config sync — automate the copy

A `mars push-config box2` command (or an auto-push on first attach) rsyncs
`~/.config/mars/` — including a `secrets.json` — to the remote's `~/.config/mars/`, so the
remote `from_env()` (or a new `from_config()`) finds the key locally.

**Tradeoffs:** dead simple; **works offline and while detached** (the key is genuinely local
to the box), which is its one real advantage over the proxy. But it is the *manual export
model with better manners*: the key still lands, at rest, on every box — the blast radius is
unchanged. It also invites drift (which box has which key?) and a revocation nightmare (you
now must scrub N boxes). **Reject as the primary mechanism**; acceptable only as an opt-in for
a box you fully own and want detached-agent behavior on.

### (d) Central broker — Mars cloud relay

The user authenticates once (OAuth) to a Mars-hosted relay; every Mars, on any box, sends
requests to the relay, which holds the key and calls the provider. No SSH required — it works
from a box you never SSH'd *from*, from a phone, from anywhere.

**Tradeoffs:** the best ergonomics of all (truly location-independent) and the smallest
remote blast radius (a revocable session token, never the provider key). But it demands
**hosted infrastructure Mars does not have**, moves the trust boundary onto our servers,
routes every user's prompts (and code context) through us, and turns a local-first terminal
tool into a service with a bill and a privacy policy. This is a **product/business decision,
not an engineering one** — right for a later, funded phase, wrong as the first move for a
tool whose identity is "runs on your box."

### (e) OS keychain + short-lived scoped tokens

The home machine's keychain holds the real key and **mints a scoped, expiring token** per
session, handed to the remote on attach. A leaked token self-destructs; the real key never
transits.

**Tradeoffs:** excellent blast-radius containment and it *does* survive detach for the
token's lifetime (unlike the proxy). The catch is **provider reality**: Groq / Gemini /
OpenAI keys are static bearer strings — you cannot client-side derive a narrower, expiring
sub-token from a `GEMINI_API_KEY`. So (e) only works when *something you run* issues the
tokens — i.e. it is a **feature of the home broker (b) or the cloud relay (d)**, not a
standalone option. As a layer on top of (b) it is exactly the right answer to the detached
case (§4, phase 2): the broker issues its own bearer that it alone validates, leases a
short-lived one to the box, and the remote caches it for offline/detached watchers.

---

## 3. Security analysis

The one question that ranks these: **when a remote box is fully compromised, what does the
attacker get, and for how long?**

| Option | Key at rest on remote? | Key in remote memory? | Trust boundary | Blast radius if remote is owned |
|---|---|---|---|---|
| **Manual export** *(today)* | **Yes** — env, history, dotfiles | Yes | every box, forever | **Total.** The key, on every box you ever configured. Revocation = scrub N machines. |
| **(c) Config sync** | **Yes** — `secrets.json` | Yes | every synced box | **Total**, same as export — just tidier and easier to forget where it landed. |
| **(a1) Forward the key** | No | **Yes**, while forwarded | boxes with a live forward | Key scrapeable from Mars's memory / the socket during the session; safe once you disconnect. |
| **(b) Proxy — key never leaves home** | **No** | **No** | your home machine only | **Bounded.** Attacker can ride the *live* tunnel to make calls (spend quota, exfiltrate via crafted prompts) — but **cannot obtain the key** and **loses all access the instant you detach.** Key never needs rotating. |
| **(e) Short-lived token** *(on top of b/d)* | Only an expiring token | Only a token | home machine / relay | **Bounded + self-healing.** A stolen token dies on its TTL; the key is never exposed. |
| **(d) Cloud relay** | No (a session token at most) | No | Mars-hosted infra | Smallest *remote* blast radius, but the key now lives on **our** servers — the trust simply moved. |

**The winner on security is (b), the key-never-leaves-home proxy** — and it wins *by
construction*, not by policy: there is no key on the remote to steal, so no configuration
mistake, no rooted box, and no shoulder-surfed `history` can leak it. The residual risk
(quota abuse over a live tunnel) is bounded to the exact window you're actively working on
that box and is trivially revoked by detaching. (e) layered on (b) closes even that for the
detached case.

This lands exactly where Mars's own doctrine points: **the costlier the error, the taller the
gate** (`key_design.md` §1.5). A leaked credential is the costliest error in the system, so it
gets the tallest gate of all — *the key is never in a position to leak.*

---

## 4. Recommendation — the phased path

**Ship the SSH-tunnel proxy first (option b), because it reuses machinery Mars already has
and it is the security winner.** Then layer leased tokens (e) for the detached case. Treat the
cloud relay (d) as a later, deliberate product decision.

### Why (b) is the cheap build: it is `session.rs` pointed at a second socket

Everything the proxy needs, Mars already does for session persistence. The mapping is
one-to-one:

| Session daemon (exists) | Secret broker (to build) |
|---|---|
| `ServerFrame` / `ClientFrame` JSON-lines protocol | `BrokerFrame::{ChatRequest, ChatResponse, ChatError}` — same style |
| `write_frame` + `BufReader::read_line` | *reused verbatim* |
| `socket_path()` under a `0700` dir | `$HOME/.mars/auth.sock`, same `0700` hygiene |
| `client_connection()` per-conn thread + `Hello` version handshake | broker's accept loop + the identical version guard |
| `FrameWriter` streaming remote → local | the response frame streaming home → remote |
| daemon survives detach; client is a thin pump | broker survives; remote agent is a thin pump of the LLM call |

### The mechanism, concretely

**1. Home machine — `mars keyd` (the broker).** A tiny daemon (its own subcommand, sibling
to `server_main`). It:
- loads the key once — from the OS keychain (`security`/`libsecret`) or, transitionally,
  from the same `GEMINI_API_KEY`/`MARS_LLM_*` env `from_env()` reads today;
- binds `$HOME/.mars/auth.sock` (`0700`, exactly `socket_dir()`'s discipline);
- on each `ChatRequest` frame, runs **today's `chat()` body unchanged** — it is the only
  process that ever calls `ureq::post` with the key — and writes back `ChatResponse{text}`
  or `ChatError{message}` (the 429/401 unwrapping stays here, at the key).

**2. Transport — the forwarded socket.** SSH remote-forwards the broker socket and points the
remote at it:

```bash
ssh -R /run/user/$(id -u)/mars-auth.sock:$HOME/.mars/auth.sock \
    -o SetEnv=MARS_AUTH_SOCK=/run/user/$(id -u)/mars-auth.sock  box2
```

A `mars ssh box2` wrapper (or a `mars ssh-setup` that writes the `RemoteForward` +
`SetEnv` lines into `~/.ssh/config`) makes this invisible — the user just types `ssh box2`
and the channel is there.

**3. Remote box — `AgentConfig::from_env()` gets one new, highest-precedence branch:**

```rust
// src/agent.rs — from_env(), before the LLM_KEY / GROQ / GEMINI ladder
if let Ok(sock) = std::env::var("MARS_AUTH_SOCK") {
    return AgentConfig { provider: "broker", broker_sock: Some(sock),
                         key: String::new(), /* url/model still overridable */ .. };
}
```

**4. Remote `chat()` forks on provider:** in `"broker"` mode it does not touch the network —
it serializes `{model, messages, max_tokens, temperature}` to `MARS_AUTH_SOCK` with
`write_frame`, blocks on `read_line` for the response frame, and returns the text. The remote
`agent.rs` thread model (`ask` spawns a thread, `chat` blocks) is untouched — only the
transport under `chat` changes. No key, no `Authorization` header, ever constructed on the box.

**5. `is_configured()` becomes honest about the broker:** true when `MARS_AUTH_SOCK` resolves
to a reachable socket, so the UI's "agent unavailable" hint reflects reality on the remote.

That is the whole surface: **one detection branch, one `chat()` fork, one small broker daemon
that is `server_main` with a three-variant protocol.** No second renderer, no new dependency,
no protocol Mars hasn't already shipped.

### Phasing

- **Phase 1 — the proxy (b).** Ship `mars keyd` + `MARS_AUTH_SOCK` detection + the `chat()`
  broker fork + the `mars ssh` / `ssh-setup` convenience. The key never lands on a remote box.
  This is the security win and the ergonomics win in one move, on existing machinery.
- **Phase 2 — leased tokens (e) for the detached watcher.** The one thing (b) can't do is
  serve the daemon *while you're detached* (scenario #3 in `strategy.md`), because the tunnel
  is down. Fix it precisely, not broadly: on attach, the broker leases the remote a
  short-lived token it alone validates; the remote caches it so watch/brief triggers keep
  working for the lease's lifetime, then go quiet. Blast radius stays bounded; the key still
  never leaves home. (For a box you fully own and want permanent detached agency on, allow
  opt-in config-sync (c) as an explicit, per-host choice — not the default.)
- **Phase 3 — hosted relay (d), if and when the product warrants it.** The zero-SSH,
  attach-from-your-phone case. A funded, deliberate decision with an infra and trust cost —
  named here so it isn't stumbled into, not scheduled here.

### The ergonomic end state

**Mars just works on every box. You never type a key twice.** You set your key once, at home,
in the keychain; from then on the agent is present wherever you attach, and the credential is
*structurally* incapable of being left behind on a machine you don't trust. The remote box
never holds your key — so onboarding a new host is nothing, and offboarding a compromised one
is nothing to clean up.

---

## 5. What the user experience feels like — before / after

### Before (today)

```bash
ssh box2
export GEMINI_API_KEY=AIzaSyD…            # fish it out of the password manager. again.
mars new work                            # agent works — until the next box.
# the key is now in box2's process env AND ~/.bash_history.

ssh box3
export GEMINI_API_KEY=AIzaSyD…            # and again.
mars new work
# …box4, box5, the on-call jump host at 2am…
```

Every host is a fresh export, a fresh copy of the secret at rest, a fresh line in a history
file you'll forget to scrub.

### After (Phase 1: the proxy)

One time, at home:

```bash
mars keyd set gemini      # stores the key in the OS keychain; starts the home broker
mars ssh-setup            # writes the RemoteForward + SetEnv lines into ~/.ssh/config
```

Then, forever, on any box:

```bash
ssh box2                  # the auth socket is forwarded automatically (from ~/.ssh/config)
mars new work             # agent just works. No export. Nothing in shell history.
                          # box2 never sees, stores, or transmits your key.

Nothing to remember, nothing to copy, nothing left behind. The key lives exactly one place —
your machine — and every Mars you touch borrows the *ability to ask*, never the secret
itself. That is `ssh -A` for your model key: **supply once, works everywhere, leaks nowhere.**
