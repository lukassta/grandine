# EIP-8321 reference vectors

EIP-8321 is not part of a `consensus-specs` release, so its vectors cannot be fetched by
`scripts/download_spec_tests.sh` the way the other fixtures are. They are committed here instead,
as a single compressed archive, and extracted into `consensus-spec-tests/` before the tests run.

## Layout

    test-vectors/eip8321.tar.xz   the archive (committed)
    test-vectors/SOURCE_COMMIT    the consensus-specs commit the archive was generated from

The archive must contain the generator's output directories at its top level:

    minimal/eip8321/...
    mainnet/eip8321/...

`scripts/download_spec_tests.sh` extracts it to `consensus-spec-tests/tests/`, giving
`consensus-spec-tests/tests/<preset>/eip8321/<category>/...`, which is where the
`#[test_resources]` globs in `transition_functions` look for it. Extraction is keyed on the
archive's own SHA-256, so replacing the archive re-extracts it.

`test_resources` expands its globs at compile time and fails if one matches nothing, so the
vectors must be extracted before building anything with the `eip8321` feature:

    bash scripts/download_spec_tests.sh
    cargo test --release -p transition_functions -p bls -p kzg_utils \
        --features bls/blst,kzg_utils/blst,transition_functions/eip8321

Cargo does not track the fixture directory, so after replacing the archive you must force a
rebuild (`touch` the files holding the globs, or `cargo clean -p transition_functions`) for the
globs to re-expand. Otherwise the previous run's test list is reused and cases that no longer
exist fail with "the SSZ Snappy file should exist". CI always builds from scratch, so this only
affects local runs.

Both presets must be present. The feature also changes the Gloas containers' SSZ layouts, so the
Gloas spec tests are compiled out when it is enabled and the EIP's own vectors run instead; the
two fixture sets cannot be run from one build.

## Regenerating

In a `consensus-specs` checkout at the commit named in `SOURCE_COMMIT`:

    make test reftests=true fork=eip8321

Then, from the directory holding the generated `minimal` and `mainnet` trees, pack only the
categories the tests actually run:

    tar -cJf eip8321.tar.xz \
        */eip8321/epoch_processing \
        */eip8321/finality \
        */eip8321/operations \
        */eip8321/sanity

The `fork_choice`, `networking`, `fast_confirmation` and `light_client` categories are
deliberately excluded: no test consumes them, and they account for most of the generator's
output. Including them is not harmless. `scripts/ci/consensus-spec-tests-coverage.rb` requires
every file under `consensus-spec-tests/` to be matched by a glob in the codebase, so a packed
category that no test reads fails CI. Pack a category only once something runs it.
