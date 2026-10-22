use anyhow::Result;
use arithmetic::{NonZeroExt as _, U64Ext as _, UsizeExt as _};
use helper_functions::{
    accessors::{
        get_activation_churn_limit, get_builder_payment_quorum_threshold, get_current_epoch,
        get_next_epoch, index_of_public_key, ptc_for_epoch_processing,
    },
    misc,
};
use itertools::Itertools as _;
use pubkey_cache::PubkeyCache;
use ssz::{PersistentVector, SszHash as _, SszListMut as _};
use try_from_iterator::TryFromIterator as _;
use typenum::Unsigned as _;
use types::{
    PtcWindow,
    cache::PackedIndices,
    capella::containers::HistoricalSummary,
    config::Config,
    eip8321::{beacon_state::BeaconState, containers::BuilderPendingPayment},
    nonstandard::RelativeEpoch,
    phase0::consts::FAR_FUTURE_EPOCH,
    preset::{BuilderPendingPaymentsLength, Preset},
    traits::{BeaconState as _, PostElectraBeaconState, PostGloasBeaconState},
};

use super::epoch_intermediates;
use crate::{
    altair::{self, EpochDeltasForTransition},
    electra::{self},
    fulu, unphased,
};

#[cfg(feature = "metrics")]
use prometheus_metrics::METRICS;

pub fn process_epoch<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut BeaconState<P>,
) -> Result<()> {
    #[cfg(feature = "metrics")]
    let _timer = METRICS
        .get()
        .map(|metrics| metrics.epoch_processing_times.start_timer());

    // TODO(Grandine Team): Some parts of epoch processing could be done in parallel.

    let (statistics, mut summaries, participation) = altair::statistics_and_summaries(state)?;

    altair::process_justification_and_finalization(state, statistics)?;

    altair::process_inactivity_updates(
        config,
        state,
        summaries.iter().copied(),
        participation.iter().copied(),
    )?;

    // Epoch deltas must be computed after `process_justification_and_finalization` and
    // `process_inactivity_updates` because they depend on updated values of
    // `BeaconState.finalized_checkpoint` and `BeaconState.inactivity_scores`.
    //
    // Using `vec_of_default` in the genesis epoch does not improve performance.
    let epoch_deltas: Vec<EpochDeltasForTransition> = epoch_intermediates::epoch_deltas(
        config,
        state,
        statistics,
        summaries.iter().copied(),
        participation,
    )?;

    unphased::process_rewards_and_penalties(state, epoch_deltas)?;
    electra::process_registry_updates(config, state, summaries.as_mut_slice())?;
    process_pending_randao_commitments(state)?;
    electra::process_slashings::<_, ()>(state, summaries)?;
    process_pending_deposits(config, pubkey_cache, state)?;
    electra::process_pending_consolidations(state)?;

    process_builder_pending_payments(state)?;
    electra::process_effective_balance_updates(state)?;
    unphased::process_slashings_reset(state)?;
    unphased::process_randao_mixes_reset(state)?;

    // > [Modified in Capella]
    process_historical_summaries_update(state)?;

    altair::process_participation_flag_updates(state);
    altair::process_sync_committee_updates(pubkey_cache, state)?;

    fulu::process_proposer_lookahead(config, state)?;

    // > [New in Gloas:EIP7732]
    let (ordered, shuffled) = process_ptc_window(state)?;

    state.cache.advance_epoch();

    if P::MinSeedLookahead::U64 == 1 {
        state.cache.active_validator_indices_ordered[RelativeEpoch::Next] = ordered.into();
        state.cache.active_validator_indices_shuffled[RelativeEpoch::Next] = shuffled.into();
    }

    Ok(())
}

/// Apply a pending deposit without creating new BLS-withdrawal validators.
///
/// EIP-8365 preserves top-ups to existing validators, even if the deposit
/// signature or supplied withdrawal credentials do not match.
pub fn apply_pending_deposit<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut BeaconState<P>,
    deposit: &types::electra::containers::PendingDeposit,
) -> Result<()> {
    if index_of_public_key(state, &deposit.pubkey).is_none()
        && deposit
            .withdrawal_credentials
            .as_bytes()
            .starts_with(types::phase0::consts::BLS_WITHDRAWAL_PREFIX)
    {
        return Ok(());
    }
    electra::apply_pending_deposit(config, pubkey_cache, state, deposit)
}

#[expect(
    clippy::useless_let_if_seq,
    reason = "assignments with multiple variables are more readable with conditional affectation"
)]
pub fn process_pending_deposits<P: Preset>(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut BeaconState<P>,
) -> Result<()> {
    let next_epoch = get_current_epoch(state).try_add(1)?;
    let available_for_processing = state
        .deposit_balance_to_consume()
        .try_add(get_activation_churn_limit(config, state))?;

    let mut processed_amount: u64 = 0;
    let mut next_deposit_index: u64 = 0;
    let mut deposits_to_postpone = vec![];
    let mut is_churn_limit_reached = false;
    let finalized_slot = misc::compute_start_slot_at_epoch::<P>(state.finalized_checkpoint().epoch);

    for deposit in &*state.pending_deposits().clone_boxed() {
        // > Check if deposit has been finalized, otherwise, stop processing.
        if deposit.slot > finalized_slot {
            break;
        }

        // > Check if number of processed deposits has not reached the limit, otherwise, stop processing.
        if next_deposit_index >= P::MAX_PENDING_DEPOSITS_PER_EPOCH {
            break;
        }

        let mut is_validator_exited = false;
        let mut is_validator_withdrawn = false;

        if let Some(validator_index) = index_of_public_key(state, &deposit.pubkey) {
            let validator = state.validators().get(validator_index)?;

            is_validator_exited = validator.exit_epoch < FAR_FUTURE_EPOCH;
            is_validator_withdrawn = validator.withdrawable_epoch < next_epoch;
        }

        if is_validator_withdrawn {
            // > Deposited balance will never become active. Increase balance but do not consume churn
            apply_pending_deposit(config, pubkey_cache, state, deposit)?;
        } else if is_validator_exited {
            // > Validator is exiting, postpone the deposit until after withdrawable epoch
            deposits_to_postpone.push(*deposit);
        } else {
            // > Check if deposit fits in the churn, otherwise, do no more deposit processing in this epoch.
            is_churn_limit_reached =
                processed_amount.try_add(deposit.amount)? > available_for_processing;

            if is_churn_limit_reached {
                break;
            }

            processed_amount = processed_amount.try_add(deposit.amount)?;
            apply_pending_deposit(config, pubkey_cache, state, deposit)?;
        }

        next_deposit_index = next_deposit_index.try_add(1)?;
    }

    let pending_deposits = state.pending_deposits().clone_boxed();

    state.pending_deposits_mut().try_assign_from_iter(
        &mut pending_deposits
            .iter()
            .copied()
            .skip(next_deposit_index.try_into()?)
            .chain(deposits_to_postpone),
    )?;

    if is_churn_limit_reached {
        *state.deposit_balance_to_consume_mut() =
            available_for_processing.try_sub(processed_amount)?;
    } else {
        *state.deposit_balance_to_consume_mut() = 0;
    }

    Ok(())
}

fn process_ptc_window<P: Preset>(
    state: &mut impl PostGloasBeaconState<P>,
) -> Result<(PackedIndices, PackedIndices)> {
    let mut ptc_window = state.ptc_window().into_iter().collect::<Vec<_>>();
    let last_epoch_start = ptc_window.len().try_sub(P::SlotsPerEpoch::USIZE)?;

    ptc_window.copy_within(P::SlotsPerEpoch::USIZE.., 0);

    let target_epoch = get_current_epoch(state).try_add(P::MinSeedLookahead::U64.try_add(1)?)?;

    let (ptcs, ordered, shuffled) = ptc_for_epoch_processing(state, target_epoch)?;

    let refs = ptcs.iter().collect::<Vec<_>>();
    ptc_window[last_epoch_start..].copy_from_slice(&refs);

    *state.ptc_window_mut() = PtcWindow::<P>::try_from_iter(ptc_window.into_iter().cloned())?;

    Ok((ordered, shuffled))
}

fn process_builder_pending_payments<P: Preset>(
    state: &mut impl PostGloasBeaconState<P>,
) -> Result<()> {
    let quorum = get_builder_payment_quorum_threshold(state)?;
    let payments = state
        .builder_pending_payments()
        .into_iter()
        .copied()
        .collect_vec();

    for payment in payments.iter().take(P::SlotsPerEpoch::USIZE) {
        if payment.weight >= quorum {
            state
                .builder_pending_withdrawals_mut()
                .push(payment.withdrawal)?;
        }
    }

    *state.builder_pending_payments_mut() = PersistentVector::try_from_iter(
        payments
            .into_iter()
            .skip(P::SlotsPerEpoch::USIZE)
            .chain(core::iter::repeat_n(
                BuilderPendingPayment::default(),
                P::SlotsPerEpoch::USIZE,
            ))
            .take(BuilderPendingPaymentsLength::<P>::USIZE),
    )?;

    Ok(())
}

fn process_historical_summaries_update<P: Preset>(state: &mut BeaconState<P>) -> Result<()> {
    let next_epoch = get_next_epoch(state)?;

    // > Set historical block root accumulator.
    if next_epoch.is_multiple_of(P::EpochsPerHistoricalRoot::non_zero().into()) {
        let historical_summary = HistoricalSummary {
            block_summary_root: state.block_roots().hash_tree_root(),
            state_summary_root: state.state_roots().hash_tree_root(),
        };

        state.historical_summaries.push(historical_summary)?;
    }

    Ok(())
}

// Official Heze vectors are exercised by tests/heze_reference_vectors.rs.

pub fn process_pending_randao_commitments<P: Preset>(state: &mut BeaconState<P>) -> Result<()> {
    let next_epoch = get_next_epoch(state)?;
    let mut consumed: usize = 0;
    for pending in &state.pending_randao_commitments {
        if pending.activation_epoch > next_epoch {
            break;
        }
        *state.randao_commitments.get_mut(pending.validator_index)? = pending.commitment;
        consumed = consumed.try_add(1)?;
    }
    state.pending_randao_commitments = ssz::PersistentProgressiveList::try_from_iter(
        state
            .pending_randao_commitments
            .into_iter()
            .copied()
            .skip(consumed),
    )?;
    Ok(())
}
