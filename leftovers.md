# Windows port leftovers

## Known failing checks

- **Windows session PTY startup:** the default Windows selfcheck reaches the
  persistent-session shell test but does not see `echo daemon_pty_ok` execute.
  The parsed screen contains only the PowerShell banner. This is likely in the
  fresh-shell input readiness path and currently blocks a fully green Windows
  selfcheck.
- **Unix DECCKM wheel forwarding:** the Unix selfcheck enters application-cursor
  mode, but the expected `ESC O A` wheel-up sequence is not observed in the PTY
  echo. This may be a startup-buffer/test interaction, but remains unresolved
  and blocks a fully green Unix selfcheck.

## Deployment blockers and unverified paths

- The currently published `mars-terminal 0.4.0` does not advertise
  `capability-v1`. Automatic bootstrap therefore installs 0.4.0 and then
  intentionally refuses Windows-to-Unix broker handoff until a compatible
  release containing this branch is published.
- A live brokered Azure LLM request is unverified because the available Azure
  identity lacks the chat-completions data-plane permission and no account key
  was available.
- ConPTY process teardown, broad Windows `killall`, and daemon detachment still
  need a final real-terminal smoke pass; the headless selfcheck cannot fully
  validate those OS-level behaviors.
