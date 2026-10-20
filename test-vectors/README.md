# Heze test-first baseline

These are unmodified reference vectors generated from ethereum/consensus-specs
commit 12c8b24bc09b12f62688d439ba20ef00b1fdfe1f. Heze in this version contains
FOCIL (EIP-7805), removal of eth1/deposit fields (EIP-8015), and rejection of new
0x00 withdrawal-credential validators (EIP-8365). This is not an empty Heze fork,
and it is not the older v1.7.0-beta.1 Heze schema downloaded by the legacy suite.
No EIP-8321 implementation is added in this baseline.

## Run

    bash scripts/test_heze.sh
    # Equivalent:
    make test-heze

Individual presets and fixture-only checks:

    bash scripts/test_heze.sh minimal
    bash scripts/test_heze.sh mainnet
    bash scripts/test_heze.sh inventory

The integration target compiles the production transition library, not its large
legacy unit-test binary. It does not need the legacy consensus test downloads.
The launcher uses CARGO_TARGET_DIR under the user's cache and two build jobs by
default. Cargo compilation is cached; vector execution itself is short.

## Expected red baseline

There are 18 official cases: 9 minimal and 9 mainnet. Each preset contains five
slot cases (1 slot, 2 slots, one epoch, two epochs, epoch boundary), two valid
block cases (empty block, skipped slots), and two invalid block cases (incorrect
state root, invalid signature).

Two Rust inventory tests should PASS, proving that all fixture inputs are present
and Snappy-readable. Four Rust execution runners should FAIL while Grandine lacks
Heze support. They account for all 18 cases and report HEZE_NOT_IMPLEMENTED from
the real Phase::from_str("heze") lookup. Unsupported Heze is never accepted as an
expected rejection of an invalid block.

The test target has no should_panic, ignored tests, unconditional failure, or
Gloas fallback. After the separate Heze phase/types/config/combined dispatch are
implemented, the same runners decode Heze states and execute production slot and
block transitions, verify every block state root and the final post-state root,
and require invalid vectors to be rejected. Phase recognition alone is not enough.

Reports: test-vectors/reports/heze-{minimal,mainnet}-{slots,blocks}.tsv.
The baseline is deliberately limited to these 18 state-transition cases; it does
not claim full Heze operation, fork-upgrade, networking, or fork-choice coverage.
Original Gloas production code and containers remain unchanged.

## Regenerate exactly this focused selection

Prepare the consensus-specs Python development environment and build its pyspec
modules following that repository's Makefile. Check out HEZE_SOURCE_COMMIT first.
From the consensus-specs root, with OUT an absolute Grandine test-vectors/heze
path, run for each PRESET (minimal, mainnet):

    .venv/bin/python -m pytest --fork=heze --preset="$PRESET" \
      --reftests --reftests-output="$OUT" -q \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_slots.py::test_slots_1 \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_slots.py::test_slots_2 \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_slots.py::test_empty_epoch \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_slots.py::test_double_empty_epoch \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_slots.py::test_over_epoch_boundary \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_blocks.py::test_empty_block_transition \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_blocks.py::test_invalid_incorrect_state_root \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_blocks.py::test_invalid_all_zeroed_sig \
      tests/core/pyspec/eth_consensus_specs/test/phase0/sanity/test_blocks.py::test_skipped_slots

The Python test functions are inherited generic tests in phase0/sanity, but
--fork=heze runs them against the Heze reference specification. Both generation
runs completed successfully (9 passed each). The expensive historical accumulator
scenario is intentionally outside this initial selection.

heze.sha256 records SHA-256 checksums of every original fixture file.
Upstream tests/vectors were not edited. consensus-specs is licensed CC0-1.0.

## GitHub Actions

.github/workflows/heze-tests.yml runs fixture inventory and the real execution
runners in separate steps on GitHub-hosted Ubuntu. The execution step is expected
to be RED until Heze support is implemented. It does not use continue-on-error
or convert expected baseline failures into a green workflow. Per-case TSV files
and logs are uploaded with if: always(). No Python regeneration or local legacy
fixture cache is needed in CI, because the 18 official cases are committed here.
