#!/usr/bin/env bash
# Test-first baseline: failures remain failures until Heze is implemented.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/grandine-target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
bash scripts/prepare_heze_dependencies.sh
case "${1:-all}" in
  all) filter=heze_ ;;
  minimal) filter=heze_minimal ;;
  mainnet) filter=heze_mainnet ;;
  inventory) filter=fixture_inventory ;;
  *) echo 'Usage: bash scripts/test_heze.sh [all|minimal|mainnet|inventory]' >&2; exit 2 ;;
esac
if [[ -z "${HEZE_VECTORS_DIR:-}" ]]; then
  (cd test-vectors && sha256sum --check --status heze.sha256)
fi
cargo test --locked --release \
  -p transition_functions -p bls -p kzg_utils \
  --features bls/blst,kzg_utils/blst \
  --test heze_reference_vectors "$filter" -- --nocapture
