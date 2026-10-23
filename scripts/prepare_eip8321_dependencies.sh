#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
git submodule update --init --recursive dedicated_executor eth2_libp2p
expected=0a80dbd2f0b173c8c63794fe7a0fb90a5122ef56
actual="$(git -C eth2_libp2p rev-parse HEAD)"
if [[ "$actual" != "$expected" ]]; then
  echo "Unexpected eth2_libp2p revision: $actual (expected $expected)" >&2
  exit 1
fi
patch="$(pwd)/patches/eth2-libp2p-eip8321.patch"
if git -C eth2_libp2p apply --reverse --check "$patch" 2>/dev/null; then
  exit 0
fi
git -C eth2_libp2p apply --check "$patch"
git -C eth2_libp2p apply "$patch"
