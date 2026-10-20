# Heze foundation and EIP-7805 consensus implementation

Source of truth: ethereum/consensus-specs commit
[12c8b24bc09b12f62688d439ba20ef00b1fdfe1f](https://github.com/ethereum/consensus-specs/tree/12c8b24bc09b12f62688d439ba20ef00b1fdfe1f/specs/heze).
Use this revision for all three team members. The live master branch is not a stable schema.

## Scope

This change implements the FOCIL consensus components and the shared Heze foundation.
It does not implement EIP-8321, the EIP-8365 pending-deposit rule, or a live Heze
networking/validator/Engine API client. A passing focused test suite is not proof of
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

## Coordination with EIP-8015 and EIP-8365

The shared Heze wire schema must already omit old deposit/eth1 fields to decode
the official vectors. Its active-field masks and the removal of eth1 processing
are therefore included in the second (FOCIL) commit as vector compatibility
changes. The first, neutral foundation commit preserves the Gloas wire layout
and rules. Do not re-add those fields to the final Heze SSZ.

Four old state fields and two block-body fields remain internal compatibility
storage for the existing generic traits, marked serde(skip) and ssz(skip).
They do not occur in Heze SSZ/JSON or its state/block roots. The EIP-8015 work
should finish separating legacy eth1/deposit accessors from post-Heze traits,
then remove that compatibility storage. Check the fork precondition and add
dedicated EIP-8015 operation/fork vectors.

The EIP-8365 work must update the Heze pending-deposit path to reject creating
new BLS-withdrawal validators while preserving top-ups to existing validators.
The inherited Gloas/Electra deposit behavior is intentionally unchanged here.

## Networking dependency

The pinned eth2_libp2p submodule has exhaustive matches over Phase. Adding a
phase requires a compatibility change even for compiling the consensus store.
patches/eth2-libp2p-heze-consensus.patch adds explicit Heze handling: live Heze
messages return unsupported errors, instead of being decoded as Gloas.
The submodule reference is unchanged; no unpublished child-repository commit is
required. This is not a live-network FOCIL implementation.

Run bash scripts/prepare_heze_dependencies.sh after a fresh recursive checkout
before invoking Cargo directly. It is idempotent, validates the patch before
applying it, and does not overwrite unrelated submodule edits. make and the
focused GitHub workflow run it automatically.

## Validation

Run make test-heze. This executes:

1. 20 unmodified official Heze state-transition reference cases:
   the original 18 focused sanity cases plus one nonzero-bit builder-bid case per preset.
2. Five unmodified official minimal Gloas sanity regressions from the same commit.
3. Additional Rust regressions: tampered builder bits invalidate signatures;
   upgrade/version/legacy-deposit precondition; list storage/timeliness/bounds;
   real BLS fork/genesis/message binding; Python committee and SSZ oracle comparisons;
   and actual Store fork-choice/optimistic-state behavior.

The two fixture inventories cover the original nine sanity cases per preset.
Official vectors are generated in consensus-specs using pytest with --fork=heze
(or --fork=gloas for the regression subset), --preset=minimal/mainnet,
--reftests and --reftests-output. No official test source or expected post-state
has been changed. Additional Python oracle fixtures are separately labelled
test-vectors/focil-oracle/SOURCE.md; they are not official pytest cases.

First compilation includes Rust and networking dependencies. Once compiled, the
focused cases themselves take well below a second on the local machine.
