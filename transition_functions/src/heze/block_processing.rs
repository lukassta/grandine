use crate::electra::process_attester_slashing;
use crate::gloas::{
    apply_attestation, process_payload_attestation, process_proposer_slashing,
    validate_attestation_with_verifier,
};
use crate::{capella, electra};
use anyhow::{Result, ensure};
use arithmetic::UsizeExt as _;
use bls::traits::SignatureBytes as _;
use helper_functions::accessors::initialize_shuffled_indices;
use helper_functions::verifier::Triple;
use helper_functions::{
    accessors::{self, get_beacon_proposer_index, get_current_epoch, get_randao_mix},
    error::SignatureKind,
    misc::builder_payment_index_for_current_epoch,
    predicates::{can_builder_cover_bid, is_active_builder},
    signing::SignForSingleFork as _,
    slot_report::SlotReport,
    verifier::{SingleVerifier, Verifier},
};
use pubkey_cache::PubkeyCache;
#[cfg(not(target_os = "zkvm"))]
use rayon::iter::ParallelIterator as _;
use ssz::{Hc, SszHash as _, SszList as _};
use typenum::Unsigned as _;
use types::{
    config::Config,
    gloas::{
        consts::{BUILDER_INDEX_SELF_BUILD, PAYLOAD_BUILDER_VERSION},
        containers::{BuilderPendingPayment, BuilderPendingWithdrawal, ExecutionRequests},
    },
    heze::{
        beacon_state::BeaconState as HezeBeaconState,
        containers::{
            BeaconBlock, ExecutionPayloadBid, SignedBeaconBlock, SignedExecutionPayloadBid,
        },
    },
    phase0::consts::GENESIS_SLOT,
    preset::Preset,
    traits::PostGloasBeaconState,
};
use types::{
    phase0::primitives::Slot,
    traits::{
        BlockBodyWithBlsToExecutionChanges, BlockBodyWithGloasAttestations,
        BlockBodyWithGloasAttesterSlashings, BlockBodyWithPayloadAttestations,
    },
};

use crate::{
    altair,
    gloas::{self, process_withdrawals},
    unphased::{self, Error},
};

#[cfg(feature = "metrics")]
use prometheus_metrics::METRICS;

/// [`process_block`](TODO(feature/electra))
///
/// This also serves as a substitute for [`compute_new_state_root`]. `compute_new_state_root` as
/// defined in `consensus-specs` uses `state_transition`, but in practice `state` will already be
/// processed up to `block.slot`, which would make `process_slots` fail due to the restriction added
/// in [version 0.11.3]. `consensus-specs` [originally used `process_block`] but it was [lost].
///
/// [`compute_new_state_root`]:        https://github.com/ethereum/consensus-specs/blob/2ef55744df782eb153fc0a3b1c7875b8c2e11730/specs/phase0/validator.md#state-root
/// [version 0.11.3]:                  https://github.com/ethereum/consensus-specs/releases/tag/v0.11.3
/// [originally used `process_block`]: https://github.com/ethereum/consensus-specs/commit/103a66b2af9d9ec1fd1c70adc8e9029af5775c1c#diff-abbdef70b08ada829d740f06c004c154R298-R301
/// [lost]:                            https://github.com/ethereum/consensus-specs/commit/2dbc33327084d2814958f92eb0a838b9bc161903#diff-e96c612010477fc9536e3ff1ef1a1d5dR343-R346
pub fn process_block<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut HezeBeaconState<P>,
    block: &Hc<BeaconBlock<P>>,
    mut verifier: impl Verifier,
    slot_report: impl SlotReport,
) -> Result<()> {
    #[cfg(feature = "metrics")]
    let _timer = METRICS
        .get()
        .map(|metrics| metrics.block_transition_times.start_timer());

    verifier.reserve(count_required_signatures(block)?);

    custom_process_block(
        config,
        pubkey_cache,
        state,
        block,
        &mut verifier,
        slot_report,
    )?;

    verifier.finish()
}

pub fn process_block_for_gossip<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &HezeBeaconState<P>,
    block: &SignedBeaconBlock<P>,
) -> Result<()> {
    debug_assert_eq!(state.slot, block.message.slot);

    unphased::process_block_header_for_gossip(config, state, &block.message)?;

    let public_key = accessors::public_key(state, block.message.proposer_index)?;

    SingleVerifier.verify_singular(
        block.message.signing_root(config, state),
        block.signature,
        pubkey_cache.get_or_insert(*public_key)?,
        SignatureKind::Block,
    )?;

    Ok(())
}

fn ensure_operation_count<P: Preset>(
    kind: &'static str,
    in_block: usize,
    maximum: usize,
) -> Result<()> {
    ensure!(
        in_block <= maximum,
        Error::<P>::TooManyOperations {
            kind,
            maximum,
            in_block
        }
    );
    Ok(())
}

pub fn count_required_signatures<P: Preset>(block: &Hc<BeaconBlock<P>>) -> Result<usize> {
    altair::count_required_signatures(block)?
        .try_add(block.body.bls_to_execution_changes.len())?
        .try_add(block.body.payload_attestations.len())
        .map_err(Into::into)
}

pub fn custom_process_block<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut HezeBeaconState<P>,
    block: &Hc<BeaconBlock<P>>,
    mut verifier: impl Verifier,
    mut slot_report: impl SlotReport,
) -> Result<()> {
    debug_assert_eq!(state.slot, block.slot);

    // > [New in Gloas:EIP7732]
    let parent_slot = state.latest_block_header.slot;
    process_parent_execution_payload(config, pubkey_cache, state, block)?;

    unphased::process_block_header(config, state, block)?;

    // > [Modified in Gloas:EIP7732]
    process_withdrawals(state)?;

    // > [New in Gloas:EIP7732]
    // This function must be called after `process_withdrawals`
    process_execution_payload_bid(
        config,
        pubkey_cache,
        state,
        &block.body.signed_execution_payload_bid,
    )?;

    unphased::process_randao(config, pubkey_cache, state, &block.body, &mut verifier)?;

    // > [Modified in Gloas:EIP7732]
    process_operations(
        config,
        pubkey_cache,
        state,
        &block.body,
        parent_slot,
        &mut verifier,
        &mut slot_report,
    )?;

    altair::process_sync_aggregate(
        config,
        pubkey_cache,
        state,
        block.body.sync_aggregate,
        verifier,
        slot_report,
    )
}

fn validate_execution_payload_bid_signature_with_verifier<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &impl PostGloasBeaconState<P>,
    signed_bid: &SignedExecutionPayloadBid<P>,
    mut verifier: impl Verifier,
) -> Result<()> {
    let SignedExecutionPayloadBid {
        message: execution_payload_bid,
        signature,
    } = signed_bid;

    let builder = state.builders().get(execution_payload_bid.builder_index)?;

    // > Verify signature
    verifier.verify_singular(
        execution_payload_bid.signing_root(config, state),
        *signature,
        pubkey_cache.get_or_insert(builder.pubkey)?,
        SignatureKind::ExecutionPayloadBid,
    )
}

fn validate_execution_payload_bid<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &impl PostGloasBeaconState<P>,
    signed_bid: &SignedExecutionPayloadBid<P>,
) -> Result<()> {
    let current_epoch = get_current_epoch(state);
    let ExecutionPayloadBid {
        builder_index,
        value: amount,
        slot,
        block_hash,
        parent_block_hash,
        parent_block_root,
        prev_randao,
        ..
    } = signed_bid.message;

    // > For self-builds, `amount` must be zero and `signature` must be empty,
    // Otherwise verify builder is active, has valid signature, and can pay bid.
    if builder_index == BUILDER_INDEX_SELF_BUILD {
        ensure!(amount == 0, Error::<P>::NoneZeroBidValue);
        ensure!(
            signed_bid.signature.is_empty(),
            Error::<P>::ExecutionPayloadBidSignatureInvalid
        );
    } else {
        let builder = state.builders().get(builder_index)?;
        ensure!(
            is_active_builder(builder, state.finalized_checkpoint().epoch),
            Error::<P>::BuilderNotActive {
                index: builder_index,
                current_epoch
            }
        );
        ensure!(
            builder.version == PAYLOAD_BUILDER_VERSION,
            Error::<P>::BuilderVersionMismatch {
                index: builder_index,
                builder_version: builder.version,
                expected: PAYLOAD_BUILDER_VERSION,
            }
        );
        ensure!(
            can_builder_cover_bid(state, builder_index, amount)?,
            Error::<P>::BuilderBalanceNotSufficient {
                index: builder_index,
                amount
            }
        );
        ensure!(
            validate_execution_payload_bid_signature_with_verifier(
                config,
                pubkey_cache,
                state,
                signed_bid,
                SingleVerifier,
            )
            .is_ok(),
            Error::<P>::ExecutionPayloadBidSignatureInvalid
        );
    }

    // > Verify commitments are under limit
    let maximum = config
        .get_blob_schedule_entry(get_current_epoch(state))
        .max_blobs_per_block;
    let in_block = signed_bid.message.blob_kzg_commitments.len();

    ensure!(
        in_block <= maximum,
        Error::<P>::TooManyBlockKzgCommitments { in_block, maximum },
    );

    // > Verify that the bid is for the current slot
    ensure!(
        slot == state.slot(),
        Error::<P>::BidSlotMismatch {
            in_bid: slot,
            in_state: state.slot(),
        }
    );
    ensure!(state.slot() > GENESIS_SLOT, Error::<P>::BidSlotAtGenesis);

    // > Verify that the bid is for the right parent block
    ensure!(
        parent_block_hash == state.latest_block_hash(),
        Error::<P>::BidParentBlockHashMismatch {
            in_bid: parent_block_hash,
            in_state: state.latest_block_hash(),
        }
    );

    // > Verify that the bid's block hash differs from its parent block hash
    ensure!(
        block_hash != parent_block_hash,
        Error::<P>::BidBlockHashEqualsParentBlockHash { block_hash }
    );

    let parent_beacon_block_root =
        accessors::get_block_root_at_slot(state, state.slot().saturating_sub(1))?;

    ensure!(
        parent_block_root == parent_beacon_block_root,
        Error::<P>::BidParentBlockRootMismatch {
            in_bid: parent_block_root,
            in_state: parent_beacon_block_root,
        }
    );

    // > Verify prev_randao
    let in_state = get_randao_mix(state, current_epoch);
    ensure!(
        prev_randao == in_state,
        Error::<P>::BidPrevRandaoMismatch {
            in_bid: prev_randao,
            in_state,
        },
    );

    Ok(())
}

pub fn process_parent_execution_payload<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut impl PostGloasBeaconState<P>,
    block: &BeaconBlock<P>,
) -> Result<()> {
    let bid = &block.body.signed_execution_payload_bid.message;
    let parent_bid = state.latest_execution_payload_bid();
    let requests = &block.body.parent_execution_requests;

    if bid.parent_block_hash != parent_bid.block_hash() {
        // Parent was EMPTY -- no execution requests expected
        ensure!(
            *requests == ExecutionRequests::<P>::default(),
            Error::<P>::ExecutionRequestsNotEmpty
        );
        return Ok(());
    }

    let computed = requests.hash_tree_root();

    ensure!(
        computed == parent_bid.execution_requests_root(),
        Error::<P>::ExecutionRequestsRootMismatch {
            computed,
            expected: parent_bid.execution_requests_root(),
        }
    );

    gloas::apply_parent_execution_payload(config, pubkey_cache, state, requests)
}

pub fn process_execution_payload_bid<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut HezeBeaconState<P>,
    signed_bid: &SignedExecutionPayloadBid<P>,
) -> Result<()> {
    let ExecutionPayloadBid {
        value: amount,
        builder_index,
        slot,
        fee_recipient,
        ..
    } = signed_bid.message;

    validate_execution_payload_bid(config, pubkey_cache, state, signed_bid)?;

    // > Record the pending payment if there is some payment
    if amount > 0 {
        let pending_payment = BuilderPendingPayment {
            weight: 0,
            withdrawal: BuilderPendingWithdrawal {
                fee_recipient,
                amount,
                builder_index,
            },
            proposer_index: get_beacon_proposer_index(config, state)?,
        };
        *state
            .builder_pending_payments_mut()
            .mod_index_mut(builder_payment_index_for_current_epoch::<P>(slot)?) = pending_payment;
    }

    // > Cache the signed execution payload bid
    state.latest_execution_payload_bid = signed_bid.message.clone();

    Ok(())
}

fn process_operations<P: Preset, V: Verifier, B>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut impl PostGloasBeaconState<P>,
    body: &B,
    parent_slot: Slot,
    mut verifier: V,
    mut slot_report: impl SlotReport,
) -> Result<()>
where
    B: BlockBodyWithGloasAttestations<P>
        + BlockBodyWithGloasAttesterSlashings<P>
        + BlockBodyWithBlsToExecutionChanges<P>
        + BlockBodyWithPayloadAttestations<P>,
{
    // > [New in Gloas:EIP7688] Progressive lists are unbounded at the SSZ level.
    // > Operation counts are validated during processing instead.
    ensure_operation_count::<P>(
        "proposer slashings",
        body.proposer_slashings().len_usize(),
        P::MaxProposerSlashings::USIZE,
    )?;
    ensure_operation_count::<P>(
        "attester slashings",
        body.attester_slashings().len_usize(),
        P::MaxAttesterSlashingsElectra::USIZE,
    )?;
    ensure_operation_count::<P>(
        "attestations",
        body.attestations().len_usize(),
        P::MaxAttestationsElectra::USIZE,
    )?;
    ensure_operation_count::<P>(
        "voluntary exits",
        body.voluntary_exits().len_usize(),
        P::MaxVoluntaryExits::USIZE,
    )?;
    ensure_operation_count::<P>(
        "BLS to execution changes",
        body.bls_to_execution_changes().len_usize(),
        P::MaxBlsToExecutionChanges::USIZE,
    )?;
    ensure_operation_count::<P>(
        "payload attestations",
        body.payload_attestations().len_usize(),
        P::MaxPayloadAttestation::USIZE,
    )?;

    for proposer_slashing in body.proposer_slashings().iter().copied() {
        process_proposer_slashing(
            config,
            pubkey_cache,
            state,
            proposer_slashing,
            &mut verifier,
            &mut slot_report,
        )?;
    }

    for attester_slashing in body.attester_slashings() {
        process_attester_slashing(
            config,
            pubkey_cache,
            state,
            attester_slashing,
            &mut verifier,
            &mut slot_report,
        )?;
    }

    // TODO: update on https://github.com/ethereum/consensus-specs/blob/v1.5.0-alpha.1/specs/electra/beacon-chain.md#modified-process_attestation
    // Parallel iteration with Rayon has some overhead, which is most noticeable when the active
    // thread pool is busy. `ParallelIterator::collect` appears to wait for worker threads to become
    // available even if the current thread is itself a worker thread. This tends to happen when
    // verifying signatures for batches of blocks outside the state transition function.
    // Fortunately, the other validations in `validate_attestation_with_verifier` take a negligible
    // amount of time, so we can avoid the issue by running them sequentially.
    if V::IS_NULL {
        for attestation in body.attestations() {
            validate_attestation_with_verifier(
                config,
                pubkey_cache,
                state,
                attestation,
                &mut verifier,
            )?;
        }
    } else {
        initialize_shuffled_indices(state, body.attestations().iter())?;

        let attestations = body.attestations().iter().collect::<Vec<_>>();
        let triples = helper_functions::par_iter!(attestations)
            .map(|attestation| {
                let mut triple = Triple::default();

                validate_attestation_with_verifier(
                    config,
                    pubkey_cache,
                    state,
                    attestation,
                    &mut triple,
                )?;

                Ok(triple)
            })
            .collect::<Result<Vec<_>>>()?;

        verifier.extend(triples, SignatureKind::Attestation)?;
    }

    for attestation in body.attestations() {
        apply_attestation(config, state, attestation, parent_slot, &mut slot_report)?;
    }

    for voluntary_exit in body.voluntary_exits().iter().copied() {
        electra::process_voluntary_exit(
            config,
            pubkey_cache,
            state,
            voluntary_exit,
            &mut verifier,
        )?;
    }

    for bls_to_execution_change in body.bls_to_execution_changes().iter().copied() {
        capella::process_bls_to_execution_change(
            config,
            pubkey_cache,
            state,
            bls_to_execution_change,
            &mut verifier,
        )?;
    }

    for payload_attestation in body.payload_attestations() {
        process_payload_attestation(
            config,
            pubkey_cache,
            state,
            payload_attestation,
            &mut verifier,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod spec_tests {
    use core::fmt::Debug;

    use helper_functions::{slot_report::NullSlotReport, verifier::NullVerifier};
    use spec_test_utils::{BlsSetting, Case, Meta};
    use ssz::SszReadDefault;
    use test_generator::test_resources;
    use types::{
        gloas::containers::{Attestation, AttesterSlashing},
        nonstandard::Phase,
        preset::{Mainnet, Minimal},
    };

    use super::*;
    use crate::{
        capella, electra, fulu,
        gloas::{
            apply_attestation,
            execution_payload_processing::{
                process_builder_deposit_request, process_builder_exit_request,
            },
            process_payload_attestation, process_proposer_slashing,
            validate_attestation_with_verifier,
        },
    };

    macro_rules! processing_tests {
        (
            $module_name: ident,
            $processing_function: expr,
            $operation_name: literal,
            $mainnet_glob: literal,
            $minimal_glob: literal,
        ) => {
            mod $module_name {
                use super::*;

                #[test_resources($mainnet_glob)]
                fn mainnet(case: Case) {
                    run_processing_case_specialized::<Mainnet>(case);
                }

                #[test_resources($minimal_glob)]
                fn minimal(case: Case) {
                    run_processing_case_specialized::<Minimal>(case);
                }

                fn run_processing_case_specialized<P: Preset>(case: Case) {
                    run_processing_case::<P, _>(case, $operation_name, $processing_function);
                }
            }
        };
    }

    macro_rules! validation_tests {
        (
            $module_name: ident,
            $validation_function: expr,
            $operation_name: literal,
            $mainnet_glob: literal,
            $minimal_glob: literal,
        ) => {
            mod $module_name {
                use super::*;

                #[test_resources($mainnet_glob)]
                fn mainnet(case: Case) {
                    run_validation_case_specialized::<Mainnet>(case);
                }

                #[test_resources($minimal_glob)]
                fn minimal(case: Case) {
                    run_validation_case_specialized::<Minimal>(case);
                }

                fn run_validation_case_specialized<P: Preset>(case: Case) {
                    run_validation_case::<P, _, _>(case, $operation_name, $validation_function);
                }
            }
        };
    }

    processing_tests! {
        process_deposit_request,
        |_, _, state, deposit_request, _| fulu::process_deposit_request(state, deposit_request),
        "deposit_request",
        "consensus-spec-tests/tests/mainnet/heze/operations/deposit_request/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/deposit_request/*/*",
    }

    processing_tests! {
        process_withdrawal_request,
        |config, _, state, withdrawal_request, _| electra::process_withdrawal_request(config, state, withdrawal_request),
        "withdrawal_request",
        "consensus-spec-tests/tests/mainnet/heze/operations/withdrawal_request/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/withdrawal_request/*/*",
    }

    processing_tests! {
        process_consolidation_request,
        |config, _, state, consolidation_request, _| electra::process_consolidation_request(config, state, consolidation_request),
        "consolidation_request",
        "consensus-spec-tests/tests/mainnet/heze/operations/consolidation_request/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/consolidation_request/*/*",
    }

    processing_tests! {
        process_builder_deposit_request,
        |config, pubkey_cache, state, builder_deposit_request, _| process_builder_deposit_request(config, pubkey_cache, state, builder_deposit_request),
        "builder_deposit_request",
        "consensus-spec-tests/tests/mainnet/heze/operations/builder_deposit_request/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/builder_deposit_request/*/*",
    }

    processing_tests! {
        process_builder_exit_request,
        |config, _, state, builder_exit_request, _| process_builder_exit_request(config, state, builder_exit_request),
        "builder_exit_request",
        "consensus-spec-tests/tests/mainnet/heze/operations/builder_exit_request/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/builder_exit_request/*/*",
    }

    // Test files for `process_block_header` are named `block.*` and contain `BeaconBlock`s.
    processing_tests! {
        process_block_header,
        |config, _, state, block: Hc<BeaconBlock<_>>, _| unphased::process_block_header(config, state, &block),
        "block",
        "consensus-spec-tests/tests/mainnet/heze/operations/block_header/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/block_header/*/*",
    }

    processing_tests! {
        process_proposer_slashing,
        |config, pubkey_cache, state, proposer_slashing, _| {
            process_proposer_slashing(
                config,
                pubkey_cache,
                state,
                proposer_slashing,
                SingleVerifier,
                NullSlotReport,
            )
        },
        "proposer_slashing",
        "consensus-spec-tests/tests/mainnet/heze/operations/proposer_slashing/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/proposer_slashing/*/*",
    }

    processing_tests! {
        process_attester_slashing,
        |config, pubkey_cache, state, attester_slashing: AttesterSlashing<P>, _| {
            electra::process_attester_slashing(
                config,
                pubkey_cache,
                state,
                &attester_slashing,
                SingleVerifier,
                NullSlotReport,
            )
        },
        "attester_slashing",
        "consensus-spec-tests/tests/mainnet/heze/operations/attester_slashing/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/attester_slashing/*/*",
    }

    processing_tests! {
        process_attestation,
        |config, pubkey_cache, state, attestation, meta| {
            process_attestation(
                config,
                pubkey_cache,
                state,
                &attestation,
                meta,
            )
        },
        "attestation",
        "consensus-spec-tests/tests/mainnet/heze/operations/attestation/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/attestation/*/*",
    }

    processing_tests! {
        process_bls_to_execution_change,
        |config, pubkey_cache, state, bls_to_execution_change, _| {
            capella::process_bls_to_execution_change(
                config,
                pubkey_cache,
                state,
                bls_to_execution_change,
                SingleVerifier,
            )
        },
        "address_change",
        "consensus-spec-tests/tests/mainnet/heze/operations/bls_to_execution_change/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/bls_to_execution_change/*/*",
    }

    processing_tests! {
        process_voluntary_exit,
        |config, pubkey_cache, state, voluntary_exit, _| {
            electra::process_voluntary_exit(
                config,
                pubkey_cache,
                state,
                voluntary_exit,
                SingleVerifier,
            )
        },
        "voluntary_exit",
        "consensus-spec-tests/tests/mainnet/heze/operations/voluntary_exit/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/voluntary_exit/*/*",
    }

    processing_tests! {
        process_voluntary_exit_churn,
        |config, pubkey_cache, state, voluntary_exit, _| {
            electra::process_voluntary_exit(
                config,
                pubkey_cache,
                state,
                voluntary_exit,
                SingleVerifier,
            )
        },
        "voluntary_exit",
        "consensus-spec-tests/tests/mainnet/heze/operations/voluntary_exit_churn/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/voluntary_exit_churn/*/*",
    }

    processing_tests! {
        process_sync_aggregate,
        |config, pubkey_cache, state, sync_aggregate, _| {
            altair::process_sync_aggregate(
                config,
                pubkey_cache,
                state,
                sync_aggregate,
                SingleVerifier,
                NullSlotReport,
            )
        },
        "sync_aggregate",
        "consensus-spec-tests/tests/mainnet/heze/operations/sync_aggregate/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/sync_aggregate/*/*",
    }

    processing_tests! {
        process_execution_payload_bid,
        |config, pubkey_cache, state, execution_payload_bid, _| {
            process_execution_payload_bid(
                config,
                pubkey_cache,
                state,
                &execution_payload_bid,
            )
        },
        "execution_payload_bid",
        "consensus-spec-tests/tests/mainnet/heze/operations/execution_payload_bid/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/execution_payload_bid/*/*",
    }

    processing_tests! {
        process_payload_attestation,
        |config, pubkey_cache, state, payload_attestation, _| {
            process_payload_attestation(
                config,
                pubkey_cache,
                state,
                &payload_attestation,
                SingleVerifier,
            )
        },
        "payload_attestation",
        "consensus-spec-tests/tests/mainnet/heze/operations/payload_attestation/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/payload_attestation/*/*",
    }

    processing_tests! {
        process_parent_execution_payload,
        |config, pubkey_cache, state, block, _| process_parent_execution_payload(config, pubkey_cache, state, &block),
        "block",
        "consensus-spec-tests/tests/mainnet/heze/operations/parent_execution_payload/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/parent_execution_payload/*/*",
    }

    validation_tests! {
        validate_proposer_slashing,
        |config, pubkey_cache, state, proposer_slashing| {
            unphased::validate_proposer_slashing(config, pubkey_cache, state, proposer_slashing)
        },
        "proposer_slashing",
        "consensus-spec-tests/tests/mainnet/heze/operations/proposer_slashing/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/proposer_slashing/*/*",
    }

    validation_tests! {
        validate_attester_slashing,
        |config, pubkey_cache, state, attester_slashing: AttesterSlashing<P>| {
            unphased::validate_attester_slashing(config, pubkey_cache, state, &attester_slashing)
        },
        "attester_slashing",
        "consensus-spec-tests/tests/mainnet/heze/operations/attester_slashing/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/attester_slashing/*/*",
    }

    validation_tests! {
        validate_voluntary_exit,
        |config, pubkey_cache, state, voluntary_exit| {
            electra::validate_voluntary_exit_with_verifier(config, pubkey_cache, state, voluntary_exit, SingleVerifier)
        },
        "voluntary_exit",
        "consensus-spec-tests/tests/mainnet/heze/operations/voluntary_exit/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/voluntary_exit/*/*",
    }

    // TODO(feature/electra): comment this & run missing test script
    validation_tests! {
        validate_bls_to_execution_change,
        |config, pubkey_cache, state, bls_to_execution_change| {
            capella::validate_bls_to_execution_change(config, pubkey_cache, state, bls_to_execution_change)
        },
        "address_change",
        "consensus-spec-tests/tests/mainnet/heze/operations/bls_to_execution_change/*/*",
        "consensus-spec-tests/tests/minimal/heze/operations/bls_to_execution_change/*/*",
    }

    #[test_resources("consensus-spec-tests/tests/mainnet/heze/operations/withdrawals/*/*")]
    fn mainnet_withdrawals(case: Case) {
        run_withdrawals_case::<Mainnet>(case);
    }

    #[test_resources("consensus-spec-tests/tests/minimal/heze/operations/withdrawals/*/*")]
    fn minimal_withdrawals(case: Case) {
        run_withdrawals_case::<Minimal>(case);
    }

    fn config<P: Preset>(state: &HezeBeaconState<P>) -> Config {
        P::default_config().upgrade_once(Phase::Heze, state.fork.epoch)
    }

    fn run_processing_case<P: Preset, O: SszReadDefault>(
        case: Case,
        operation_name: &str,
        processing_function: impl FnOnce(
            &Config,
            &PubkeyCache,
            &mut HezeBeaconState<P>,
            O,
            &Meta,
        ) -> Result<()>,
    ) {
        let pubkey_cache = PubkeyCache::default();
        let mut state = case.ssz_default("pre");
        let operation = case.ssz_default(operation_name);
        let post_option = case.try_ssz_default("post");
        let meta = case.meta();
        let config = config(&state);

        let result = processing_function(&config, &pubkey_cache, &mut state, operation, &meta)
            .map(|()| state);

        if let Some(expected_post) = post_option {
            let actual_post = result.expect("operation processing should succeed");
            assert_eq!(actual_post, expected_post);
        } else {
            result.expect_err("operation processing should fail");
        }
    }

    fn run_validation_case<P: Preset, O: SszReadDefault, R: Debug>(
        case: Case,
        operation_name: &str,
        validation_function: impl FnOnce(&Config, &PubkeyCache, &mut HezeBeaconState<P>, O) -> Result<R>,
    ) {
        let pubkey_cache = PubkeyCache::default();
        let mut state = case.ssz_default("pre");
        let operation = case.ssz_default(operation_name);
        let post_exists = case.exists("post");
        let config = config(&state);

        let result = validation_function(&config, &pubkey_cache, &mut state, operation);

        if post_exists {
            result.expect("validation should succeed");
        } else {
            result.expect_err("validation should fail");
        }
    }

    fn run_withdrawals_case<P: Preset>(case: Case) {
        let mut state = case.ssz_default::<HezeBeaconState<P>>("pre");
        let post_option = case.try_ssz_default("post");

        let result = process_withdrawals(&mut state).map(|()| state);

        if let Some(expected_post) = post_option {
            let actual_post = result.expect("withdrawals processing should succeed");
            assert_eq!(actual_post, expected_post);
        } else {
            result.expect_err("withdrawals processing should fail");
        }
    }

    fn process_attestation<P: Preset>(
        config: &Config,
        pubkey_cache: &PubkeyCache,
        state: &mut HezeBeaconState<P>,
        attestation: &Attestation<P>,
        meta: &Meta,
    ) -> Result<()> {
        match meta.bls_setting {
            BlsSetting::Optional | BlsSetting::Required => validate_attestation_with_verifier(
                config,
                pubkey_cache,
                state,
                attestation,
                SingleVerifier,
            )?,
            BlsSetting::Ignored => validate_attestation_with_verifier(
                config,
                pubkey_cache,
                state,
                attestation,
                NullVerifier,
            )?,
        }

        // The bid is not processed here, so the bid in the state
        // is still the parent block's bid and its slot is the parent block's slot.
        let parent_slot = meta
            .parent_slot
            .unwrap_or(state.latest_execution_payload_bid.slot);

        apply_attestation(config, state, attestation, parent_slot, NullSlotReport)
    }
}
