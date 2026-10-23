#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/grandine-target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
target=eip8321_reference_vectors
case "${1:-all}" in
  all) filter=eip8321_; preset=all ;;
  minimal) filter=eip8321_minimal; preset=minimal ;;
  mainnet) filter=eip8321_mainnet; preset=mainnet ;;
  regressions) filter=eip8321_; preset=minimal; target=eip8321_regressions ;;
  *) echo 'Usage: bash scripts/test_eip8321.sh [all|minimal|mainnet|regressions]' >&2; exit 2 ;;
esac
if [[ -z "${EIP8321_VECTORS_DIR:-}" ]]; then
  python3 scripts/generate_reference_vectors.py --suite eip8321 --preset "$preset"
  export EIP8321_VECTORS_DIR="$PWD/test-vectors/reference"
fi
cargo test --locked --release \
  -p transition_functions -p bls -p kzg_utils \
  --features transition_functions/reference-tests,bls/blst,kzg_utils/blst \
  --test "$target" "$filter" -- --nocapture
