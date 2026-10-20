//! EIP-7805 pure consensus helpers and inclusion-list storage.
//! Spec: consensus-specs 12c8b24, specs/heze/{beacon-chain,inclusion-list}.md.
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    accessors,
    error::SignatureKind,
    misc,
    verifier::{SingleVerifier, Verifier as _},
};
use anyhow::{Result, ensure};
use try_from_iterator::TryFromIterator as _;
use types::{
    config::Config,
    gloas::primitives::Transaction,
    heze::{
        consts::{DOMAIN_INCLUSION_LIST_COMMITTEE, INCLUSION_LIST_COMMITTEE_SIZE},
        containers::{InclusionList, SignedInclusionList},
        primitives::{InclusionListBits, InclusionListCommittee},
    },
    phase0::primitives::{H256, Slot, ValidatorIndex, Version},
    preset::Preset,
    traits::BeaconState,
};

/// Concatenate beacon committees in order, then wrap when fewer than 16 members exist.
pub fn get_inclusion_list_committee<P: Preset>(
    state: &impl BeaconState<P>,
    slot: Slot,
) -> Result<InclusionListCommittee> {
    let epoch = misc::compute_epoch_at_slot::<P>(slot);
    let relative_epoch = accessors::relative_epoch(state, epoch)?;
    let count = accessors::get_committee_count_per_slot(state, relative_epoch)?;
    let mut indices = vec![];
    for index in 0..count {
        indices.extend(accessors::beacon_committee(state, slot, index)?);
    }
    inclusion_list_committee_from_indices(&indices)
}

/// The repeated positions are intentional: small validator sets may occupy several bits.
pub fn inclusion_list_committee_from_indices(
    indices: &[ValidatorIndex],
) -> Result<InclusionListCommittee> {
    ensure!(
        !indices.is_empty(),
        "inclusion list committee has no beacon committee members"
    );
    Ok(InclusionListCommittee::try_from_iter(
        indices
            .iter()
            .copied()
            .cycle()
            .take(INCLUSION_LIST_COMMITTEE_SIZE),
    )?)
}

/// The fork version belongs to the message's slot, not necessarily the current state fork.
pub fn inclusion_list_signing_root<P: Preset>(
    config: &Config,
    message: &InclusionList<P>,
    fork_version: Version,
    genesis_validators_root: H256,
) -> H256 {
    let domain = misc::compute_domain(
        config,
        DOMAIN_INCLUSION_LIST_COMMITTEE,
        Some(fork_version),
        Some(genesis_validators_root),
    );
    misc::compute_signing_root(message, domain)
}

pub fn is_valid_inclusion_list_signature<P: Preset>(
    config: &Config,
    state: &impl BeaconState<P>,
    signed: &SignedInclusionList<P>,
) -> bool {
    let result = (|| -> Result<()> {
        let public_key = accessors::public_key(state, signed.message.validator_index)?;
        let epoch = misc::compute_epoch_at_slot::<P>(signed.message.slot);
        SingleVerifier.verify_singular(
            inclusion_list_signing_root(
                config,
                &signed.message,
                config.version_at_epoch(epoch),
                state.genesis_validators_root(),
            ),
            signed.signature,
            std::sync::Arc::new(bls::PublicKey::try_from(*public_key)?),
            SignatureKind::InclusionList,
        )
    })();
    result.is_ok()
}

#[derive(Clone, Debug)]
pub struct InclusionListEntry<P: Preset> {
    pub signed_inclusion_list: SignedInclusionList<P>,
    pub timely: bool,
}

/// Entries are scoped by both slot and shuffling dependent root. No global singleton is used.
#[derive(Clone, Debug, Default)]
pub struct InclusionListStore<P: Preset> {
    inclusion_lists: BTreeMap<(Slot, H256), BTreeMap<ValidatorIndex, InclusionListEntry<P>>>,
    equivocators: BTreeMap<(Slot, H256), BTreeSet<ValidatorIndex>>,
}

impl<P: Preset> InclusionListStore<P> {
    /// Call only after validation. Duplicate messages never upgrade their first arrival time.
    pub fn process_inclusion_list(&mut self, signed: SignedInclusionList<P>, timely: bool) {
        let message = &signed.message;
        let key = (message.slot, message.dependent_root);
        let validator = message.validator_index;
        let entries = self.inclusion_lists.entry(key).or_default();
        if let Some(stored) = entries.get(&validator) {
            if stored.signed_inclusion_list.message != *message {
                self.equivocators.entry(key).or_default().insert(validator);
            }
            return;
        }
        entries.insert(
            validator,
            InclusionListEntry {
                signed_inclusion_list: signed,
                timely,
            },
        );
    }

    fn is_included(&self, key: (Slot, H256), validator: ValidatorIndex, only_timely: bool) -> bool {
        let equivocates = self
            .equivocators
            .get(&key)
            .is_some_and(|set| set.contains(&validator));
        !equivocates
            && self
                .inclusion_lists
                .get(&key)
                .and_then(|entries| entries.get(&validator))
                .is_some_and(|entry| !only_timely || entry.timely)
    }

    #[must_use]
    pub fn get_inclusion_list_transactions(
        &self,
        slot: Slot,
        dependent_root: H256,
        only_timely: bool,
    ) -> Vec<Transaction<P>> {
        let key = (slot, dependent_root);
        let mut transactions = BTreeMap::new();
        if let Some(entries) = self.inclusion_lists.get(&key) {
            for (&validator, entry) in entries {
                if self.is_included(key, validator, only_timely) {
                    for transaction in &entry.signed_inclusion_list.message.transactions {
                        transactions
                            .entry(transaction.as_bytes().to_vec())
                            .or_insert_with(|| transaction.clone());
                    }
                }
            }
        }
        transactions.into_values().collect()
    }

    #[must_use]
    pub fn get_inclusion_list_bits(
        &self,
        committee: &InclusionListCommittee,
        slot: Slot,
        dependent_root: H256,
        only_timely: bool,
    ) -> InclusionListBits {
        let mut bits = InclusionListBits::default();
        for (position, &validator) in committee.iter().enumerate() {
            bits.set(
                position,
                self.is_included((slot, dependent_root), validator, only_timely),
            );
        }
        bits
    }

    /// A bid may advertise additional lists, but must include every locally required list.
    #[must_use]
    pub fn is_inclusion_list_bits_inclusive(
        &self,
        committee: &InclusionListCommittee,
        slot: Slot,
        dependent_root: H256,
        bits: InclusionListBits,
        only_timely: bool,
    ) -> bool {
        let local = self.get_inclusion_list_bits(committee, slot, dependent_root, only_timely);
        (0..INCLUSION_LIST_COMMITTEE_SIZE).all(|index| !local[index] || bits[index])
    }

    /// Keep the inclusive retention boundary, including equivocation records.
    pub fn prune(&mut self, current_slot: Slot, retention_slots: Slot) {
        let keep = |key: &(Slot, H256)| current_slot.saturating_sub(key.0) <= retention_slots;
        self.inclusion_lists.retain(|key, _| keep(key));
        self.equivocators.retain(|key, _| keep(key));
    }
}

/// Cheap checks shared by gossip and fork-choice handlers. No mutation on failure.
pub fn validate_inclusion_list_bounds<P: Preset>(
    message: &InclusionList<P>,
    current_slot: Slot,
    retention_slots: Slot,
    max_transaction_bytes: usize,
) -> Result<()> {
    ensure!(
        message.slot <= current_slot,
        "inclusion list is from a future slot"
    );
    ensure!(
        current_slot.saturating_sub(message.slot) <= retention_slots,
        "inclusion list is outside retention window"
    );
    let mut size = 0_usize;
    for transaction in &message.transactions {
        let length = transaction.as_bytes().len();
        ensure!(length > 0, "inclusion list contains an empty transaction");
        size = size
            .checked_add(length)
            .ok_or_else(|| anyhow::anyhow!("inclusion list size overflow"))?;
        ensure!(
            size <= max_transaction_bytes,
            "inclusion list exceeds transaction byte limit"
        );
    }
    ensure!(size > 0, "inclusion list transactions are empty");
    Ok(())
}

/// Validator duty lookup is stable only through the next epoch.
pub fn get_inclusion_list_committee_assignment<P: Preset>(
    state: &impl BeaconState<P>,
    epoch: types::phase0::primitives::Epoch,
    validator_index: ValidatorIndex,
) -> Result<Option<Slot>> {
    use typenum::Unsigned as _;
    let next_epoch = accessors::get_current_epoch(state)
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("epoch overflow"))?;
    ensure!(
        epoch <= next_epoch,
        "inclusion-list assignments beyond next epoch are unstable"
    );
    let start = epoch
        .checked_mul(P::SlotsPerEpoch::U64)
        .ok_or_else(|| anyhow::anyhow!("slot overflow"))?;
    let end = start
        .checked_add(P::SlotsPerEpoch::U64)
        .ok_or_else(|| anyhow::anyhow!("slot overflow"))?;
    for slot in start..end {
        if get_inclusion_list_committee(state, slot)?.contains(&validator_index) {
            return Ok(Some(slot));
        }
    }
    Ok(None)
}
