//! Validator-side hash chains for EIP-8321 RANDAO.
//!
//! A validator draws a random 32-byte secret, derives a chain of links from it, and registers
//! the tip as its commitment. Each block it proposes reveals the preimage of the commitment
//! currently stored in the state, walking the chain backwards one link per proposal.
//!
//! # The cursor lives in the state, not here
//!
//! `process_randao` *consumes* each reveal: it stores the revealed preimage as the
//! validator's new commitment. The validator's position in its chain is therefore a property
//! of the canonical state, not of this process. A local counter would desync on any restart
//! or reorg, so this store is a pure lookup table — "given the commitment the state holds,
//! what is its preimage?" — and the caller supplies the commitment it read from the state it
//! is proposing against. Restarts and reorgs then need no special handling.
//!
//! # Losing a chain is terminal for that validator index
//!
//! A commitment can be registered once and is never reset, so a validator that loses its
//! chain cannot propose again on that index and there is no protocol-level recovery. Chains
//! are currently held in memory only, which means a restart after registration permanently
//! retires that index — the remedy is to onboard a new validator under a new public key. The
//! chain secret is deliberately *not* derived from the signing key: a hash chain eventually
//! reveals its seed, whereas a signing key must never be revealed.

use core::num::NonZeroU64;
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Mutex,
};

use anyhow::{Result, ensure};
use rand::RngCore as _;
use types::{
    eip8321::consts::HASH_CHAIN_RANDAO_DST,
    phase0::primitives::{H256, ValidatorIndex},
};

/// Number of links generated per validator.
///
/// Reveals are consumed one per proposed block, so the chain only has to outlast the
/// validator. The protocol never learns the length, so this is purely a local choice; the
/// spec recommends at least `2**16` but any length is conformant.
pub const HASH_CHAIN_LENGTH: u64 = 10_000;

/// One step along a hash chain.
///
/// Uses the same hash as the consensus-side `verify_hash_chain_reveal`, which checks
/// `blake3(HASH_CHAIN_RANDAO_DST + reveal) == commitment`.
#[must_use]
pub fn hash_chain_link(preimage: H256) -> H256 {
    hashing::blake3_parts(&[HASH_CHAIN_RANDAO_DST, preimage.as_bytes()])
}

/// A validator's hash chain, from its secret through to the commitment it registers.
pub struct HashChain {
    /// `links[0]` is the chain secret and `links[length]` is the registered commitment.
    links: Vec<H256>,
    /// Commitment -> its index in `links`, so a reveal is a single lookup.
    index_by_link: HashMap<H256, usize>,
}

impl HashChain {
    /// Derive a chain from `secret`.
    ///
    /// Fails if any link collides with the unset-commitment sentinel, which a reveal could
    /// never be distinguished from; the spec's remedy is to regenerate from a fresh secret.
    pub fn from_secret(secret: H256, length: NonZeroU64) -> Result<Self> {
        let length = usize::try_from(length.get())?;

        let mut links = Vec::with_capacity(length.saturating_add(1));
        let mut link = secret;

        ensure!(
            !link.is_zero(),
            "hash chain secret collides with the unset-commitment sentinel",
        );

        links.push(link);

        for _ in 0..length {
            link = hash_chain_link(link);

            ensure!(
                !link.is_zero(),
                "hash chain link collides with the unset-commitment sentinel; \
                 regenerate from a fresh secret",
            );

            links.push(link);
        }

        let index_by_link = links
            .iter()
            .enumerate()
            .map(|(index, link)| (*link, index))
            .collect();

        Ok(Self {
            links,
            index_by_link,
        })
    }

    /// Derive a chain from a freshly drawn random secret.
    pub fn generate(length: NonZeroU64) -> Result<Self> {
        // Not derived from the signing key: a hash chain eventually reveals its seed.
        let mut secret = [0; 32];
        rand::thread_rng().fill_bytes(&mut secret);

        Self::from_secret(H256(secret), length)
    }

    /// The tip, which is what gets registered on chain.
    #[must_use]
    pub fn commitment(&self) -> H256 {
        *self
            .links
            .last()
            .expect("a chain always holds at least its secret")
    }

    /// The preimage of `commitment`, for a block proposed against a state holding it.
    ///
    /// `None` when the commitment is not from this chain, or when the chain is exhausted —
    /// the secret itself has no preimage the validator holds.
    #[must_use]
    pub fn reveal_for(&self, commitment: H256) -> Option<H256> {
        let index = *self.index_by_link.get(&commitment)?;

        self.links.get(index.checked_sub(1)?).copied()
    }

    /// Number of reveals left before the chain is exhausted.
    #[must_use]
    pub fn remaining_reveals(&self, commitment: H256) -> Option<usize> {
        self.index_by_link.get(&commitment).copied()
    }
}

/// Chains for this node's own validators, shared between the component that registers them
/// and the one that reveals from them.
///
/// Held in memory only: a restart after registration permanently retires that validator
/// index, as described at the module level. Persistence would mean replacing this type's
/// internals, not its callers — nothing outside depends on chains being in memory.
#[derive(Default)]
pub struct HashChainStore {
    chains: Mutex<HashMap<ValidatorIndex, HashChain>>,
}

impl HashChainStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Generate a chain for `pubkey` if it has none, returning the commitment to register.
    ///
    /// Idempotent: an existing chain is kept and its commitment returned, so repeated calls
    /// across slots never strand a chain whose registration is already in flight.
    pub fn ensure_chain(&self, validator_index: ValidatorIndex, length: NonZeroU64) -> Result<H256> {
        let mut chains = self
            .chains
            .lock()
            .map_err(|_error| anyhow::anyhow!("hash chain store is poisoned"))?;

        match chains.entry(validator_index) {
            Entry::Occupied(entry) => Ok(entry.get().commitment()),
            Entry::Vacant(entry) => Ok(entry.insert(HashChain::generate(length)?).commitment()),
        }
    }

    /// Whether a chain is already held for `validator_index`.
    #[must_use]
    pub fn contains(&self, validator_index: ValidatorIndex) -> bool {
        self.chains
            .lock()
            .is_ok_and(|chains| chains.contains_key(&validator_index))
    }

    /// The preimage of `commitment`, which the caller must have read from the state it is
    /// proposing against.
    #[must_use]
    pub fn reveal_for(&self, validator_index: ValidatorIndex, commitment: H256) -> Option<H256> {
        self.chains
            .lock()
            .ok()?
            .get(&validator_index)?
            .reveal_for(commitment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> HashChain {
        HashChain::from_secret(H256::repeat_byte(1), NonZeroU64::new(16).unwrap())
            .expect("test chain should generate")
    }

    #[test]
    fn commitment_is_the_tip_and_reveals_walk_backwards() {
        let chain = chain();
        let mut commitment = chain.commitment();

        // Every reveal must hash forward to the commitment it replaces, which is exactly what
        // `verify_hash_chain_reveal` checks on chain.
        for expected_remaining in (1..=16).rev() {
            assert_eq!(chain.remaining_reveals(commitment), Some(expected_remaining));

            let reveal = chain
                .reveal_for(commitment)
                .expect("chain should not be exhausted");

            assert_eq!(hash_chain_link(reveal), commitment);

            commitment = reveal;
        }
    }

    #[test]
    fn chain_is_exhausted_at_the_secret() {
        let chain = chain();
        let secret = *chain.links.first().expect("chain holds its secret");

        assert_eq!(chain.remaining_reveals(secret), Some(0));
        assert_eq!(chain.reveal_for(secret), None);
    }

    #[test]
    fn foreign_commitment_has_no_reveal() {
        assert_eq!(chain().reveal_for(H256::repeat_byte(0xff)), None);
    }

    #[test]
    fn store_is_idempotent_and_reveals_through_the_stored_chain() {
        let store = HashChainStore::new();
        let validator_index = 7;
        let length = NonZeroU64::new(8).unwrap();

        let commitment = store
            .ensure_chain(validator_index, length)
            .expect("generation should succeed");

        // A second call must not replace the chain, which would strand an in-flight
        // registration.
        assert_eq!(
            store
                .ensure_chain(validator_index, length)
                .expect("generation should succeed"),
            commitment,
        );
        assert!(store.contains(validator_index));

        let reveal = store
            .reveal_for(validator_index, commitment)
            .expect("the tip should have a preimage");

        assert_eq!(hash_chain_link(reveal), commitment);
        assert_eq!(store.reveal_for(8, commitment), None);
    }

    #[test]
    fn generated_chains_differ() {
        let length = NonZeroU64::new(8).unwrap();

        let first = HashChain::generate(length).expect("generation should succeed");
        let second = HashChain::generate(length).expect("generation should succeed");

        assert_ne!(first.commitment(), second.commitment());
    }
}
