#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0

# Build browser Rune/Canvas without inheriting the ESP-IDF target or runner.
set -euo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$script_dir/.."
if [[ "$(uname -s)" == Darwin && -z "${DEVELOPER_DIR:-}" && -d /Library/Developer/CommandLineTools ]]; then
  export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi
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
    echo "Install stable Rust and its wasm32-unknown-unknown target before building browser preview." >&2
    exit 1
  fi
  export PATH="$NOTE4C_STABLE_BIN:$PATH"
  NOTE4C_RUSTC=("$NOTE4C_STABLE_BIN/rustc")
  NOTE4C_CARGO=("$NOTE4C_STABLE_BIN/cargo")
fi
NOTE4C_SYSROOT="$("${NOTE4C_RUSTC[@]}" --print sysroot)"
if [[ ! -d "$NOTE4C_SYSROOT/lib/rustlib/wasm32-unknown-unknown/lib" ]]; then
  echo "Install the browser target: rustup target add wasm32-unknown-unknown --toolchain stable" >&2
  exit 1
fi
# rust-lld in a directly installed macOS toolchain needs its sibling LLVM dylib.
# This affects only this script, never the user's global shell configuration.
if [[ "$(uname -s)" == Darwin ]]; then
  export DYLD_LIBRARY_PATH="$NOTE4C_SYSROOT/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
fi
NOTE4C_BUILD_TARGET_DIR="$PWD/target/preview-wasm"
CARGO_TARGET_DIR="$NOTE4C_BUILD_TARGET_DIR" \
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_PROFILE_RELEASE_LTO=true \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
CARGO_PROFILE_RELEASE_PANIC=abort \
CARGO_PROFILE_RELEASE_STRIP=symbols \
RUSTFLAGS="-C link-arg=--max-memory=67108864" \
  "${NOTE4C_CARGO[@]}" rustc -p panelet-os --target wasm32-unknown-unknown \
  --lib --crate-type cdylib --no-default-features --features browser-preview --release
mkdir -p target/web-assets/raw
cp "$NOTE4C_BUILD_TARGET_DIR/wasm32-unknown-unknown/release/panelet_preview.wasm" \
  target/web-assets/raw/preview.wasm
printf 'Browser preview WASM: %s (%s bytes)\n' \
  "$PWD/target/web-assets/raw/preview.wasm" "$(wc -c < target/web-assets/raw/preview.wasm | tr -d ' ')"
