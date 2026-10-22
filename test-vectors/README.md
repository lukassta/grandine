# Pinned Heze reference vectors

Specification revision: see HEZE_SOURCE_COMMIT (12c8b24bc09b12f62688d439ba20ef00b1fdfe1f).

Run make test-heze for the focused consensus suite. See
[Heze/FOCIL implementation and team coordination](../docs/HEZE_FOCIL.md).

- heze/: 44 unmodified official cases (27 minimal, 17 mainnet).
  Sanity and builder-bid cases, EIP-8365 pending-deposit transitions, and
  EIP-8015 Gloas-to-Heze fork transitions.
- gloas/: six official minimal regressions guarding the earlier fork,
  including new BLS-withdrawal validator registration.
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


Additional EIP-8015/EIP-8365 generation commands and exact selections are documented
in ../docs/HEZE_FOCIL.md. Fork vectors need --fork=gloas --fork=heze because their
input phase differs from their output phase.

## EIP-8321

The SHA-256-pinned eip8321-vectors.tar.gz contains 368 unmodified official
state-transition cases from EIP8321_SOURCE_COMMIT. EIP8321_INVENTORY.json
lists the selected categories and exact counts. Run make test-eip8321;
see ../docs/EIP8321.md for scope, regeneration, provenance and the known
upstream mainnet lifecycle generator failure.
