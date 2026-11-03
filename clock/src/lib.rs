//! A [`Stream`]-based timer for the Ethereum 2.0 Beacon Chain.
//!
//! # Implementation
//!
//! This is implemented using [`try_unfold`] with [`sleep_until`]. Each iteration sleeps until the
//! next tick's deadline, then advances the deadline by the current tick duration before looping.
//! Some subtleties to keep in mind:
//!
//! - The API of [`sleep_until`] (as well as other timer utilities in [`tokio::time`]) uses
//!   [`Instant`]s. [`Instant`]s are opaque. There is no way to directly convert a timestamp
//!   (of any kind, not just Unix time) to an [`Instant`]. The hack in [`ticks`] may result in
//!   unexpected behavior in extreme conditions.
//!
//! - The tick duration is recomputed at every tick boundary so that fork transitions (e.g. from
//!   pre-Gloas 12-tick slots to post-Gloas 16-tick slots) are handled correctly without any
//!   accumulated error.
//!
//! - It is unclear how [`sleep_until`] behaves around leap seconds.
//!
//! # Possible alternatives
//!
//! There are several other crates we could choose from:
//! - [`clokwerk`]
//! - [`job_scheduler`]
//! - [`schedule`]
//! - [`timer`]
//! - [`white_rabbit`]
//!
//! The first 3 do not come with any timers or runtimes. They need to be driven manually:
//! ```ignore
//! loop {
//!     scheduler.run_pending();
//!     thread::sleep(duration);
//! }
//! ```
//! This would have some benefits:
//! - By varying the sleep duration, we could trade higher CPU usage for higher precision.
//! - Leap seconds should be handled correctly without any extra effort on our part.
//!
//! [`timer`] and [`white_rabbit`] use timers internally.
//! They are likely to be more efficient, but it is unclear if they handle leap seconds correctly.
//!
//! None of these libraries are designed to work with futures, but making them work together should
//! be as simple as using a channel.
//!
//! [`tokio::time`]:  tokio::time
//! [`Instant`]:      tokio::time::Instant
//! [`sleep_until`]:  tokio::time::sleep_until
//! [`try_unfold`]:   futures::stream::try_unfold
//!
//! [`clokwerk`]:      https://crates.io/crates/clokwerk
//! [`job_scheduler`]: https://crates.io/crates/job_scheduler
//! [`schedule`]:      https://crates.io/crates/schedule
//! [`timer`]:         https://crates.io/crates/timer
//! [`white_rabbit`]:  https://crates.io/crates/white_rabbit

use core::{error::Error, pin::Pin, time::Duration};
use std::time::{Instant, SystemTime, SystemTimeError};

use anyhow::Result;
use enum_iterator::Sequence;
use futures::stream::Stream;
use helper_functions::misc;
use serde::Deserialize;
use strum::AsRefStr;
use thiserror::Error;
use types::{
    config::Config,
    nonstandard::Phase,
    phase0::{
        consts::GENESIS_SLOT,
        primitives::{Epoch, Slot, UnixSeconds},
    },
    preset::Preset,
    traits::{BeaconBlock as _, SignedBeaconBlock},
};

#[cfg(test)]
mod fake_time;

pub trait InstantLike: Sized {
    fn checked_add(self, duration: Duration) -> Option<Self>;
}

pub trait SystemTimeLike: Copy {
    type Error: Error + Send + Sync + 'static;

    const UNIX_EPOCH: Self;

    fn duration_since(self, earlier: Self) -> Result<Duration, Self::Error>;
}

impl InstantLike for Instant {
    fn checked_add(self, duration: Duration) -> Option<Self> {
        Self::checked_add(&self, duration)
    }
}

impl InstantLike for tokio::time::Instant {
    fn checked_add(self, duration: Duration) -> Option<Self> {
        Self::checked_add(&self, duration)
    }
}

impl SystemTimeLike for SystemTime {
    type Error = SystemTimeError;

    const UNIX_EPOCH: Self = Self::UNIX_EPOCH;

    fn duration_since(self, earlier: Self) -> Result<Duration, Self::Error> {
        Self::duration_since(&self, earlier)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
pub struct Tick {
    #[serde(with = "serde_utils::string_or_native")]
    pub slot: Slot,
    pub kind: TickKind,
}

impl Tick {
    #[must_use]
    pub const fn start_of_slot(slot: Slot) -> Self {
        Self::new(slot, TickKind::Propose)
    }

    #[must_use]
    pub fn block_proposal<P: Preset>(block: &impl SignedBeaconBlock<P>) -> Self {
        Self::new(block.message().slot(), TickKind::Propose)
    }

    pub fn at_time<P: Preset>(
        config: &Config,
        time: UnixSeconds,
        genesis_time: UnixSeconds,
    ) -> Result<Self> {
        let duration_since_unix_epoch = Duration::from_secs(time);
        Self::from_duration::<P>(config, duration_since_unix_epoch, genesis_time)
    }

    pub fn at_time_in_ms<P: Preset>(
        config: &Config,
        time: u64,
        genesis_time: UnixSeconds,
    ) -> Result<Self> {
        let duration_since_unix_epoch = Duration::from_millis(time);
        Self::from_duration::<P>(config, duration_since_unix_epoch, genesis_time)
    }

    pub fn current<P: Preset>(config: &Config, genesis_time: UnixSeconds) -> Result<Self> {
        let duration_since_unix_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
        Self::from_duration::<P>(config, duration_since_unix_epoch, genesis_time)
    }

    #[must_use]
    pub fn epoch<P: Preset>(self) -> Epoch {
        misc::compute_epoch_at_slot::<P>(self.slot)
    }

    #[must_use]
    pub const fn is_before_attesting_interval(self) -> bool {
        matches!(
            self.kind,
            TickKind::Propose
                | TickKind::ProposeSecond
                | TickKind::ProposeThird
                | TickKind::ProposeFourth,
        )
    }

    #[must_use]
    pub fn is_before_payload_attesting_interval(self) -> bool {
        self.kind < TickKind::PayloadAttest
    }

    #[must_use]
    pub const fn is_start_of_slot(self) -> bool {
        matches!(self.kind, TickKind::Propose)
    }

    #[must_use]
    pub fn is_start_of_epoch<P: Preset>(self) -> bool {
        misc::is_epoch_start::<P>(self.slot) && self.is_start_of_slot()
    }

    #[must_use]
    pub const fn is_start_of_interval(self) -> bool {
        matches!(
            self.kind,
            TickKind::Propose | TickKind::Attest | TickKind::Aggregate | TickKind::PayloadAttest,
        )
    }

    #[must_use]
    pub fn is_end_of_slot<P: Preset>(self, config: &Config) -> bool {
        if config.phase_at_slot::<P>(self.slot) >= Phase::Gloas {
            matches!(self.kind, TickKind::PayloadAttest)
        } else {
            matches!(self.kind, TickKind::Aggregate)
        }
    }

    #[must_use]
    pub fn is_last_tick<P: Preset>(self, config: &Config) -> bool {
        if config.phase_at_slot::<P>(self.slot) >= Phase::Gloas {
            matches!(self.kind, TickKind::PayloadAttestFourth)
        } else {
            matches!(self.kind, TickKind::AggregateFourth)
        }
    }

    #[must_use]
    pub const fn is_end_of_interval(self) -> bool {
        matches!(
            self.kind,
            TickKind::AttestFourth
                | TickKind::AggregateFourth
                | TickKind::ProposeFourth
                | TickKind::PayloadAttestFourth,
        )
    }

    pub fn delay<P: Preset>(self, config: &Config, genesis_time: UnixSeconds) -> Result<Duration> {
        let duration_since_unix_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
        let unix_epoch_to_genesis = Duration::from_secs(genesis_time);

        let duration_since_genesis =
            duration_since_unix_epoch.saturating_sub(unix_epoch_to_genesis);

        // Not `slot_duration * slots_since_genesis + tick_duration * kind`: the slot
        // duration can change at the Heze boundary, and a rounded tick duration does not
        // fill a slot exactly, so the offset is taken from genesis directly.
        let duration_until_tick = duration_from_genesis_at_tick::<P>(config, self);

        Ok(duration_since_genesis.saturating_sub(duration_until_tick))
    }

    fn from_duration<P: Preset>(
        config: &Config,
        duration_since_unix_epoch: Duration,
        genesis_time: UnixSeconds,
    ) -> Result<Self> {
        let unix_epoch_to_genesis = Duration::from_secs(genesis_time);

        let duration_since_genesis =
            duration_since_unix_epoch.saturating_sub(unix_epoch_to_genesis);

        // First, determine the slot. The slot duration can change at the Heze boundary, so
        // this is not a single division, and the tick index cannot be derived by taking a
        // remainder over all ticks since genesis either.
        let slot = config.slot_at_duration_from_genesis::<P>(duration_since_genesis);

        // Then, determine which tick within that slot, from the offset into the slot.
        //
        // `Duration` does not implement `Div<Duration>`, so we have to do arithmetic on
        // nanoseconds. The division comes last, inverting `tick_offset_in_slot`, so that a
        // rounded tick duration cannot misplace the last tick of a slot.
        let nanos_since_slot = duration_since_genesis
            .saturating_sub(config.duration_from_genesis_at_slot::<P>(slot))
            .as_nanos();

        let slot_duration = config.slot_duration_at_slot::<P>(slot).as_nanos();
        let ticks_per_slot = u128::try_from(TickKind::ticks_per_slot::<P>(config, slot))?;

        let ticks_since_slot = usize::try_from(
            nanos_since_slot
                .saturating_mul(ticks_per_slot)
                .checked_div(slot_duration)
                .expect("slot duration must be at least one nanosecond")
                // The slot was resolved from the same offset, so this only guards against
                // a rounding edge exactly on the next slot's boundary.
                .min(ticks_per_slot.saturating_sub(1)),
        )?;

        let kind = TickKind::from_index_at_slot::<P>(config, slot, ticks_since_slot)
            .expect("the offset into a slot cannot add up to a whole slot of ticks");

        Ok(Self::new(slot, kind))
    }

    const fn new(slot: Slot, kind: TickKind) -> Self {
        Self { slot, kind }
    }

    fn next<P: Preset>(self, config: &Config) -> Result<Self> {
        let Self { slot, kind } = self;

        let next_slot = match kind.next_at_slot::<P>(config, slot) {
            Some(_) => slot,
            None => slot.checked_add(1).ok_or(ClockError::RanOutOfSlots)?,
        };

        let next_kind = kind.next_cycle_at_slot::<P>(config, slot);

        Ok(Self::new(next_slot, next_kind))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Sequence, AsRefStr, Deserialize)]
pub enum TickKind {
    Propose,
    ProposeSecond,
    ProposeThird,
    ProposeFourth,
    Attest,
    AttestSecond,
    AttestThird,
    AttestFourth,
    Aggregate,
    AggregateSecond,
    AggregateThird,
    AggregateFourth,
    PayloadAttest,
    PayloadAttestSecond,
    PayloadAttestThird,
    PayloadAttestFourth,
}

impl TickKind {
    fn from_index_at_slot<P: Preset>(config: &Config, slot: Slot, index: usize) -> Option<Self> {
        if config.phase_at_slot::<P>(slot) >= Phase::Gloas {
            enum_iterator::all::<Self>().nth(index)
        } else {
            Self::pre_gloas_ticks().nth(index)
        }
    }

    fn next_at_slot<P: Preset>(self, config: &Config, slot: Slot) -> Option<Self> {
        if config.phase_at_slot::<P>(slot) >= Phase::Gloas {
            self.next()
        } else {
            let current_idx = Self::pre_gloas_ticks().position(|k| k == self)?;
            let next_idx = current_idx.saturating_add(1);

            Self::pre_gloas_ticks().nth(next_idx)
        }
    }

    fn next_cycle_at_slot<P: Preset>(self, config: &Config, slot: Slot) -> Self {
        self.next_at_slot::<P>(config, slot)
            .unwrap_or(Self::Propose)
    }

    /// Returns the number of tick variants in use for a given slot.
    /// Pre-Gloas: 12 ticks (3 intervals * 4 ticks each)
    /// Post-Gloas: 16 ticks (4 intervals * 4 ticks each)
    pub fn ticks_per_slot<P: Preset>(config: &Config, slot: Slot) -> usize {
        if config.phase_at_slot::<P>(slot) >= Phase::Gloas {
            Self::CARDINALITY // 4 intervals of 4 ticks each
        } else {
            Self::pre_gloas_ticks().count() // 3 intervals of 4 ticks each
        }
    }

    fn pre_gloas_ticks() -> impl Iterator<Item = Self> {
        enum_iterator::all::<Self>().take_while(|t| !matches!(t, Self::PayloadAttest))
    }
}

#[derive(Debug, Error)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum ClockError {
    #[error("time of next tick overflowed")]
    NextInstantOverflow,
    #[error("ran out of slots")]
    RanOutOfSlots,
}

pub fn ticks<P: Preset>(
    config: &Config,
    genesis_time_in_ms: u64,
) -> Result<Pin<Box<dyn Stream<Item = Result<Tick>> + Send + '_>>> {
    // We assume the `Instant` and `SystemTime` obtained here correspond to the same point in time.
    // This is slightly inaccurate but the error will probably be negligible compared to clock
    // differences between different nodes in the network.
    let now_instant = tokio::time::Instant::now();
    let now_system_time = SystemTime::now();

    let (next_tick, next_instant) = next_tick_with_instant::<P, _, _>(
        config,
        now_instant,
        now_system_time,
        genesis_time_in_ms,
        false,
    )?;

    Ok(Box::pin(futures::stream::try_unfold(
        (next_tick, next_instant),
        move |(mut next_tick, mut deadline)| {
            async move {
                loop {
                    tokio::time::sleep_until(deadline).await;

                    let current_tick = next_tick;
                    next_tick = current_tick.next::<P>(config)?;

                    // Advance by the gap between the two ticks' offsets from genesis rather
                    // than by a rounded tick duration. Both ends are absolute, so the
                    // deadlines cannot drift however long the stream runs -- adding a
                    // rounded duration each time loses 8 ms per slot at Heze's 5 s slots.
                    let advance = duration_from_genesis_at_tick::<P>(config, next_tick)
                        .saturating_sub(duration_from_genesis_at_tick::<P>(config, current_tick));

                    deadline = deadline
                        .checked_add(advance)
                        .ok_or(ClockError::NextInstantOverflow)?;

                    // Emit only ticks that the application currently uses.
                    // From Heze, `AggregateSecond` triggers own inclusion lists.
                    if current_tick.is_start_of_interval()
                        || current_tick.is_end_of_interval()
                        || (current_tick.kind == TickKind::AggregateSecond
                            && config.phase_at_slot::<P>(current_tick.slot) >= Phase::Heze)
                    {
                        return Ok(Some((current_tick, (next_tick, deadline))));
                    }
                }
            }
        },
    )))
}

pub fn next_interval_with_remaining_time<P: Preset>(
    config: &Config,
    genesis_time: UnixSeconds,
) -> Result<(Tick, Duration)> {
    // We assume the `Instant` and `SystemTime` obtained here correspond to the same point in time.
    // This is slightly inaccurate but the error will probably be negligible compared to clock
    // differences between different nodes in the network.
    let now_instant = Instant::now();
    let now_system_time = SystemTime::now();

    let genesis_time_in_ms = genesis_time.saturating_mul(1000);
    let (next_interval, next_instant) = next_tick_with_instant::<P, _, _>(
        config,
        now_instant,
        now_system_time,
        genesis_time_in_ms,
        true,
    )?;

    let remaining_time = next_instant.duration_since(now_instant);

    Ok((next_interval, remaining_time))
}

fn next_tick_with_instant<P: Preset, I: InstantLike, S: SystemTimeLike>(
    config: &Config,
    now_instant: I,
    now_system_time: S,
    genesis_time_in_ms: u64,
    only_interval_ticks: bool,
) -> Result<(Tick, I)> {
    let unix_epoch_to_now = now_system_time.duration_since(S::UNIX_EPOCH)?;

    // Tick now operates in milliseconds, so to prevent precision loss by rounding down to unix seconds,
    // We pass genesis time in milliseconds to prevent rounding error and precision calculation and
    // comparision.
    let unix_epoch_to_genesis = Duration::from_millis(genesis_time_in_ms);

    // Some platforms do not support negative `Instant`s. Operations that would produce an `Instant`
    // corresponding to time before the epoch will panic on those platforms. The epoch in question
    // is not the Unix epoch but a platform dependent value, typically the system boot time.
    // This means we are not allowed to subtract `Duration`s from `Instant`s. The `InstantLike`
    // trait conveniently prevents us from doing so.

    let mut next_tick;
    let mut now_to_next_tick;

    // `consensus-specs` originally did not make it clear whether the number of the first slot after
    // genesis is 0 or 1. We assumed it was 1. That way every slot could have a block proposed in
    // it. Thanks to Lighthouse and several new functions in `consensus-specs`
    // (`compute_time_at_slot` and `get_slots_since_genesis`) we now know it's 0.

    if unix_epoch_to_now <= unix_epoch_to_genesis {
        next_tick = Tick::start_of_slot(GENESIS_SLOT);
        now_to_next_tick = unix_epoch_to_genesis
            .checked_sub(unix_epoch_to_now)
            .expect("the difference from genesis to now fits in Duration");
    } else {
        let genesis_to_now = unix_epoch_to_now
            .checked_sub(unix_epoch_to_genesis)
            .expect("the difference from now to genesis fits in Duration");

        // The slot duration can change at the Heze boundary, so the current slot is not
        // `genesis_to_now / slot_duration`.
        let current_slot = config.slot_at_duration_from_genesis::<P>(genesis_to_now);
        let genesis_to_current_slot = config.duration_from_genesis_at_slot::<P>(current_slot);

        let current_slot_to_now = genesis_to_now
            .checked_sub(genesis_to_current_slot)
            .expect("the difference from now to current slot fits in Duration");

        next_tick = Tick::start_of_slot(current_slot);
        now_to_next_tick = Duration::ZERO;

        // Measure each candidate tick from the start of the current slot using its absolute
        // offset from genesis. Accumulating a per-phase tick duration instead would both
        // drift and need re-deriving at every fork boundary.
        let offset_of = |tick| {
            duration_from_genesis_at_tick::<P>(config, tick).saturating_sub(genesis_to_current_slot)
        };

        while now_to_next_tick < current_slot_to_now {
            next_tick = next_tick.next::<P>(config)?;
            now_to_next_tick = offset_of(next_tick);
        }

        if only_interval_ticks {
            while !next_tick.is_start_of_interval() {
                next_tick = next_tick.next::<P>(config)?;
                now_to_next_tick = offset_of(next_tick);
            }
        }

        now_to_next_tick = now_to_next_tick.saturating_sub(current_slot_to_now);
    }

    let next_instant = now_instant
        .checked_add(now_to_next_tick)
        .ok_or(ClockError::NextInstantOverflow)?;

    Ok((next_tick, next_instant))
}

/// Offset from the start of `slot` to the tick at `index` within it.
///
/// The division is applied last, so the ticks of a slot always add up to exactly the slot.
/// A single rounded tick duration does not: Heze's 5 s slot over 16 ticks truncates to
/// 312 ms, and 16 of those is 4992 ms. Scheduling advances one tick at a time, so that 8 ms
/// would be lost on every slot and the clock would run progressively fast.
fn tick_offset_in_slot<P: Preset>(config: &Config, slot: Slot, index: usize) -> Duration {
    let slot_duration = config.slot_duration_at_slot::<P>(slot).as_nanos();

    let ticks_per_slot = u128::try_from(TickKind::ticks_per_slot::<P>(config, slot))
        .expect("number of ticks per slot should fit in u128");

    let index = u128::try_from(index).expect("tick index should fit in u128");

    let nanos = slot_duration
        .saturating_mul(index)
        .checked_div(ticks_per_slot)
        .expect("there should be at least one tick per slot");

    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// Offset from genesis to `tick`.
///
/// Absolute, so rounding within any single slot or tick cannot accumulate.
fn duration_from_genesis_at_tick<P: Preset>(config: &Config, tick: Tick) -> Duration {
    config
        .duration_from_genesis_at_slot::<P>(tick.slot)
        .saturating_add(tick_offset_in_slot::<P>(config, tick.slot, tick.kind as usize))
}

#[cfg(test)]
mod tests {
    use core::ops::Add as _;

    use futures::StreamExt as _;
    use futures::future::FutureExt as _;
    use itertools::Itertools as _;
    use test_case::test_case;
    use types::{
        gloas::consts::INTERVALS_PER_SLOT_GLOAS,
        phase0::consts::INTERVALS_PER_SLOT,
        preset::{Mainnet, Minimal},
    };

    use crate::fake_time::{FakeInstant, FakeSystemTime, Timespec};

    use super::*;

    #[test]
    fn tick_count_is_a_multiple_of_interval_count() {
        assert!(
            TickKind::pre_gloas_ticks()
                .count()
                .is_multiple_of(INTERVALS_PER_SLOT.into())
        );
        assert!(TickKind::CARDINALITY.is_multiple_of(INTERVALS_PER_SLOT_GLOAS.into()));
    }

    #[test]
    fn tick_count_is_a_multiple_of_interval_count_pre_gloas() {
        let config = Config::mainnet();
        let slot = GENESIS_SLOT;
        assert!(
            TickKind::ticks_per_slot::<Mainnet>(&config, slot)
                .is_multiple_of(INTERVALS_PER_SLOT.into())
        );
    }

    #[test]
    fn tick_count_is_a_multiple_of_interval_count_post_gloas() {
        let config = Config::mainnet().start_and_stay_in(Phase::Gloas);
        let slot = GENESIS_SLOT;
        assert!(
            TickKind::ticks_per_slot::<Mainnet>(&config, slot)
                .is_multiple_of(INTERVALS_PER_SLOT_GLOAS.into())
        );
    }

    #[test]
    fn tick_count_change_at_fork_boundary() {
        // activate Gloas at epoch 2
        let gloas_fork_epoch = 2;
        let config = Config::mainnet().upgrade_once(Phase::Gloas, gloas_fork_epoch);

        // slot before Gloas fork
        let pre_gloas_slot = 32;
        assert_eq!(
            TickKind::ticks_per_slot::<Mainnet>(&config, pre_gloas_slot),
            12
        );

        // first slot at Gloas fork
        let first_gloas_slot = 64;
        assert_eq!(
            TickKind::ticks_per_slot::<Mainnet>(&config, first_gloas_slot),
            16
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ticks_with_mainnet_config_produces_a_tick_every_second_pre_gloas() -> Result<()> {
        let genesis_time_in_ms = u64::try_from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_millis()
                .add(1000),
        )
        .expect("genesis_time_in_ms should fit in u64");

        let config = Config::mainnet();
        let mut ticks = ticks::<Mainnet>(&config, genesis_time_in_ms)?;
        let mut next_tick = || ticks.next().now_or_never().flatten().transpose();

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::Propose)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::ProposeFourth)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::Attest)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::AttestFourth)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::Aggregate)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::AggregateFourth)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(1, TickKind::Propose)));
        assert_eq!(next_tick()?, None);

        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn ticks_adjusts_duration_at_gloas_fork() -> Result<()> {
        // This setup activates Gloas fork at epoch 1 with mainnet preset
        let config = Config::mainnet().upgrade_once(Phase::Gloas, 1);
        let one_second_duration = Duration::from_secs(1);
        let post_gloas_tick_duration = Duration::from_millis(750);

        // Genesis is 1 second from now
        let genesis_time_in_ms = u64::try_from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_millis()
                .add(1000),
        )
        .expect("genesis_time_in_ms should fit in u64");

        let mut ticks = ticks::<Mainnet>(&config, genesis_time_in_ms)?;

        assert_eq!(next_tick(&mut ticks)?, None);

        // Advance to genesis slot
        tokio::time::advance(one_second_duration).await;
        assert_eq!(
            next_tick(&mut ticks)?,
            Some(Tick::new(0, TickKind::Propose))
        );

        // Fast-forward through slots 0-31 (pre-Gloas)
        // Each slot has 12 ticks at 1000 ms each = 12 000 ms. We skip 31 slots
        // and 11 ticks in bulk and draining the stream.
        tokio::time::advance(Duration::from_secs(32 * 12 - 2)).await;
        drain(&mut ticks).await?;

        // Last tick in slot 31 (pre-Gloas)
        tokio::time::advance(one_second_duration).await;
        assert_eq!(
            next_tick(&mut ticks)?,
            Some(Tick::new(31, TickKind::AggregateFourth))
        );
        assert_eq!(next_tick(&mut ticks)?, None);

        // Fork transition to Gloas at slot 32
        // tokio::time::advance(one_second_duration).await;
        tokio::time::advance(post_gloas_tick_duration).await;
        assert_eq!(
            next_tick(&mut ticks)?,
            None,
            "Propose tick at slot 32 should not appear yet"
        );

        tokio::time::advance(Duration::from_millis(250)).await;
        assert_eq!(
            next_tick(&mut ticks)?,
            Some(Tick::new(32, TickKind::Propose))
        );
        assert_eq!(next_tick(&mut ticks)?, None);

        // Advance to next tick by 750ms (post-Gloas)
        tokio::time::advance(post_gloas_tick_duration).await;
        assert_eq!(next_tick(&mut ticks)?, None); // ProposeSecond

        tokio::time::advance(post_gloas_tick_duration).await;
        assert_eq!(next_tick(&mut ticks)?, None); // ProposeThird

        // End of propose interval (post-Gloas)
        tokio::time::advance(post_gloas_tick_duration).await;
        assert_eq!(
            next_tick(&mut ticks)?,
            Some(Tick::new(32, TickKind::ProposeFourth))
        );
        assert_eq!(next_tick(&mut ticks)?, None);

        // Then, Start of attest interval
        tokio::time::advance(post_gloas_tick_duration).await;
        assert_eq!(
            next_tick(&mut ticks)?,
            Some(Tick::new(32, TickKind::Attest))
        );
        assert_eq!(next_tick(&mut ticks)?, None);

        // ── Regression guard: advancing only 250 ms more must NOT emit anything ───
        // Under the old bug the interval would have fired at the 1000 ms mark (250 ms after Attest), producing a spurious tick.
        tokio::time::advance(Duration::from_millis(250)).await;
        assert_eq!(
            next_tick(&mut ticks)?,
            None,
            "spurious tick at old 1000 ms boundary must not appear post-fork"
        );

        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn ticks_with_mainnet_config_iterate_through_every_tick_post_gloas() -> Result<()> {
        let genesis_time_in_ms = u64::try_from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_millis()
                .add(1000),
        )
        .expect("genesis_time_in_ms should fit in u64");

        let config = Config::mainnet().start_and_stay_in(Phase::Gloas);
        // `tick_offset_in_slot::<Mainnet>(&config, GENESIS_SLOT, 1)`: 12 s over 16 ticks.
        let tick_duration = Duration::from_millis(750);
        let mut ticks = ticks::<Mainnet>(&config, genesis_time_in_ms)?;
        let mut next_tick = || ticks.next().now_or_never().flatten().transpose();

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::Propose)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::ProposeFourth)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::Attest)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::AttestFourth)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::Aggregate)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::AggregateFourth)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::PayloadAttest)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(
            next_tick()?,
            Some(Tick::new(0, TickKind::PayloadAttestFourth))
        );
        assert_eq!(next_tick()?, None);

        tokio::time::advance(tick_duration).await;

        assert_eq!(next_tick()?, Some(Tick::new(1, TickKind::Propose)));
        assert_eq!(next_tick()?, None);

        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn ticks_starts_with_tick_at_end_of_interval_when_just_past_genesis() -> Result<()> {
        let genesis_time_in_ms = u64::try_from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_millis(),
        )
        .expect("genesis_time_in_ms should fit in u64");

        let config = Config::mainnet();
        let mut ticks = ticks::<Mainnet>(&config, genesis_time_in_ms)?;
        let mut next_tick = || ticks.next().now_or_never().flatten().transpose();

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::ProposeFourth)));
        assert_eq!(next_tick()?, None);

        tokio::time::advance(Duration::from_secs(1)).await;

        assert_eq!(next_tick()?, Some(Tick::new(0, TickKind::Attest)));
        assert_eq!(next_tick()?, None);

        Ok(())
    }

    #[tokio::test]
    async fn ticks_does_not_panic() {
        let configs = [
            config_with_slot_duration(Duration::from_secs(1)),
            config_with_slot_duration(Duration::from_secs(2)),
            config_with_slot_duration(Duration::from_secs(3)),
            Config::minimal(),
            Config::mainnet(),
            config_with_slot_duration(Duration::from_secs(18)),
        ];

        let genesis_times = [
            UnixSeconds::MIN,
            777_000,
            UnixSeconds::MAX - 3,
            UnixSeconds::MAX - 2,
            UnixSeconds::MAX - 1,
            UnixSeconds::MAX,
        ];

        for (config, genesis_time_in_ms) in configs.iter().cartesian_product(genesis_times) {
            ticks::<Mainnet>(config, genesis_time_in_ms).ok();
        }
    }

    #[tokio::test]
    async fn ticks_does_not_panic_post_gloas() {
        let configs = [
            config_with_slot_duration(Duration::from_secs(1)).start_and_stay_in(Phase::Gloas),
            config_with_slot_duration(Duration::from_secs(2)).start_and_stay_in(Phase::Gloas),
            config_with_slot_duration(Duration::from_secs(3)).start_and_stay_in(Phase::Gloas),
            Config::minimal().start_and_stay_in(Phase::Gloas),
            Config::mainnet().start_and_stay_in(Phase::Gloas),
            config_with_slot_duration(Duration::from_secs(18)).start_and_stay_in(Phase::Gloas),
        ];

        let genesis_times = [
            UnixSeconds::MIN,
            777_000,
            UnixSeconds::MAX - 3,
            UnixSeconds::MAX - 2,
            UnixSeconds::MAX - 1,
            UnixSeconds::MAX,
        ];

        for (config, genesis_time_in_ms) in configs.iter().cartesian_product(genesis_times) {
            ticks::<Mainnet>(config, genesis_time_in_ms).ok();
        }
    }

    #[test_case(-24 => Tick::new(0, TickKind::Propose);             "24 seconds before genesis")]
    #[test_case(-12 => Tick::new(0, TickKind::Propose);             "12 seconds before genesis")]
    #[test_case( -1 => Tick::new(0, TickKind::Propose);             "1 second before genesis")]
    #[test_case(  0 => Tick::new(0, TickKind::Propose);             "at genesis")]
    #[test_case(  1 => Tick::new(0, TickKind::ProposeSecond);       "1 second after genesis")]
    #[test_case(  2 => Tick::new(0, TickKind::ProposeThird);        "2 second after genesis")]
    #[test_case(  3 => Tick::new(0, TickKind::ProposeFourth);       "3 seconds after genesis")]
    #[test_case(  4 => Tick::new(0, TickKind::Attest);              "4 seconds after genesis")]
    #[test_case(  5 => Tick::new(0, TickKind::AttestSecond);        "5 seconds after genesis")]
    #[test_case(  6 => Tick::new(0, TickKind::AttestThird);         "6 seconds after genesis")]
    #[test_case(  7 => Tick::new(0, TickKind::AttestFourth);        "7 seconds after genesis")]
    #[test_case(  8 => Tick::new(0, TickKind::Aggregate);           "8 seconds after genesis")]
    #[test_case(  9 => Tick::new(0, TickKind::AggregateSecond);     "9 seconds after genesis")]
    #[test_case( 10 => Tick::new(0, TickKind::AggregateThird);      "10 seconds after genesis")]
    #[test_case( 11 => Tick::new(0, TickKind::AggregateFourth);     "11 seconds after genesis")]
    #[test_case( 12 => Tick::new(1, TickKind::Propose);             "12 seconds after genesis")]
    #[test_case( 13 => Tick::new(1, TickKind::ProposeSecond);       "13 seconds after genesis")]
    #[test_case( 24 => Tick::new(2, TickKind::Propose);             "24 seconds after genesis")]
    #[test_case(384 => Tick::new(32, TickKind::Propose);            "at Gloas fork")]
    #[test_case(385 => Tick::new(32, TickKind::ProposeSecond);      "1 seconds after Gloas fork")]
    #[test_case(386 => Tick::new(32, TickKind::ProposeThird);       "2 seconds after Gloas fork")]
    #[test_case(387 => Tick::new(32, TickKind::Attest);             "3 seconds after Gloas fork")]
    #[test_case(388 => Tick::new(32, TickKind::AttestSecond);       "4 seconds after Gloas fork")]
    #[test_case(389 => Tick::new(32, TickKind::AttestThird);        "5 seconds after Gloas fork")]
    #[test_case(390 => Tick::new(32, TickKind::Aggregate);          "6 seconds after Gloas fork")]
    #[test_case(391 => Tick::new(32, TickKind::AggregateSecond);    "7 seconds after Gloas fork")]
    #[test_case(392 => Tick::new(32, TickKind::AggregateThird);     "8 seconds after Gloas fork")]
    #[test_case(393 => Tick::new(32, TickKind::PayloadAttest);      "9 seconds after Glaos fork")]
    #[test_case(394 => Tick::new(32, TickKind::PayloadAttestSecond); "10 seconds after Glaos fork")]
    #[test_case(395 => Tick::new(32, TickKind::PayloadAttestThird); "11 seconds after Glaos fork")]
    #[test_case(396 => Tick::new(33, TickKind::Propose);            "12 seconds after Glaos fork")]
    fn tick_at_time_relative_to_genesis_with_mainnet_config(offset: i64) -> Tick {
        tick_at_time_relative_to_genesis::<Mainnet>(
            &Config::mainnet().upgrade_once(Phase::Gloas, 1),
            offset,
        )
    }

    #[test_case(    0 => Tick::new(0, TickKind::Propose);               "at genesis")]
    #[test_case(  750 => Tick::new(0, TickKind::ProposeSecond);         "0.75 seconds after genesis")]
    #[test_case( 1500 => Tick::new(0, TickKind::ProposeThird);          "1.50 seconds after genesis")]
    #[test_case( 2250 => Tick::new(0, TickKind::ProposeFourth);         "2.25 seconds after genesis")]
    #[test_case( 3000 => Tick::new(0, TickKind::Attest);                "3.00 seconds after genesis")]
    #[test_case( 3750 => Tick::new(0, TickKind::AttestSecond);          "3.75 seconds after genesis")]
    #[test_case( 4500 => Tick::new(0, TickKind::AttestThird);           "4.50 seconds after genesis")]
    #[test_case( 5250 => Tick::new(0, TickKind::AttestFourth);          "5.25 seconds after genesis")]
    #[test_case( 6000 => Tick::new(0, TickKind::Aggregate);             "6.00 seconds after genesis")]
    #[test_case( 6750 => Tick::new(0, TickKind::AggregateSecond);       "6.75 seconds after genesis")]
    #[test_case( 7500 => Tick::new(0, TickKind::AggregateThird);        "7.50 seconds after genesis")]
    #[test_case( 8250 => Tick::new(0, TickKind::AggregateFourth);       "8.25 seconds after genesis")]
    #[test_case( 9000 => Tick::new(0, TickKind::PayloadAttest);         "9.00 seconds after genesis")]
    #[test_case( 9750 => Tick::new(0, TickKind::PayloadAttestSecond);   "9.75 seconds after genesis")]
    #[test_case(10500 => Tick::new(0, TickKind::PayloadAttestThird);    "10.50 seconds after genesis")]
    #[test_case(11250 => Tick::new(0, TickKind::PayloadAttestFourth);   "11.25 seconds after genesis")]
    #[test_case(12000 => Tick::new(1, TickKind::Propose);               "12.00 seconds after genesis")]
    #[test_case(24000 => Tick::new(2, TickKind::Propose);               "24.00 seconds after genesis")]
    fn tick_at_time_millis_relative_to_genesis_with_mainnet_config_post_gloas(
        offset: i128,
    ) -> Tick {
        tick_at_time_millis_relative_to_genesis::<Mainnet>(
            &Config::mainnet().start_and_stay_in(Phase::Gloas),
            offset,
        )
    }

    #[test_case(-12 => Tick::new(0, TickKind::Propose);             "12 seconds before genesis")]
    #[test_case( -6 => Tick::new(0, TickKind::Propose);             "6 seconds before genesis")]
    #[test_case( -1 => Tick::new(0, TickKind::Propose);             "1 second before genesis")]
    #[test_case(  0 => Tick::new(0, TickKind::Propose);             "at genesis")]
    #[test_case(  1 => Tick::new(0, TickKind::ProposeThird);        "1 second after genesis")]
    #[test_case(  2 => Tick::new(0, TickKind::Attest);              "2 seconds after genesis")]
    #[test_case(  3 => Tick::new(0, TickKind::AttestThird);         "3 seconds after genesis")]
    #[test_case(  4 => Tick::new(0, TickKind::Aggregate);           "4 seconds after genesis")]
    #[test_case(  5 => Tick::new(0, TickKind::AggregateThird);      "5 seconds after genesis")]
    #[test_case(  6 => Tick::new(1, TickKind::Propose);             "6 seconds after genesis")]
    #[test_case(  7 => Tick::new(1, TickKind::ProposeThird);        "7 seconds after genesis")]
    #[test_case( 12 => Tick::new(2, TickKind::Propose);             "12 seconds after genesis")]
    #[test_case( 48 => Tick::new(8, TickKind::Propose);             "at Gloas fork")]
    #[test_case( 49 => Tick::new(8, TickKind::ProposeThird);        "1 seconds after Gloas fork")]
    #[test_case( 50 => Tick::new(8, TickKind::AttestSecond);        "2 seconds after Gloas fork")]
    #[test_case( 51 => Tick::new(8, TickKind::Aggregate);           "3 seconds after Gloas fork")]
    #[test_case( 52 => Tick::new(8, TickKind::AggregateThird);      "4 seconds after Gloas fork")]
    #[test_case( 53 => Tick::new(8, TickKind::PayloadAttestSecond); "5 seconds after Gloas fork")]
    #[test_case( 54 => Tick::new(9, TickKind::Propose);             "6 seconds after Gloas fork")]
    #[test_case( 55 => Tick::new(9, TickKind::ProposeThird);        "7 seconds after Gloas fork")]
    #[test_case( 60 => Tick::new(10, TickKind::Propose);            "12 seconds after Gloas fork")]
    fn tick_at_time_relative_to_genesis_with_minimal_config(offset: i64) -> Tick {
        tick_at_time_relative_to_genesis::<Minimal>(
            &Config::minimal().upgrade_once(Phase::Gloas, 1),
            offset,
        )
    }

    #[test_case(    0 => Tick::new(0, TickKind::Propose);               "at genesis")]
    #[test_case(  375 => Tick::new(0, TickKind::ProposeSecond);         "0.375 seconds after genesis")]
    #[test_case(  750 => Tick::new(0, TickKind::ProposeThird);          "0.750 seconds after genesis")]
    #[test_case( 1125 => Tick::new(0, TickKind::ProposeFourth);         "1.125 seconds after genesis")]
    #[test_case( 1500 => Tick::new(0, TickKind::Attest);                "1.500 seconds after genesis")]
    #[test_case( 1875 => Tick::new(0, TickKind::AttestSecond);          "1.875 seconds after genesis")]
    #[test_case( 2250 => Tick::new(0, TickKind::AttestThird);           "2.250 seconds after genesis")]
    #[test_case( 2625 => Tick::new(0, TickKind::AttestFourth);          "2.625 seconds after genesis")]
    #[test_case( 3000 => Tick::new(0, TickKind::Aggregate);             "3.000 seconds after genesis")]
    #[test_case( 3375 => Tick::new(0, TickKind::AggregateSecond);       "3.375 seconds after genesis")]
    #[test_case( 3750 => Tick::new(0, TickKind::AggregateThird);        "3.750 seconds after genesis")]
    #[test_case( 4125 => Tick::new(0, TickKind::AggregateFourth);       "4.125 seconds after genesis")]
    #[test_case( 4500 => Tick::new(0, TickKind::PayloadAttest);         "4.500 seconds after genesis")]
    #[test_case( 4875 => Tick::new(0, TickKind::PayloadAttestSecond);   "4.875 seconds after genesis")]
    #[test_case( 5250 => Tick::new(0, TickKind::PayloadAttestThird);    "5.250 seconds after genesis")]
    #[test_case( 5625 => Tick::new(0, TickKind::PayloadAttestFourth);   "5.625 seconds after genesis")]
    #[test_case( 6000 => Tick::new(1, TickKind::Propose);               "6.000 seconds after genesis")]
    #[test_case(12000 => Tick::new(2, TickKind::Propose);               "12.000 seconds after genesis")]
    fn tick_at_time_millis_relative_to_genesis_with_minimal_config_post_gloas(
        offset: i128,
    ) -> Tick {
        tick_at_time_millis_relative_to_genesis::<Minimal>(
            &Config::minimal().start_and_stay_in(Phase::Gloas),
            offset,
        )
    }

    #[test_case(100 => (777, Tick::new(0, TickKind::Propose));         "long before genesis")]
    #[test_case(777 => (777, Tick::new(0, TickKind::Propose));         "at genesis")]
    #[test_case(778 => (778, Tick::new(0, TickKind::ProposeSecond));   "1 second after genesis")]
    #[test_case(779 => (779, Tick::new(0, TickKind::ProposeThird));    "2 seconds after genesis")]
    #[test_case(780 => (780, Tick::new(0, TickKind::ProposeFourth));   "3 seconds after genesis")]
    #[test_case(781 => (781, Tick::new(0, TickKind::Attest));          "4 seconds after genesis")]
    #[test_case(782 => (782, Tick::new(0, TickKind::AttestSecond));    "5 seconds after genesis")]
    #[test_case(783 => (783, Tick::new(0, TickKind::AttestThird));     "6 seconds after genesis")]
    #[test_case(784 => (784, Tick::new(0, TickKind::AttestFourth));    "7 seconds after genesis")]
    #[test_case(785 => (785, Tick::new(0, TickKind::Aggregate));       "8 seconds after genesis")]
    #[test_case(786 => (786, Tick::new(0, TickKind::AggregateSecond)); "9 seconds after genesis")]
    #[test_case(787 => (787, Tick::new(0, TickKind::AggregateThird));  "10 seconds after genesis")]
    #[test_case(788 => (788, Tick::new(0, TickKind::AggregateFourth)); "11 seconds after genesis")]
    #[test_case(789 => (789, Tick::new(1, TickKind::Propose));         "12 seconds after genesis")]
    fn next_tick_with_instant_with_mainnet_config(time: UnixSeconds) -> (UnixSeconds, Tick) {
        next_tick_with_instant::<Mainnet>(&Config::mainnet(), time, false)
    }

    #[test_case(100 => (777, Tick::new(0, TickKind::Propose));   "long before genesis")]
    #[test_case(777 => (777, Tick::new(0, TickKind::Propose));   "at genesis")]
    #[test_case(778 => (781, Tick::new(0, TickKind::Attest));    "1 second after genesis")]
    #[test_case(779 => (781, Tick::new(0, TickKind::Attest));    "2 seconds after genesis")]
    #[test_case(780 => (781, Tick::new(0, TickKind::Attest));    "3 seconds after genesis")]
    #[test_case(781 => (781, Tick::new(0, TickKind::Attest));    "4 seconds after genesis")]
    #[test_case(782 => (785, Tick::new(0, TickKind::Aggregate)); "5 seconds after genesis")]
    #[test_case(783 => (785, Tick::new(0, TickKind::Aggregate)); "6 seconds after genesis")]
    #[test_case(784 => (785, Tick::new(0, TickKind::Aggregate)); "7 seconds after genesis")]
    #[test_case(785 => (785, Tick::new(0, TickKind::Aggregate)); "8 seconds after genesis")]
    #[test_case(786 => (789, Tick::new(1, TickKind::Propose));   "9 seconds after genesis")]
    #[test_case(787 => (789, Tick::new(1, TickKind::Propose));   "10 seconds after genesis")]
    #[test_case(788 => (789, Tick::new(1, TickKind::Propose));   "11 seconds after genesis")]
    #[test_case(789 => (789, Tick::new(1, TickKind::Propose));   "12 seconds after genesis")]
    fn next_tick_with_instant_with_mainnet_config_only_interval_ticks_pre_gloas(
        time: UnixSeconds,
    ) -> (UnixSeconds, Tick) {
        next_tick_with_instant::<Mainnet>(&Config::mainnet(), time, true)
    }

    #[test_case(100 => (777, Tick::new(0, TickKind::Propose));      "long before genesis")]
    #[test_case(777 => (777, Tick::new(0, TickKind::Propose));      "at genesis")]
    #[test_case(778 => (780, Tick::new(0, TickKind::Attest));       "1 second after genesis")]
    #[test_case(779 => (780, Tick::new(0, TickKind::Attest));       "2 seconds after genesis")]
    #[test_case(780 => (780, Tick::new(0, TickKind::Attest));       "3 seconds after genesis")]
    #[test_case(781 => (783, Tick::new(0, TickKind::Aggregate));    "4 seconds after genesis")]
    #[test_case(782 => (783, Tick::new(0, TickKind::Aggregate));    "5 seconds after genesis")]
    #[test_case(783 => (783, Tick::new(0, TickKind::Aggregate));    "6 seconds after genesis")]
    #[test_case(784 => (786, Tick::new(0, TickKind::PayloadAttest)); "7 seconds after genesis")]
    #[test_case(785 => (786, Tick::new(0, TickKind::PayloadAttest)); "8 seconds after genesis")]
    #[test_case(786 => (786, Tick::new(0, TickKind::PayloadAttest)); "9 seconds after genesis")]
    #[test_case(787 => (789, Tick::new(1, TickKind::Propose));   "10 seconds after genesis")]
    #[test_case(788 => (789, Tick::new(1, TickKind::Propose));   "11 seconds after genesis")]
    #[test_case(789 => (789, Tick::new(1, TickKind::Propose));   "12 seconds after genesis")]
    fn next_tick_with_instant_with_mainnet_config_only_interval_ticks_post_gloas(
        time: UnixSeconds,
    ) -> (UnixSeconds, Tick) {
        next_tick_with_instant::<Mainnet>(
            &Config::mainnet().start_and_stay_in(Phase::Gloas),
            time,
            true,
        )
    }

    #[test_case(100 => (777, Tick::new(0, TickKind::Propose));        "long before genesis")]
    #[test_case(777 => (777, Tick::new(0, TickKind::Propose));        "at genesis")]
    #[test_case(778 => (778, Tick::new(0, TickKind::ProposeThird));   "1 second after genesis")]
    #[test_case(779 => (779, Tick::new(0, TickKind::Attest));         "2 seconds after genesis")]
    #[test_case(780 => (780, Tick::new(0, TickKind::AttestThird));    "3 seconds after genesis")]
    #[test_case(781 => (781, Tick::new(0, TickKind::Aggregate));      "4 seconds after genesis")]
    #[test_case(782 => (782, Tick::new(0, TickKind::AggregateThird)); "5 seconds after genesis")]
    #[test_case(783 => (783, Tick::new(1, TickKind::Propose));        "6 seconds after genesis")]
    fn next_tick_with_instant_with_minimal_config(time: UnixSeconds) -> (UnixSeconds, Tick) {
        next_tick_with_instant::<Minimal>(&Config::minimal(), time, false)
    }

    #[test_case(100 => (777, Tick::new(0, TickKind::Propose));   "long before genesis")]
    #[test_case(777 => (777, Tick::new(0, TickKind::Propose));   "at genesis")]
    #[test_case(778 => (779, Tick::new(0, TickKind::Attest));    "1 second after genesis")]
    #[test_case(779 => (779, Tick::new(0, TickKind::Attest));    "2 seconds after genesis")]
    #[test_case(780 => (781, Tick::new(0, TickKind::Aggregate)); "3 seconds after genesis")]
    #[test_case(781 => (781, Tick::new(0, TickKind::Aggregate)); "4 seconds after genesis")]
    #[test_case(782 => (783, Tick::new(1, TickKind::Propose));   "5 seconds after genesis")]
    #[test_case(783 => (783, Tick::new(1, TickKind::Propose));   "6 seconds after genesis")]
    fn next_tick_with_instant_with_minimal_config_only_interval_ticks(
        time: UnixSeconds,
    ) -> (UnixSeconds, Tick) {
        next_tick_with_instant::<Minimal>(&Config::minimal(), time, true)
    }

    #[test_case(Duration::from_secs(3) => Duration::from_millis(250))]
    #[test_case(Config::minimal().slot_duration_ms => Duration::from_millis(500))]
    #[test_case(Config::mainnet().slot_duration_ms => Duration::from_secs(1))]
    #[test_case(Duration::from_secs(18) => Duration::from_millis(1500))]
    fn tick_duration_with_custom_slot_duration_pre_gloas(slot_duration: Duration) -> Duration {
        let config = config_with_slot_duration(slot_duration);
        tick_offset_in_slot::<Mainnet>(&config, GENESIS_SLOT, 1)
    }

    #[test_case(Duration::from_secs(2) => Duration::from_millis(125))]
    #[test_case(Duration::from_secs(4) => Duration::from_millis(250))]
    #[test_case(Config::minimal().slot_duration_ms => Duration::from_millis(375))]
    #[test_case(Duration::from_secs(8) => Duration::from_millis(500))]
    #[test_case(Config::mainnet().slot_duration_ms => Duration::from_millis(750))]
    #[test_case(Duration::from_secs(16) => Duration::from_secs(1))]
    #[test_case(Duration::from_secs(18) => Duration::from_millis(1125))]
    fn tick_duration_with_seconds_per_slot_post_gloas(slot_duration: Duration) -> Duration {
        let config = config_with_slot_duration(slot_duration).start_and_stay_in(Phase::Gloas);
        tick_offset_in_slot::<Mainnet>(&config, GENESIS_SLOT, 1)
    }

    fn tick_at_time_relative_to_genesis<P: Preset>(config: &Config, offset: i64) -> Tick {
        let genesis_time = config.min_genesis_time;

        let time = genesis_time
            .checked_add_signed(offset)
            .expect("offset should be small enough to make the resulting time fit in UnixSeconds");

        Tick::at_time::<P>(config, time, genesis_time)
            .expect("config should have a valid value of SECONDS_PER_SLOT")
    }

    fn tick_at_time_millis_relative_to_genesis<P: Preset>(
        config: &Config,
        offset_in_millis: i128,
    ) -> Tick {
        let genesis_time = config.min_genesis_time;

        let time = Duration::from_secs(genesis_time)
            .as_millis()
            .checked_add_signed(offset_in_millis)
            .expect("offset should be small enough to make the resulting time fit in UnixSeconds")
            .try_into()
            .expect("time should be small enough to fit into u64");

        Tick::from_duration::<P>(config, Duration::from_millis(time), genesis_time)
            .expect("config should have a valid value of SECONDS_PER_SLOT")
    }

    fn next_tick_with_instant<P: Preset>(
        config: &Config,
        time: UnixSeconds,
        only_interval_ticks: bool,
    ) -> (UnixSeconds, Tick) {
        let genesis_time_in_ms = 777_000;
        let timespec = Timespec::from_secs(time);

        let (actual_tick, actual_instant) = super::next_tick_with_instant::<P, _, _>(
            config,
            FakeInstant(timespec),
            FakeSystemTime(timespec),
            genesis_time_in_ms,
            only_interval_ticks,
        )
        .expect("FakeSystemTime cannot represent times before the Unix epoch");

        assert_eq!(actual_instant.0.subsec_nanos(), 0);

        (actual_instant.0.as_secs(), actual_tick)
    }

    fn config_with_slot_duration(slot_duration: Duration) -> Config {
        Config {
            slot_duration_ms: slot_duration,
            // Keep the duration uniform across the Heze boundary, so the helper means the
            // same thing in every phase.
            slot_duration_ms_heze: slot_duration,
            ..Config::default()
        }
    }

    /// The `heze-devnet` shape: 6 s slots before the fork and 5 s from it, with the minimal
    /// preset's 8 slots per epoch putting the Heze boundary at slot 8.
    ///
    /// 5 s over 16 ticks does not divide evenly, which is what makes this interesting.
    fn heze_devnet_config() -> Config {
        Config {
            slot_duration_ms_heze: Duration::from_secs(5),
            ..Config::minimal()
        }
        .upgrade_once(Phase::Heze, 1)
    }

    /// The ticks of a slot must add up to *exactly* the slot.
    ///
    /// Scheduling advances one tick at a time, so a remainder here is lost on every slot
    /// and the clock runs progressively fast. Heze's 5 s slot over 16 ticks is the case
    /// that caught this: `5000 / 16` truncates to 312 ms and 16 of those is 4992 ms.
    #[test_case(0)]
    #[test_case(7)]
    #[test_case(8)]
    #[test_case(9)]
    #[test_case(63)]
    fn ticks_add_up_to_the_whole_slot(slot: Slot) {
        let config = heze_devnet_config();
        let ticks_per_slot = TickKind::ticks_per_slot::<Minimal>(&config, slot);

        assert_eq!(
            tick_offset_in_slot::<Minimal>(&config, slot, ticks_per_slot),
            config.slot_duration_at_slot::<Minimal>(slot),
            "the ticks of slot {slot} do not fill it",
        );
    }

    /// Walking every tick of a slot must land exactly on the next slot, however many slots
    /// have passed since the duration changed.
    #[test]
    fn tick_offsets_from_genesis_do_not_drift() {
        let config = heze_devnet_config();

        for slot in 0..64 {
            let ticks_per_slot = TickKind::ticks_per_slot::<Minimal>(&config, slot);

            let walked = config
                .duration_from_genesis_at_slot::<Minimal>(slot)
                .saturating_add(tick_offset_in_slot::<Minimal>(&config, slot, ticks_per_slot));

            assert_eq!(
                walked,
                config.duration_from_genesis_at_slot::<Minimal>(slot + 1),
                "slot {slot} drifted",
            );
        }
    }

    /// Every tick resolves back to itself from its own offset, on both sides of the fork.
    #[test]
    fn ticks_round_trip_through_their_offset_from_genesis() {
        let config = heze_devnet_config();
        let genesis_time = config.min_genesis_time;
        let unix_epoch_to_genesis = Duration::from_secs(genesis_time);

        for slot in 0..24 {
            let mut tick = Tick::start_of_slot(slot);

            for _ in 0..TickKind::ticks_per_slot::<Minimal>(&config, slot) {
                let offset = duration_from_genesis_at_tick::<Minimal>(&config, tick);

                let resolved = Tick::from_duration::<Minimal>(
                    &config,
                    unix_epoch_to_genesis.saturating_add(offset),
                    genesis_time,
                )
                .expect("tick offsets are within range");

                assert_eq!(resolved, tick, "tick at {offset:?} did not round-trip");

                tick = tick.next::<Minimal>(&config).expect("slots do not run out");
            }
        }
    }

    fn next_tick<S>(stream: &mut S) -> Result<Option<Tick>>
    where
        S: Stream<Item = Result<Tick>> + Unpin,
    {
        stream.next().now_or_never().flatten().transpose()
    }

    async fn drain<S>(stream: &mut S) -> Result<()>
    where
        S: Stream<Item = Result<Tick>> + Unpin,
    {
        // `FutureExt::now_or_never()` drives futures without ever yielding to the Tokio scheduler,
        // this would exhaust coop task budget. By using `timeout(Duration::ZERO, ...)` allows
        // driving the stream while yielding to the scheduler, and the task budget got reset every
        // await points.
        while tokio::time::timeout(Duration::ZERO, stream.next())
            .await
            .ok()
            .flatten()
            .transpose()?
            .is_some()
        {}

        Ok(())
    }
}
