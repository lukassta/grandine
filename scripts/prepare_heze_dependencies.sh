#!/usr/bin/env bash
# The pinned networking submodule needs explicit handling of the new Phase variant.
set -euo pipefail
root="$(cd -- "$(dirname -- "$0")/.." && pwd)"
patch="$root/patches/eth2-libp2p-heze-consensus.patch"
module="$root/eth2_libp2p"
if git -C "$module" apply --reverse --check "$patch" 2>/dev/null; then
  exit 0
fi
git -C "$module" apply --check "$patch"
git -C "$module" apply "$patch"
