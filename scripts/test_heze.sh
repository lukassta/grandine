#!/usr/bin/env bash
# Focused official Heze/Gloas vectors and FOCIL consensus regressions.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/grandine-target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
case "${1:-all}" in
  all) filter=heze_ ;;
  minimal) filter=heze_minimal ;;
  mainnet) filter=heze_mainnet ;;
  inventory) filter=fixture_inventory ;;
  *) echo 'Usage: bash scripts/test_heze.sh [all|minimal|mainnet|inventory]' >&2; exit 2 ;;
esac
if [[ -z "${HEZE_VECTORS_DIR:-}" ]]; then
  python3 scripts/generate_reference_vectors.py --suite heze
  export HEZE_VECTORS_DIR="$PWD/test-vectors/reference"
fi
cargo test --locked --release \
  -p transition_functions -p bls -p kzg_utils \
  --features transition_functions/reference-tests,bls/blst,kzg_utils/blst \
  --test heze_reference_vectors "$filter" -- --nocapture

if [[ "$filter" == heze_ ]]; then
  cargo test --locked --release \
    -p types -p bls -p kzg_utils \
    --features bls/blst,kzg_utils/blst --test heze_eip8015
  cargo test --locked --release \
    -p genesis -p bls -p kzg_utils \
    --features bls/blst,kzg_utils/blst --test heze_genesis
fi
