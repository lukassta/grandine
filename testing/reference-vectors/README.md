# Official reference vector selection

Source: ethereum/consensus-specs commit
12c8b24bc09b12f62688d439ba20ef00b1fdfe1f.

selection.json lists exact existing official pytest node IDs and expected case
counts: 368 EIP-8321, 44 Heze and six Gloas comparisons. sha256.json records
the exact bytes of their generated SSZ/Snappy/YAML files. Neither file defines
a new consensus case or computes an expected state using Grandine.

Generate with:

    python3 scripts/generate_reference_vectors.py --suite all

The script clones the pinned official source, installs its locked test
dependencies using uv, builds its Python specifications, and selects only
the listed official tests. It never patches official tests or constants.
Generated output and generator checkouts live under ignored test-vectors/.
GitHub Actions caches generated vectors and verifies their checksums on reuse.

To reuse an existing official checkout and environment:

    python3 scripts/generate_reference_vectors.py --suite all \
      --spec-checkout ~/projects/consensus-specs \
      --python ~/projects/consensus-specs/.venv/bin/python

For faster initial generation, add --preset minimal. To verify a cache without
installing or generating anything, add --verify-only.

The upstream 256-validator mainnet registration-lifecycle generator fails its
128-registration assertion. It is not selected or claimed as passing.
The minimal lifecycle vector is selected. Official expectations are unchanged.

The ordinary scripts/download_spec_tests.sh fetches released consensus,
cryptography and SSZ archives into their existing directories. That archive
does not contain EIP-8321, and its Heze cases target a different revision.
The versioned reference selection must not be confused with that released suite.

If the selected checkout already has reftests/, matching files are reused
after checksum validation. --reftests-source /path/to/reftests also accepts
a generated reference directory directly. Missing or incorrect files are
regenerated, and the final output is verified again.

Only the first uncached generation is expensive, especially for mainnet.
Subsequent test runs verify and reuse the cache. Generated binary files are
removed from the current Git tree, not rewritten out of older Git history.
