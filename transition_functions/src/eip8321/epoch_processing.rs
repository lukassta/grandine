use anyhow::Result;
use arithmetic::NonZeroExt as _;
use arithmetic::U64Ext as _;
use arithmetic::UsizeExt as _;
use helper_functions::accessors::get_next_epoch;
use helper_functions::{
    accessors::{get_activation_churn_limit, get_current_epoch, index_of_public_key},
    misc,
};
use pubkey_cache::PubkeyCache;
use ssz::{SszHash as _, SszListMut as _};
use try_from_iterator::TryFromIterator as _;
use types::{
    capella::containers::HistoricalSummary, config::Config, eip8321::beacon_state::BeaconState,
    preset::Preset, traits::BeaconState as _,
};
use types::{phase0::consts::FAR_FUTURE_EPOCH, traits::PostElectraBeaconState as _};

use super::epoch_intermediates;
use crate::{
    altair::{self, EpochDeltasForTransition},
    electra, fulu,
    gloas::{process_builder_pending_payments, process_ptc_window},
    unphased,
};

#[cfg(feature = "metrics")]
use prometheus_metrics::METRICS;

pub fn process_epoch(
    config: &Config,
    pubkey_cache: &PubkeyCache,
    state: &mut BeaconState<impl Preset>,
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
    process_ptc_window(state)?;

    state.cache.advance_epoch();

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
