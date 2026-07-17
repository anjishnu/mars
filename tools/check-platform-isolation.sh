#!/usr/bin/env bash
# Platform-isolation lint (WINDOWS_PORT.md "Platform boundary").
#
# The dependency rule: no module outside `src/sys/` may name a platform
# primitive. All OS-specific behavior is reached through `sys::<capability>`.
# If this script prints anything, an abstraction leak has been introduced —
# push it down into a `sys/` adapter behind a capability port.
#
# `src/broker.rs` is exempt: the ssh key broker is an optional Unix-only
# capability until the tunnel is redesigned for a Windows home host.
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='std::os::(unix|windows)|\blibc::|\bwindows_sys::|\binterprocess::|\bnix::'

if git grep -nE "$pattern" -- \
      'src/*.rs' \
      ':(exclude)src/sys/*.rs' \
      ':(exclude)src/broker.rs'; then
    echo
    echo "✗ platform primitive leaked outside src/sys/ (see WINDOWS_PORT.md)." >&2
    echo "  Move it into a sys/ adapter behind a capability port." >&2
    exit 1
fi
echo "✓ platform isolation holds — no OS primitives outside src/sys/ (broker.rs exempt)."
