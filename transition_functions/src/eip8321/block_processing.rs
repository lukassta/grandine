use crate::gloas::{
    apply_attestation, process_attester_slashing, process_payload_attestation,
    process_proposer_slashing, validate_attestation_with_verifier,
};
use crate::{capella, electra};
use anyhow::{Result, ensure};
use arithmetic::U64Ext as _;
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
use ssz::SszListMut as _;
use ssz::{Hc, SszHash as _, SszList as _};
use typenum::Unsigned as _;
use types::traits::{BeaconBlockBody as _, SszValidatorList as _};
use types::{
    config::Config,
    eip8321::{
        beacon_state::BeaconState as Eip8321BeaconState,
        containers::{
            BeaconBlock, ExecutionPayloadBid, SignedBeaconBlock, SignedExecutionPayloadBid,
        },
    },
    gloas::{
        consts::{BUILDER_INDEX_SELF_BUILD, PAYLOAD_BUILDER_VERSION},
        containers::{BuilderPendingPayment, BuilderPendingWithdrawal, ExecutionRequests},
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
    state: &mut Eip8321BeaconState<P>,
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
    state: &Eip8321BeaconState<P>,
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
        .try_add(block.body.payload_attestations.len())?
        .try_add(block.body.randao_commitment_registrations.len())
        .map_err(Into::into)
}

pub fn custom_process_block<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut Eip8321BeaconState<P>,
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

    process_randao(config, pubkey_cache, state, &block.body, &mut verifier)?;

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
    state: &mut Eip8321BeaconState<P>,
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

#[expect(
    clippy::too_many_lines,
    reason = "Keep the specified consensus operation ordering explicit"
)]
pub fn process_operations<P: Preset, V: Verifier>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut Eip8321BeaconState<P>,
    body: &types::eip8321::containers::BeaconBlockBody<P>,
    parent_slot: Slot,
    mut verifier: V,
    mut slot_report: impl SlotReport,
) -> Result<()> {
    ensure_operation_count::<P>(
        "RANDAO commitment registrations",
        body.randao_commitment_registrations.len(),
        P::MAX_RANDAO_COMMITMENT_REGISTRATIONS,
    )?;
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

    for registration in &body.randao_commitment_registrations {
        process_randao_commitment_registration(
            config,
            pubkey_cache,
            state,
            registration,
            &mut verifier,
        )?;
    }

    Ok(())
}

/// BLAKE3 v1, unkeyed, 32-byte output. SSZ and the legacy path still use SHA-256.
fn blake3_parts(parts: &[&[u8]]) -> types::phase0::primitives::H256 {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    types::phase0::primitives::H256(*hasher.finalize().as_bytes())
}

pub fn verify_hash_chain_reveal<P: Preset>(
    state: &Eip8321BeaconState<P>,
    body: &types::eip8321::containers::BeaconBlockBody<P>,
    proposer_index: u64,
) -> Result<()> {
    use types::eip8321::consts::HASH_CHAIN_RANDAO_DST;
    ensure!(!body.hash_chain_reveal.is_zero(), "zero hash-chain reveal");
    // Compressed G2 infinity is c0 followed by zero bytes; all-zero is not infinity.
    ensure!(
        body.randao_reveal == bls::SignatureBytes::empty(),
        "active hash chain requires G2 infinity RANDAO signature"
    );
    ensure!(
        blake3_parts(&[HASH_CHAIN_RANDAO_DST, body.hash_chain_reveal.as_bytes()])
            == *state.randao_commitments.get(proposer_index)?,
        "hash-chain reveal is not the commitment preimage",
    );
    Ok(())
}

pub fn process_randao<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut Eip8321BeaconState<P>,
    body: &types::eip8321::containers::BeaconBlockBody<P>,
    verifier: impl Verifier,
) -> Result<()> {
    let index = get_beacon_proposer_index(config, state)?;
    if state.randao_commitments.get(index)?.is_zero() {
        ensure!(
            body.hash_chain_reveal.is_zero(),
            "unregistered proposer supplied hash-chain reveal"
        );
        unphased::process_randao(config, pubkey_cache, state, body, verifier)
    } else {
        verify_hash_chain_reveal(state, body, index)?;
        let epoch = get_current_epoch(state);
        let mix = blake3_parts(&[
            get_randao_mix(state, epoch).as_bytes(),
            body.hash_chain_reveal.as_bytes(),
        ]);
        *state.randao_mixes.mod_index_mut(epoch) = mix;
        *state.randao_commitments.get_mut(index)? = body.hash_chain_reveal;
        Ok(())
    }
}

pub fn process_randao_commitment_registration<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut Eip8321BeaconState<P>,
    registration: &types::eip8321::containers::SignedRandaoCommitmentRegistration,
    mut verifier: impl Verifier,
) -> Result<()> {
    use helper_functions::signing::SignForAllForksWithGenesis as _;
    let message = &registration.message;
    let index = message.validator_index;
    ensure!(
        index < state.validators.len_u64(),
        "unknown RANDAO registration validator"
    );
    ensure!(!message.commitment.is_zero(), "zero RANDAO commitment");
    ensure!(
        state.randao_commitments.get(index)?.is_zero(),
        "validator already registered"
    );
    ensure!(
        !state
            .pending_randao_commitments
            .into_iter()
            .any(|pending| pending.validator_index == index),
        "validator already has pending registration"
    );
    verifier.verify_singular(
        message.signing_root(config, state),
        registration.signature,
        pubkey_cache.get_or_insert(*accessors::public_key(state, index)?)?,
        SignatureKind::RandaoCommitmentRegistration,
    )?;
    state
        .pending_randao_commitments
        .push(types::eip8321::containers::PendingRandaoCommitment {
            validator_index: index,
            commitment: message.commitment,
            activation_epoch: get_current_epoch(state).try_add(P::COMMITMENT_REGISTRATION_DELAY)?,
        })?;
    Ok(())
}
