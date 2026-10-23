#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/grandine-target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
case "${1:-all}" in
  all) filter=eip8321_ ;;
  minimal) filter=eip8321_minimal ;;
  mainnet) filter=eip8321_mainnet ;;
  *) echo 'Usage: bash scripts/test_eip8321.sh [all|minimal|mainnet]' >&2; exit 2 ;;
esac
if [[ -z "${EIP8321_VECTORS_DIR:-}" ]]; then
  (cd test-vectors && sha256sum --check --status eip8321.sha256)
  mkdir -p test-vectors/eip8321
  tar --no-same-owner --no-same-permissions -xzf test-vectors/eip8321-vectors.tar.gz -C test-vectors/eip8321
fi
cargo test --locked --release \
  -p transition_functions -p bls -p kzg_utils \
  --features bls/blst,kzg_utils/blst \
  --test eip8321_reference_vectors "$filter" -- --nocapture

if [[ "$filter" == eip8321_ ]]; then
  cargo test --locked --release \
    -p genesis -p bls -p kzg_utils \
    --features bls/blst,kzg_utils/blst --test eip8321_genesis
fi
