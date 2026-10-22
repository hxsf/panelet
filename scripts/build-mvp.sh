#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0


set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"

cd "$repo_root"
exec cargo build --release -p panelet-os --bin panelet-os "$@"
