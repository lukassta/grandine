use anyhow::Result;
use arithmetic::{NonZeroU64Ext as _, U64Ext as _};
use helper_functions::{
    accessors::{compute_base_reward, get_base_reward_per_increment, total_active_balance},
    predicates::is_in_inactivity_leak,
};
use itertools::izip;
use types::{
    altair::consts::{
        TIMELY_HEAD_WEIGHT, TIMELY_SOURCE_WEIGHT, TIMELY_TARGET_WEIGHT, WEIGHT_DENOMINATOR,
    },
    config::Config,
    heze::beacon_state::BeaconState,
    nonstandard::Participation,
    phase0::primitives::Gwei,
    preset::Preset,
};

use crate::altair::{EpochDeltas, Statistics, ValidatorSummary};

pub fn epoch_deltas<P: Preset, D: EpochDeltas>(
    config: &Config,
    state: &BeaconState<P>,
    statistics: Statistics,
    summaries: impl IntoIterator<Item = ValidatorSummary>,
    participation: impl IntoIterator<Item = Participation>,
) -> Result<Vec<D>> {
    let in_inactivity_leak = is_in_inactivity_leak(state)?;
    let base_reward_per_increment = get_base_reward_per_increment(state)?;

    let increment = P::EFFECTIVE_BALANCE_INCREMENT;
    let source_increments = statistics.previous_epoch_source_participating_balance / increment;
    let target_increments = statistics.previous_epoch_target_participating_balance / increment;
    let head_increments = statistics.previous_epoch_head_participating_balance / increment;
    let active_increments = total_active_balance(state) / increment;

    izip!(summaries, participation, &state.inactivity_scores)
        .map(|(summary, participation, inactivity_score)| -> Result<D> {
            let mut deltas = D::default();

            let ValidatorSummary {
                effective_balance,
                slashed,
                eligible_for_penalties,
                ..
            } = summary;

            if !eligible_for_penalties {
                return Ok(deltas);
            }

            let base_reward =
                compute_base_reward::<P>(effective_balance, base_reward_per_increment)?;

            let participation_component_reward =
                |weight, unslashed_participating_increments| -> Result<Gwei> {
                    let reward_numerator = base_reward
                        .try_mul(weight)?
                        .try_mul(unslashed_participating_increments)?;

                    let reward_denominator = active_increments.try_mul(WEIGHT_DENOMINATOR.get())?;

                    Ok(reward_numerator.try_div(reward_denominator)?)
                };

            let participation_component_penalty =
                |weight| -> Result<Gwei> { Ok(base_reward.try_mul(weight)? / WEIGHT_DENOMINATOR) };

            if !slashed && participation.previous_epoch_matching_source() {
                if !in_inactivity_leak {
                    deltas.add_source_reward(participation_component_reward(
                        TIMELY_SOURCE_WEIGHT,
                        source_increments,
                    )?)?;
                }
            } else {
                deltas
                    .add_source_penalty(participation_component_penalty(TIMELY_SOURCE_WEIGHT)?)?;
            }

            if !slashed && participation.previous_epoch_matching_target() {
                if !in_inactivity_leak {
                    deltas.add_target_reward(participation_component_reward(
                        TIMELY_TARGET_WEIGHT,
                        target_increments,
                    )?)?;
                }
            } else {
                deltas
                    .add_target_penalty(participation_component_penalty(TIMELY_TARGET_WEIGHT)?)?;

                let penalty_numerator = effective_balance.try_mul(*inactivity_score)?;
                let penalty_denominator = config
                    .inactivity_score_bias
                    .try_mul(P::INACTIVITY_PENALTY_QUOTIENT_BELLATRIX)?;

                deltas.add_inactivity_penalty(penalty_numerator / penalty_denominator)?;
            }

            if !slashed && participation.previous_epoch_matching_head() && !in_inactivity_leak {
                deltas.add_head_reward(participation_component_reward(
                    TIMELY_HEAD_WEIGHT,
                    head_increments,
                )?)?;
            }

            Ok(deltas)
        })
        .collect()
}

// Official Heze vectors are exercised by tests/heze_reference_vectors.rs.
