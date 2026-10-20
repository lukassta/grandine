# Pinned Heze reference vectors

Specification revision: see HEZE_SOURCE_COMMIT (12c8b24bc09b12f62688d439ba20ef00b1fdfe1f).

Run make test-heze for the focused consensus suite. See
[Heze/FOCIL implementation and team coordination](../docs/HEZE_FOCIL.md).

- heze/: 20 unmodified official cases (10 minimal, 10 mainnet).
  Each preset has nine sanity cases and one operation case verifying a builder
  bid with non-default inclusion_list_bits.
- gloas/: five official minimal sanity cases guarding the earlier fork.
- focil-oracle/: additional comparisons produced directly by the pinned Python
  specification. These are explicitly separate from official pytest vectors.
- heze.sha256: checksums of all checked-in fixture files.
- reports/: generated per-case results and logs, ignored by Git.

The initial test-first commit intentionally failed with HEZE_NOT_IMPLEMENTED.
The implementation now has a real Heze phase; these focused cases should pass.
This subset does not claim full Heze/EIP-8321 coverage.

## Official vector generation

From the consensus-specs checkout at the pinned revision:

    .venv/bin/python -m pytest --fork=heze --preset=minimal --reftests \
      --reftests-output=/path/to/grandine/test-vectors/heze -q \
      tests/core/pyspec/eth_consensus_specs/test/heze/block_processing/test_process_execution_payload_bid.py

Repeat with --preset=mainnet. The nine sanity cases per preset are selected from:

- test/phase0/sanity/test_slots.py: test_slots_1, test_slots_2, test_empty_epoch,
  test_double_empty_epoch, test_over_epoch_boundary.
- test/phase0/sanity/test_blocks.py: test_empty_block_transition,
  test_invalid_incorrect_state_root, test_invalid_all_zeroed_sig, test_skipped_slots.

Pass each as its full pytest path followed by ::test_name, retaining --fork=heze.
The source directory phase0 does not determine the generated fork: --fork=heze does.

The five Gloas regressions use --fork=gloas --preset=minimal and output directory
test-vectors/gloas: slots_1, empty_epoch, empty_block_transition,
invalid_incorrect_state_root and invalid_all_zeroed_sig.
