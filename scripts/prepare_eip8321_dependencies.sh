#!/usr/bin/env bash
# Adapt exhaustive fork matches without selecting or resetting dependency revisions.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [[ ! -f eth2_libp2p/Cargo.toml ]]; then
  echo 'Initialize the repository submodules first: git submodule update --init --recursive' >&2
  exit 1
fi
patch="$(pwd)/patches/eth2-libp2p-eip8321.patch"
if git -C eth2_libp2p apply --reverse --check "$patch" 2>/dev/null; then
  exit 0
fi
if ! git -C eth2_libp2p apply --check "$patch"; then
  echo 'The networking dependency differs from the version supported by this patch.' >&2
  echo 'No files or submodule revisions were changed. Check the submodule selected by Git.' >&2
  exit 1
fi
git -C eth2_libp2p apply "$patch"
