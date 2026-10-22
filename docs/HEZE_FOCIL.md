# Heze foundation and EIP-7805 consensus implementation

Source of truth: ethereum/consensus-specs commit
[12c8b24bc09b12f62688d439ba20ef00b1fdfe1f](https://github.com/ethereum/consensus-specs/tree/12c8b24bc09b12f62688d439ba20ef00b1fdfe1f/specs/heze).
Use this revision for all three team members. The live master branch is not a stable schema.

## Scope

This change implements the FOCIL consensus components and the shared Heze foundation.
It includes EIP-8015 and EIP-8365 consensus rules. It does not implement EIP-8321
or a live Heze networking/validator/Engine API client. A passing focused test suite is not proof of
all Heze categories passing.

Heze has its own Phase variant, fork epoch/version, state, block, block body and
payload bid. The combined dispatcher decodes and processes these concrete Heze
types. Gloas is still a separate fork. Shared unchanged algorithms/containers are
inherited; Heze fixtures are never processed with Phase::Gloas.

## Code map

- types/src/heze/containers.rs: InclusionList, SignedInclusionList, the 13-field
  progressive ExecutionPayloadBid (including inclusion_list_bits), and Heze blocks.
- types/src/heze/beacon_state.rs: separate progressive Heze state with the official
  active-field gaps. The latest bid is the actual Heze bid.
- types/src/heze/primitives.rs and consts.rs: 16-bit committee mask, committee,
  transactions, and signature domain 0x10000000.
- types/src/heze/bid_fields.rs: read-only common-field projections for existing
  generic code. These projections are never serialized, signed, or stored as a bid.
  Heze bits remain part of the real bid's SSZ root and builder signature.
- types/src/{combined,nonstandard,config,traits}.rs: phase and shared API plumbing.
- helper_functions/src/fork.rs: upgrade_to_heze. It checks that legacy deposits
  are fully processed and initializes inclusion-list bits to zero.
- helper_functions/src/heze.rs: committee/duty selection; BLS signature validation
  using the message slot's configured fork version and genesis root; size/slot
  bounds; list store; duplicate/equivocation handling; transaction deduplication;
  timeliness filtering, inclusive bit masks, and retention.
- transition_functions/src/heze/: separate state/slot/epoch/block processing.
  Builder bids are checked and stored with their real Heze root, including bits.
- fork_choice_store/src/store.rs: on_inclusion_list validates before mutation;
  apply_heze_execution_payload_envelope takes an EL satisfaction callback;
  payload_inclusion_list_transactions prepares that callback's input;
  record_payload_inclusion_list_satisfaction handles optimistic-to-validated
  results, retaining validated results after pruning; should_extend_payload
  applies the Heze-only constraint. An unsatisfied payload remains execution-valid.
  validate_heze_bid_inclusion_list_bits supports proposer/bid selection.

The EL callback is a required integration boundary. A simple search for every
transaction in the payload is not an implementation of the execution-layer
satisfaction rules (gas-limit and execution validity exceptions matter).

## EIP-8015 and EIP-8365

Heze state and block-body types contain none of the retired fields: eth1_data,
eth1_data_votes, eth1_deposit_index, deposit_requests_start_index and body deposits.
They are absent from the Rust types as well as SSZ and JSON. Shared state/body
traits expose optional pre_heze capabilities; only Phase0 through Gloas implement
the legacy traits. Electra bridge-index access is also restricted to pre-Heze states.

upgrade_to_heze checks that eth1_deposit_index equals deposit_requests_start_index,
preserves the remaining consensus fields and drops the retired ones. Heze block
and epoch processing never call the Eth1 voting/reset paths. Genesis assembly
uses the pre-Heze deposit mechanism internally, then upgrades to the actual Heze
state and initializes the Heze genesis block-body root.

Heze apply_pending_deposit skips a deposit whose public key is new and whose
withdrawal credentials start with 0x00. The queue still consumes the deposit and
its applicable churn; no refund or validator is created. Existing validators
can still receive top-ups, including deposits with invalid signatures or different
withdrawal credentials. Other prefixes follow the inherited proof-of-possession
rules. The same restriction applies to deposits queued before the fork.
Gloas retains its original behavior.

This uses the pinned specification above. It does not establish compatibility
with arbitrary heze-devnet-2 images or mutable unstable tags; that version
alignment remains part of the later Kurtosis interoperability work.

## Networking dependency

The pinned eth2_libp2p submodule has exhaustive matches over Phase. Adding a
phase requires a compatibility change even for compiling the consensus store.
patches/eth2-libp2p-heze-consensus.patch adds explicit Heze handling: live Heze
messages return unsupported errors, instead of being decoded as Gloas.
patches/eth2-libp2p-heze-fork-context-tests.patch updates the runtime fork-digest
regression to assert Gloas -> Heze -> BPO transitions before the final zero digest.
This is a networking unit-test adaptation, not a modification to official vectors.
The patches apply independently, so existing checkouts with the first patch can
upgrade safely. The submodule reference is unchanged; no unpublished child-repository commit is
required. This is not a live-network FOCIL implementation.

Run bash scripts/prepare_heze_dependencies.sh after a fresh recursive checkout
before invoking Cargo directly. It is idempotent, validates the patch before
applying it, and does not overwrite unrelated submodule edits. make and the
focused GitHub workflow run it automatically.

## Validation

Run make test-heze. This executes:

1. 44 unmodified official Heze reference cases:
   20 original sanity/builder-bid cases; 16 pending-deposit cases (10 minimal,
   six mainnet); and eight Gloas-to-Heze fork cases (seven minimal, one mainnet).
2. Six unmodified official minimal Gloas regressions, including creation of a
   new BLS-withdrawal validator, which remains allowed before Heze.
3. Additional Rust regressions: removed fields are absent from JSON and rejected
   on input; legacy capabilities end at Gloas; Heze genesis assembly; tampered bid
   signatures; fork preconditions; list storage, BLS signatures, committee/SSZ
   oracle comparisons; and actual Store fork-choice/optimistic-state behavior.

The two fixture inventories cover the original nine sanity cases per preset.
Official vectors are generated in consensus-specs using pytest with --fork=heze
(or --fork=gloas for the regression subset), --preset=minimal/mainnet,
--reftests and --reftests-output. No official test source or expected post-state
has been changed. Additional Python oracle fixtures are separately labelled
test-vectors/focil-oracle/SOURCE.md; they are not official pytest cases.

First compilation includes Rust and networking dependencies. Once compiled, the
focused cases themselves take well below a second on the local machine.


## Additional official vector generation

Use the same pytest command above, selecting these modules at the pinned revision:

- test/heze/epoch_processing/test_process_pending_deposits.py:
  all six tests, separately for minimal and mainnet with --fork=heze.
- test/heze/fork/test_heze_fork_basic.py:
  all seven minimal tests and mainnet test_fork_base_state.
  Use BOTH --fork=gloas --fork=heze: these tests run on Gloas but emit Heze vectors.
- test/electra/epoch_processing/pending_deposits/test_apply_pending_deposit.py:
  minimal Heze tests for eth1_withdrawal_credentials,
  compounding_withdrawal_credentials_max, non_versioned_withdrawal_credentials
  and incorrect_sig_new_deposit (each prefixed test_apply_pending_deposit_).
  The Gloas regression selects test_apply_pending_deposit_min_activation
  with --fork=gloas and output test-vectors/gloas.

The directory of an inherited Python test does not select its fork; --fork does.
No Python test sources or expected post-states are modified.


Heze reference tests are centralized in
transition_functions/tests/heze_reference_vectors.rs against the pinned fixture
directory. The copied Gloas unit-test scaffolding in the Heze modules was removed:
it referenced Gloas state aliases and a different released vector schema, so it
could not compile or validate Heze. The actual Gloas test modules are retained.
