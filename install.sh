#!/bin/sh
# Foolproof Mars installer — run it directly (`sh install.sh`) or scp it to a host.
# (Once this repo is public on GitHub, the curl|sh form of its raw URL works too.)
#
# It automates the official steps (rust-lang.org/tools/install + crates.io):
#   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
#   cargo install mars-terminal
#
# Handles the #1 remote-host trap: a distro `cargo` (e.g. Ubuntu's 1.75) is far too
# old for Mars's dependency tree, which needs Rust >= 1.85 (edition2024). This script
# installs rustup (latest stable) when the toolchain is missing or too old, then
# installs the `mars` binary — no `sudo apt install cargo` required.
#
# Knobs (env vars):
#   MARS_SOURCE=git         install the latest from the repo instead of crates.io
#   MARS_GIT_URL=<url>       repo to use when MARS_SOURCE=git
set -eu

MSRV_MINOR=85                 # Mars needs Rust >= 1.85
CRATE=mars-terminal           # crates.io package name (the binary is `mars`)
: "${MARS_SOURCE:=crate}"
: "${MARS_GIT_URL:=https://github.com/anjishnu/mars}"

say() { printf '\033[1m[mars-install]\033[0m %s\n' "$1"; }
die() { printf '\033[1;31m[mars-install]\033[0m %s\n' "$1" >&2; exit 1; }

# Is the cargo on PATH new enough?
cargo_ok() {
  command -v cargo >/dev/null 2>&1 || return 1
  v=$(cargo --version 2>/dev/null | awk '{print $2}') || return 1
  maj=${v%%.*}; rest=${v#*.}; min=${rest%%.*}
  [ "${maj:-0}" -gt 1 ] 2>/dev/null && return 0
  [ "${maj:-0}" -eq 1 ] 2>/dev/null && [ "${min:-0}" -ge "$MSRV_MINOR" ] 2>/dev/null
}

if cargo_ok; then
  say "using existing toolchain: $(cargo --version)"
else
  if command -v cargo >/dev/null 2>&1; then
    say "found $(cargo --version) — too old (need Rust >= 1.$MSRV_MINOR). Installing rustup…"
    say "(a distro-packaged cargo is fine to leave installed; we just won't use it)"
  else
    say "no Rust toolchain found — installing rustup…"
  fi
  command -v curl >/dev/null 2>&1 || die "need 'curl' to fetch rustup — install it first (e.g. sudo apt install -y curl)."
  curl --proto '=https' --tlsv1.2 -fsSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

# Make rustup's cargo win over any distro cargo for this session.
export PATH="$HOME/.cargo/bin:$PATH"
cargo_ok || die "Rust is still older than 1.$MSRV_MINOR after install — try: rustup update stable"

say "installing $CRATE (this compiles from source; a minute or two)…"
if [ "$MARS_SOURCE" = git ]; then
  cargo install --git "$MARS_GIT_URL" --force
else
  cargo install "$CRATE" --force
fi

BIN="$(command -v mars || true)"
[ -n "$BIN" ] || die "install finished but 'mars' isn't on PATH — add:  export PATH=\"\$HOME/.cargo/bin:\$PATH\""
say "done → $BIN"
mars version 2>/dev/null | tail -1 || true
say "if a future shell can't find 'mars', add this to your rc file:"
say "  export PATH=\"\$HOME/.cargo/bin:\$PATH\""
