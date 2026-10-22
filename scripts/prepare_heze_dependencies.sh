#!/usr/bin/env bash
# The pinned networking submodule needs explicit handling of the new Phase variant.
set -euo pipefail
root="$(cd -- "$(dirname -- "$0")/.." && pwd)"
module="$root/eth2_libp2p"
# Separate patches allow an existing checkout to upgrade from the earlier patch.
for patch in "$root/patches/eth2-libp2p-heze-consensus.patch" "$root/patches/eth2-libp2p-heze-fork-context-tests.patch"; do
  if git -C "$module" apply --reverse --check "$patch" 2>/dev/null; then
    continue
  fi
  git -C "$module" apply --check "$patch"
  git -C "$module" apply "$patch"
done
