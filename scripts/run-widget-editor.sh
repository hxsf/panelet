#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0

# Explicit host target: never inherit the repository's flashing runner.
set -euo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$script_dir/.."
# Use standalone Apple command-line tools when available. This avoids requiring
# the full Xcode application's setup for a host-only Rust executable.
if [[ "$(uname -s)" == Darwin && -z "${DEVELOPER_DIR:-}" && -d /Library/Developer/CommandLineTools ]]; then
  export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi
# Prefer rustup. A directly installed stable toolchain also works when its
# rustup launcher is missing (without changing the user's global toolchain).
if rustc +stable -vV >/dev/null 2>&1; then
  NOTE4C_RUSTC=(rustc +stable)
  NOTE4C_CARGO=(cargo +stable)
else
  NOTE4C_STABLE_BIN=""
  for candidate in "${RUSTUP_HOME:-$HOME/.rustup}"/toolchains/stable-*/bin; do
    if "$candidate/rustc" -vV >/dev/null 2>&1; then
      NOTE4C_STABLE_BIN="$candidate"
      break
    fi
  done
  if [[ -z "$NOTE4C_STABLE_BIN" ]]; then
    echo "Install a stable Rust toolchain with rustup before starting the editor." >&2
    exit 1
  fi
  export PATH="$NOTE4C_STABLE_BIN:$PATH"
  NOTE4C_RUSTC=("$NOTE4C_STABLE_BIN/rustc")
  NOTE4C_CARGO=("$NOTE4C_STABLE_BIN/cargo")
fi
NOTE4C_HOST_TARGET="$("${NOTE4C_RUSTC[@]}" -vV | sed -n 's/^host: //p')"
case "$NOTE4C_HOST_TARGET" in
  *-apple-darwin|*-linux-*) ;;
  *) echo "The widget editor supports macOS and Linux hosts." >&2; exit 1 ;;
esac
exec "${NOTE4C_CARGO[@]}" run -p panelet-os --no-default-features \
  --features desktop-editor --bin panelet-studio --target "$NOTE4C_HOST_TARGET" -- "$@"
