//! [Hash-Chain RANDAO (EIP-8321)](https://eips.ethereum.org/EIPS/eip-8321)
//!
//! A validator registers the tip of a hash chain once; from then on it reveals one preimage per
//! proposal instead of signing the epoch with its BLS key. Validators that have not registered
//! keep using the legacy BLS reveal, so the two paths coexist until the BLS path is sunset.
//!
//! The entry points are compiled as no-ops (or, for `process_randao`, as the unmodified Gloas
//! function) unless the `eip8321` feature is enabled, because the fields the EIP adds to
//! `BeaconState` and `BeaconBlockBody` change their SSZ layouts.
//!
//! There are no `spec_tests` here yet. EIP-8321 is not in a `consensus-specs` release, so
//! `scripts/download_spec_tests.sh` cannot fetch its vectors and `test_resources` globs would
//! fail to compile. Once the vectors ship, add `operations/randao`,
//! `operations/randao_commitment_registration` and `epoch_processing/pending_randao_commitments`
//! in the same shape as the Gloas spec tests. Until then they can be generated with
//! `make test reftests=true fork=eip8321` in a `consensus-specs` checkout and run against this
//! code by copying them into `consensus-spec-tests/tests/<preset>/eip8321/`.

#[cfg(feature = "eip8321")]
pub use with_eip8321::*;

#[cfg(not(feature = "eip8321"))]
pub use without_eip8321::*;

#[cfg(feature = "eip8321")]
mod with_eip8321 {
    use anyhow::{Result, ensure};
    use arithmetic::U64Ext as _;
    use bls::traits::SignatureBytes as _;
    use helper_functions::{
        accessors::{get_beacon_proposer_index, get_current_epoch, get_next_epoch, get_randao_mix},
        error::SignatureKind,
        signing::{RandaoEpoch, SignForAllForksWithGenesis as _, SignForSingleFork as _},
        verifier::{Verifier, VerifierOption},
    };
    use pubkey_cache::PubkeyCache;
    use ssz::{SszList as _, SszListMut as _};
    use types::{
        config::Config,
        gloas::{
            consts::{COMMITMENT_REGISTRATION_DELAY, MAX_RANDAO_COMMITMENT_REGISTRATIONS},
            containers::{
                BeaconBlockBody, PendingRandaoCommitment, SignedRandaoCommitmentRegistration,
            },
        },
        phase0::primitives::{H256, ValidatorIndex},
        preset::Preset,
        traits::PostGloasBeaconState,
    };

    use crate::unphased::Error;

    /// [`process_randao`](https://eips.ethereum.org/EIPS/eip-8321)
    ///
    /// A proposer with a registered commitment reveals its preimage; everyone else keeps using the
    /// legacy BLS reveal. The two paths are mutually exclusive: each requires the other path's
    /// field to be empty.
    pub fn process_randao<P: Preset>(
        config: &Config,
        pubkey_cache: &PubkeyCache,
        state: &mut impl PostGloasBeaconState<P>,
        body: &BeaconBlockBody<P>,
        mut verifier: impl Verifier,
    ) -> Result<()> {
        let epoch = get_current_epoch(state);
        let index = get_beacon_proposer_index(config, state)?;
        let commitment = *state.randao_commitments().get(index)?;
        let reveal = body.hash_chain_reveal;

        let mix = if commitment.is_zero() {
            // > Legacy BLS reveal
            ensure!(
                reveal.is_zero(),
                Error::<P>::HashChainRevealNotEmpty { index },
            );

            let public_key = state.validators().pubkey(index)?;

            if !verifier.has_option(VerifierOption::SkipRandaoVerification) {
                verifier.verify_singular(
                    RandaoEpoch::from(epoch).signing_root(config, state),
                    body.randao_reveal,
                    pubkey_cache.get_or_insert(*public_key)?,
                    SignatureKind::Randao,
                )?;
            }

            // > Mix in RANDAO reveal
            //
            // The [Eth Beacon Node API specification] does not say whether the RANDAO reveal
            // should be mixed in when `skip_randao_verification` is `true`.
            // [Lighthouse mixes it in either way].
            //
            // [Eth Beacon Node API specification]: https://ethereum.github.io/beacon-APIs/
            // [Lighthouse mixes it in either way]: https://github.com/sigp/lighthouse/blob/v4.3.0/consensus/state_processing/src/per_block_processing.rs#L282-L309
            get_randao_mix(state, epoch) ^ hashing::hash_768(body.randao_reveal)
        } else {
            // > Hash chain reveal
            ensure!(!reveal.is_zero(), Error::<P>::HashChainRevealZero { index });
            ensure!(
                body.randao_reveal.is_empty(),
                Error::<P>::RandaoRevealNotEmpty { index },
            );

            // > Check that the proposer knows the preimage.
            //
            // This costs one hash, so `SkipRandaoVerification` does not apply to it.
            let computed = hashing::blake3_chain_link(reveal);

            ensure!(
                computed == commitment,
                Error::<P>::HashChainRevealMismatch {
                    in_state: commitment,
                    in_block: reveal,
                },
            );

            let mix = hashing::blake3_256_256(get_randao_mix(state, epoch), reveal);

            // > The revealed value becomes the validator's next commitment.
            *state.randao_commitments_mut().get_mut(index)? = reveal;

            mix
        };

        *state.randao_mixes_mut().mod_index_mut(epoch) = mix;

        Ok(())
    }

    /// The `randao_commitment_registrations` loop appended to
    /// [`process_operations`](https://eips.ethereum.org/EIPS/eip-8321).
    pub fn process_randao_commitment_registrations<P: Preset>(
        config: &Config,
        pubkey_cache: &PubkeyCache,
        state: &mut impl PostGloasBeaconState<P>,
        body: &BeaconBlockBody<P>,
        mut verifier: impl Verifier,
    ) -> Result<()> {
        ensure!(
            body.randao_commitment_registrations.len() <= MAX_RANDAO_COMMITMENT_REGISTRATIONS,
            Error::<P>::TooManyOperations {
                kind: "RANDAO commitment registrations",
                maximum: MAX_RANDAO_COMMITMENT_REGISTRATIONS,
                in_block: body.randao_commitment_registrations.len(),
            },
        );
        for registration in body.randao_commitment_registrations.iter().copied() {
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

    /// [`process_randao_commitment_registration`](https://eips.ethereum.org/EIPS/eip-8321)
    pub fn process_randao_commitment_registration<P: Preset>(
        config: &Config,
        pubkey_cache: &PubkeyCache,
        state: &mut impl PostGloasBeaconState<P>,
        signed_registration: SignedRandaoCommitmentRegistration,
        mut verifier: impl Verifier,
    ) -> Result<()> {
        let registration = signed_registration.message;
        let index = registration.validator_index;

        ensure!(
            !registration.commitment.is_zero(),
            Error::<P>::RandaoCommitmentZero,
        );

        // > Register-only: valid only while the validator is unregistered.
        ensure!(
            state.randao_commitments().get(index)?.is_zero(),
            Error::<P>::RandaoCommitmentAlreadyRegistered { index },
        );

        let public_key = state.validators().pubkey(index)?;

        // > Fork-agnostic domain since registrations are valid across forks
        verifier.verify_singular(
            registration.signing_root(config, state),
            signed_registration.signature,
            pubkey_cache.get_or_insert(*public_key)?,
            SignatureKind::RandaoCommitmentRegistration,
        )?;

        queue_randao_commitment(state, index, registration.commitment)
    }

    /// [`queue_randao_commitment`](https://eips.ethereum.org/EIPS/eip-8321)
    ///
    /// At most one registration per validator may be pending. The check also rejects a second
    /// registration for the same validator within a single block.
    ///
    /// The scan is linear in the pending queue, which is itself unbounded, so a block of
    /// `MAX_RANDAO_COMMITMENT_REGISTRATIONS` registrations costs that many passes over it. A
    /// pending-index cache would remove the quadratic factor if the queue ever grows large.
    fn queue_randao_commitment<P: Preset>(
        state: &mut impl PostGloasBeaconState<P>,
        index: ValidatorIndex,
        commitment: H256,
    ) -> Result<()> {
        ensure!(
            state
                .pending_randao_commitments()
                .into_iter()
                .all(|pending| pending.validator_index != index),
            Error::<P>::RandaoCommitmentAlreadyPending { index },
        );

        let activation_epoch = get_current_epoch(state).try_add(COMMITMENT_REGISTRATION_DELAY)?;

        state
            .pending_randao_commitments_mut()
            .push(PendingRandaoCommitment {
                validator_index: index,
                commitment,
                activation_epoch,
            })?;

        Ok(())
    }

    /// [`process_pending_randao_commitments`](https://eips.ethereum.org/EIPS/eip-8321)
    ///
    /// Pending commitments are applied in queue order once their activation epoch is reached. The
    /// one-pending-registration-per-validator rule enforced in `queue_randao_commitment`
    /// guarantees entries never conflict.
    pub fn process_pending_randao_commitments<P: Preset>(
        state: &mut impl PostGloasBeaconState<P>,
    ) -> Result<()> {
        let next_epoch = get_next_epoch(state)?;
        let pending_commitments = state.pending_randao_commitments().clone();
        let mut remaining = vec![];

        for pending in &pending_commitments {
            if pending.activation_epoch <= next_epoch {
                *state
                    .randao_commitments_mut()
                    .get_mut(pending.validator_index)? = pending.commitment;
            } else {
                remaining.push(*pending);
            }
        }

        if remaining.len() < pending_commitments.len_usize() {
            state
                .pending_randao_commitments_mut()
                .try_assign_from_iter(&mut remaining.into_iter())?;
        }

        Ok(())
    }

    #[must_use]
    pub fn count_registration_signatures<P: Preset>(body: &BeaconBlockBody<P>) -> usize {
        body.randao_commitment_registrations.len()
    }

    pub fn verify_registration_signatures<P: Preset>(
        config: &Config,
        pubkey_cache: &PubkeyCache,
        state: &(impl PostGloasBeaconState<P> + ?Sized),
        body: &BeaconBlockBody<P>,
        mut verifier: impl Verifier,
    ) -> Result<()> {
        for registration in &body.randao_commitment_registrations {
            let index = registration.message.validator_index;
            let pubkey = *state.validators().pubkey(index)?;
            verifier.verify_singular(
                registration.message.signing_root(config, state),
                registration.signature,
                pubkey_cache.get_or_insert(pubkey)?,
                SignatureKind::RandaoCommitmentRegistration,
            )?;
        }
        Ok(())
    }

    /// Whether the validator reveals from a hash chain rather than signing the epoch with its BLS
    /// key, which is what decides which branch of `process_randao` applies to it.
    pub fn has_randao_commitment<P: Preset>(
        state: &(impl PostGloasBeaconState<P> + ?Sized),
        index: ValidatorIndex,
    ) -> Result<bool> {
        state
            .randao_commitments()
            .get(index)
            .map(|commitment| !commitment.is_zero())
            .map_err(Into::into)
    }
}

#[cfg(not(feature = "eip8321"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "these mirror the signatures of the eip8321 implementations"
)]
mod without_eip8321 {
    use anyhow::Result;
    use helper_functions::verifier::Verifier;
    use pubkey_cache::PubkeyCache;
    use types::{
        config::Config, gloas::containers::BeaconBlockBody, phase0::primitives::ValidatorIndex,
        preset::Preset, traits::PostGloasBeaconState,
    };

    use crate::unphased;

    pub fn process_randao<P: Preset>(
        config: &Config,
        pubkey_cache: &PubkeyCache,
        state: &mut impl PostGloasBeaconState<P>,
        body: &BeaconBlockBody<P>,
        verifier: impl Verifier,
    ) -> Result<()> {
        unphased::process_randao(config, pubkey_cache, state, body, verifier)
    }

    pub fn process_randao_commitment_registrations<P: Preset>(
        _config: &Config,
        _pubkey_cache: &PubkeyCache,
        _state: &mut impl PostGloasBeaconState<P>,
        _body: &BeaconBlockBody<P>,
        _verifier: impl Verifier,
    ) -> Result<()> {
        Ok(())
    }

    pub const fn process_pending_randao_commitments<P: Preset>(
        _state: &mut impl PostGloasBeaconState<P>,
    ) -> Result<()> {
        Ok(())
    }

    #[must_use]
    pub const fn count_registration_signatures<P: Preset>(_body: &BeaconBlockBody<P>) -> usize {
        0
    }

    pub fn verify_registration_signatures<P: Preset>(
        _config: &Config,
        _pubkey_cache: &PubkeyCache,
        _state: &(impl PostGloasBeaconState<P> + ?Sized),
        _body: &BeaconBlockBody<P>,
        _verifier: impl Verifier,
    ) -> Result<()> {
        Ok(())
    }

    pub const fn has_randao_commitment<P: Preset>(
        _state: &(impl PostGloasBeaconState<P> + ?Sized),
        _index: ValidatorIndex,
    ) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(all(test, feature = "eip8321"))]
mod tests {
    use anyhow::Result;
    use bls::{
        PublicKeyBytes, SecretKey, SecretKeyBytes, SignatureBytes,
        traits::{SecretKey as _, SignatureBytes as _},
    };
    use helper_functions::{
        misc,
        signing::{RandaoEpoch, SignForAllForksWithGenesis as _, SignForSingleFork as _},
        verifier::{MultiVerifier, NullVerifier, SingleVerifier, VerifierOption},
    };
    use itertools::Itertools as _;
    use pubkey_cache::PubkeyCache;
    use ssz::{Hc, SszList as _, SszListMut as _};
    use std_ext::CopyExt as _;
    use tap::{Conv as _, TryConv as _};
    use types::{
        config::Config,
        gloas::{
            beacon_state::BeaconState as GloasBeaconState,
            consts::{COMMITMENT_REGISTRATION_DELAY, MAX_RANDAO_COMMITMENT_REGISTRATIONS},
            containers::{
                BeaconBlock, BeaconBlockBody, PendingRandaoCommitment,
                RandaoCommitmentRegistration, SignedBeaconBlock,
                SignedRandaoCommitmentRegistration,
            },
        },
        phase0::{consts::FAR_FUTURE_EPOCH, containers::Validator, primitives::H256},
        preset::Minimal,
        traits::BeaconState as _,
    };

    use super::*;
    use crate::unphased::Error;

    const PREVIOUS_MIX: H256 = H256::repeat_byte(7);
    const CHAIN_SECRET: H256 = H256::repeat_byte(42);

    #[test]
    fn hash_chain_reveal_advances_commitment_and_folds_mix() -> Result<()> {
        let reveal = CHAIN_SECRET;
        let commitment = hashing::blake3_chain_link(reveal);
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        *state.randao_commitments.get_mut(0)? = commitment;

        process_randao(
            &Config::minimal(),
            &PubkeyCache::default(),
            &mut state,
            &body_with_hash_chain_reveal(reveal),
            NullVerifier,
        )?;

        assert_eq!(*state.randao_commitments.get(0)?, reveal);
        assert_eq!(
            *state.randao_mixes.mod_index(0),
            hashing::blake3_256_256(PREVIOUS_MIX, reveal),
        );

        Ok(())
    }

    #[test]
    fn consecutive_reveals_consume_one_chain_link_each() -> Result<()> {
        let config = Config::minimal();
        let first = hashing::blake3_chain_link(CHAIN_SECRET);
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        *state.randao_commitments.get_mut(0)? = hashing::blake3_chain_link(first);

        // Both reveals land in the same epoch, so the second folds into the first one's mix.
        for reveal in [first, CHAIN_SECRET] {
            process_randao(
                &config,
                &PubkeyCache::default(),
                &mut state,
                &body_with_hash_chain_reveal(reveal),
                NullVerifier,
            )?;
        }

        assert_eq!(*state.randao_commitments.get(0)?, CHAIN_SECRET);
        assert_eq!(
            *state.randao_mixes.mod_index(0),
            hashing::blake3_256_256(hashing::blake3_256_256(PREVIOUS_MIX, first), CHAIN_SECRET),
        );

        Ok(())
    }

    #[test]
    fn hash_chain_reveal_rejects_value_that_is_not_a_preimage() -> Result<()> {
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        *state.randao_commitments.get_mut(0)? = hashing::blake3_chain_link(H256::repeat_byte(1));

        assert_process_randao_error(
            &mut state,
            body_with_hash_chain_reveal(H256::repeat_byte(2)),
        );

        Ok(())
    }

    #[test]
    fn hash_chain_reveal_rejects_zero_reveal() -> Result<()> {
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        *state.randao_commitments.get_mut(0)? = hashing::blake3_chain_link(H256::zero());

        assert_process_randao_error(&mut state, body_with_hash_chain_reveal(H256::zero()));

        Ok(())
    }

    #[test]
    fn hash_chain_reveal_rejects_non_empty_bls_reveal() -> Result<()> {
        let reveal = CHAIN_SECRET;
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        *state.randao_commitments.get_mut(0)? = hashing::blake3_chain_link(reveal);

        let body = BeaconBlockBody::<Minimal> {
            randao_reveal: SignatureBytes::repeat_byte(1),
            hash_chain_reveal: reveal,
            ..BeaconBlockBody::default()
        };

        assert_process_randao_error(&mut state, body);

        Ok(())
    }

    #[test]
    fn unregistered_proposer_rejects_hash_chain_reveal() -> Result<()> {
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        assert_process_randao_error(&mut state, body_with_hash_chain_reveal(CHAIN_SECRET));

        Ok(())
    }

    #[test]
    fn unregistered_proposer_verifies_bls_reveal_and_xors_the_mix() -> Result<()> {
        let config = Config::minimal();
        let secret_key = secret_key();
        let mut state = state_with_validator(secret_key.to_public_key().into())?;
        let signature = RandaoEpoch::from(0)
            .sign(&config, &state, &secret_key)
            .into();

        let body = BeaconBlockBody::<Minimal> {
            randao_reveal: signature,
            ..BeaconBlockBody::default()
        };

        process_randao(
            &config,
            &PubkeyCache::default(),
            &mut state,
            &body,
            SingleVerifier,
        )?;

        assert_eq!(
            *state.randao_mixes.mod_index(0),
            PREVIOUS_MIX ^ hashing::hash_768(signature),
        );
        assert!(state.randao_commitments.get(0)?.is_zero());

        Ok(())
    }

    #[test]
    fn unregistered_proposer_rejects_bls_reveal_for_wrong_epoch() -> Result<()> {
        let config = Config::minimal();
        let secret_key = secret_key();
        let mut state = state_with_validator(secret_key.to_public_key().into())?;

        let body = BeaconBlockBody::<Minimal> {
            randao_reveal: RandaoEpoch::from(1)
                .sign(&config, &state, &secret_key)
                .into(),
            ..BeaconBlockBody::default()
        };

        assert!(
            process_randao(
                &config,
                &PubkeyCache::default(),
                &mut state,
                &body,
                SingleVerifier,
            )
            .is_err(),
        );
        assert_eq!(*state.randao_mixes.mod_index(0), PREVIOUS_MIX);

        Ok(())
    }

    #[test]
    fn registration_queues_commitment_with_activation_delay() -> Result<()> {
        let config = Config::minimal();
        let pubkey_cache = PubkeyCache::default();
        let secret_key = secret_key();
        let mut state = state_with_validator(secret_key.to_public_key().into())?;

        let registration = RandaoCommitmentRegistration {
            validator_index: 0,
            commitment: H256::repeat_byte(3),
        };

        let signed_registration = SignedRandaoCommitmentRegistration {
            message: registration,
            signature: registration.sign(&config, &state, &secret_key).into(),
        };

        process_randao_commitment_registration(
            &config,
            &pubkey_cache,
            &mut state,
            signed_registration,
            SingleVerifier,
        )?;

        assert_eq!(
            state
                .pending_randao_commitments
                .into_iter()
                .copied()
                .collect_vec(),
            [PendingRandaoCommitment {
                validator_index: 0,
                commitment: H256::repeat_byte(3),
                activation_epoch: COMMITMENT_REGISTRATION_DELAY,
            }],
        );

        // The commitment is not active until `process_pending_randao_commitments` applies it.
        assert!(state.randao_commitments.get(0)?.is_zero());

        // A second registration for the same validator is rejected while the first is pending.
        assert!(
            process_randao_commitment_registration(
                &config,
                &pubkey_cache,
                &mut state,
                signed_registration,
                SingleVerifier,
            )
            .is_err(),
        );

        Ok(())
    }

    #[test]
    fn registration_rejects_registered_validator_and_zero_commitment() -> Result<()> {
        let config = Config::minimal();
        let pubkey_cache = PubkeyCache::default();
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        let zero_commitment = SignedRandaoCommitmentRegistration::default();

        assert!(
            process_randao_commitment_registration(
                &config,
                &pubkey_cache,
                &mut state,
                zero_commitment,
                NullVerifier,
            )
            .is_err(),
        );

        *state.randao_commitments.get_mut(0)? = H256::repeat_byte(5);

        let registration = SignedRandaoCommitmentRegistration {
            message: RandaoCommitmentRegistration {
                validator_index: 0,
                commitment: H256::repeat_byte(6),
            },
            ..SignedRandaoCommitmentRegistration::default()
        };

        assert!(
            process_randao_commitment_registration(
                &config,
                &pubkey_cache,
                &mut state,
                registration,
                NullVerifier,
            )
            .is_err(),
        );

        Ok(())
    }

    #[test]
    fn registration_rejects_tampered_signature() -> Result<()> {
        let config = Config::minimal();
        let secret_key = secret_key();
        let mut state = state_with_validator(secret_key.to_public_key().into())?;

        let mut message = RandaoCommitmentRegistration {
            validator_index: 0,
            commitment: H256::repeat_byte(3),
        };

        let signature = message.sign(&config, &state, &secret_key).into();

        message.commitment = H256::repeat_byte(4);

        assert!(
            process_randao_commitment_registration(
                &config,
                &PubkeyCache::default(),
                &mut state,
                SignedRandaoCommitmentRegistration { message, signature },
                SingleVerifier,
            )
            .is_err(),
        );
        assert_eq!(state.pending_randao_commitments.len_usize(), 0);

        Ok(())
    }

    #[test]
    fn registrations_over_the_per_block_limit_are_rejected_before_queuing() -> Result<()> {
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        let registration = SignedRandaoCommitmentRegistration {
            message: RandaoCommitmentRegistration {
                validator_index: 0,
                commitment: H256::repeat_byte(3),
            },
            signature: SignatureBytes::empty(),
        };

        let body = BeaconBlockBody::<Minimal> {
            randao_commitment_registrations: vec![
                registration;
                MAX_RANDAO_COMMITMENT_REGISTRATIONS + 1
            ]
            .try_into()?,
            ..BeaconBlockBody::default()
        };

        let error = process_randao_commitment_registrations(
            &Config::minimal(),
            &PubkeyCache::default(),
            &mut state,
            &body,
            NullVerifier,
        )
        .expect_err("more registrations than the per-block limit must be invalid");

        assert!(matches!(
            error.downcast_ref::<Error<Minimal>>(),
            Some(Error::TooManyOperations {
                maximum: MAX_RANDAO_COMMITMENT_REGISTRATIONS,
                ..
            }),
        ));

        // The limit is checked before any registration is queued.
        assert_eq!(state.pending_randao_commitments.len_usize(), 0);

        Ok(())
    }

    #[test]
    fn only_due_pending_commitments_are_activated() -> Result<()> {
        let mut state = state_with_validator(PublicKeyBytes::default())?;

        state.validators_mut().push(Validator::default())?;
        state.randao_commitments.push(H256::zero())?;

        // The state is in epoch 0, so `next_epoch` is 1.
        let due = PendingRandaoCommitment {
            validator_index: 0,
            commitment: H256::repeat_byte(1),
            activation_epoch: 1,
        };

        let not_due = PendingRandaoCommitment {
            validator_index: 1,
            commitment: H256::repeat_byte(2),
            activation_epoch: 2,
        };

        state.pending_randao_commitments.push(due)?;
        state.pending_randao_commitments.push(not_due)?;

        process_pending_randao_commitments(&mut state)?;

        assert_eq!(*state.randao_commitments.get(0)?, due.commitment);
        assert!(state.randao_commitments.get(1)?.is_zero());
        assert_eq!(
            state
                .pending_randao_commitments
                .into_iter()
                .copied()
                .collect_vec(),
            [not_due],
        );

        Ok(())
    }

    #[test]
    fn pending_commitment_activates_at_the_delay_and_cannot_be_replayed() -> Result<()> {
        let config = Config::minimal();
        let pubkey_cache = PubkeyCache::default();
        let secret_key = secret_key();
        let mut state = state_with_validator(secret_key.to_public_key().into())?;

        let message = RandaoCommitmentRegistration {
            validator_index: 0,
            commitment: hashing::blake3_chain_link(CHAIN_SECRET),
        };

        let registration = SignedRandaoCommitmentRegistration {
            signature: message.sign(&config, &state, &secret_key).into(),
            message,
        };

        process_randao_commitment_registration(
            &config,
            &pubkey_cache,
            &mut state,
            registration,
            SingleVerifier,
        )?;

        // Not yet due in epoch 0.
        process_pending_randao_commitments(&mut state)?;

        assert!(state.randao_commitments.get(0)?.is_zero());
        assert_eq!(state.pending_randao_commitments.len_usize(), 1);

        // `process_pending_randao_commitments` compares against the next epoch, so the
        // registration becomes active one epoch before `COMMITMENT_REGISTRATION_DELAY`.
        state.slot =
            misc::compute_start_slot_at_epoch::<Minimal>(COMMITMENT_REGISTRATION_DELAY - 1);

        process_pending_randao_commitments(&mut state)?;

        assert_eq!(*state.randao_commitments.get(0)?, message.commitment);
        assert_eq!(state.pending_randao_commitments.len_usize(), 0);

        // Replaying the same registration is rejected now that the validator is registered.
        assert!(
            process_randao_commitment_registration(
                &config,
                &pubkey_cache,
                &mut state,
                registration,
                SingleVerifier,
            )
            .is_err(),
        );

        Ok(())
    }

    #[test]
    fn block_signature_verifier_accepts_a_valid_registration() -> Result<()> {
        let config = Config::minimal();
        let secret_key = secret_key();
        let state = state_with_validator(secret_key.to_public_key().into())?;

        let message = RandaoCommitmentRegistration {
            validator_index: 0,
            commitment: H256::repeat_byte(3),
        };

        let registration = SignedRandaoCommitmentRegistration {
            signature: message.sign(&config, &state, &secret_key).into(),
            message,
        };

        crate::gloas::verify_signatures(
            &config,
            &PubkeyCache::default(),
            &state,
            &block_with_registrations(&config, &state, &secret_key, vec![registration])?,
            isolating_verifier(),
        )
    }

    #[test]
    fn block_signature_verifier_rejects_a_forged_registration() -> Result<()> {
        let config = Config::minimal();
        let secret_key = secret_key();
        let state = state_with_validator(secret_key.to_public_key().into())?;

        // A block with no registrations passes, which isolates the registration as the only
        // difference between the two cases below.
        crate::gloas::verify_signatures(
            &config,
            &PubkeyCache::default(),
            &state,
            &block_with_registrations(&config, &state, &secret_key, vec![])?,
            isolating_verifier(),
        )?;

        let mut message = RandaoCommitmentRegistration {
            validator_index: 0,
            commitment: H256::repeat_byte(3),
        };

        let signature = message.sign(&config, &state, &secret_key).into();

        message.commitment = H256::repeat_byte(4);

        assert!(
            crate::gloas::verify_signatures(
                &config,
                &PubkeyCache::default(),
                &state,
                &block_with_registrations(
                    &config,
                    &state,
                    &secret_key,
                    vec![SignedRandaoCommitmentRegistration { message, signature }],
                )?,
                isolating_verifier(),
            )
            .is_err(),
            "the block signature verification path accepted a forged registration",
        );

        Ok(())
    }

    fn secret_key() -> SecretKey {
        b"????????????????????????????????"
            .copy()
            .conv::<SecretKeyBytes>()
            .try_conv::<SecretKey>()
            .expect("bytes are a valid secret key")
    }

    fn state_with_validator(pubkey: PublicKeyBytes) -> Result<GloasBeaconState<Minimal>> {
        let mut state = GloasBeaconState::<Minimal>::default();

        state.validators_mut().push(Validator {
            pubkey,
            withdrawal_credentials: H256::zero(),
            effective_balance: 0,
            slashed: false,
            activation_eligibility_epoch: FAR_FUTURE_EPOCH,
            activation_epoch: 0,
            exit_epoch: FAR_FUTURE_EPOCH,
            withdrawable_epoch: FAR_FUTURE_EPOCH,
        })?;

        state.randao_commitments.push(H256::zero())?;

        *state.randao_mixes.mod_index_mut(0) = PREVIOUS_MIX;

        Ok(state)
    }

    fn body_with_hash_chain_reveal(reveal: H256) -> BeaconBlockBody<Minimal> {
        BeaconBlockBody {
            randao_reveal: SignatureBytes::empty(),
            hash_chain_reveal: reveal,
            ..BeaconBlockBody::default()
        }
    }

    /// A block whose only real signatures are the legacy RANDAO reveal and the registrations,
    /// so that `verify_signatures` exercises exactly those.
    fn block_with_registrations(
        config: &Config,
        state: &GloasBeaconState<Minimal>,
        secret_key: &SecretKey,
        registrations: Vec<SignedRandaoCommitmentRegistration>,
    ) -> Result<SignedBeaconBlock<Minimal>> {
        Ok(SignedBeaconBlock {
            message: Hc::new(BeaconBlock {
                body: BeaconBlockBody {
                    randao_reveal: RandaoEpoch::from(0).sign(config, state, secret_key).into(),
                    randao_commitment_registrations: registrations.try_into()?,
                    ..BeaconBlockBody::default()
                },
                ..BeaconBlock::default()
            }),
            signature: SignatureBytes::empty(),
        })
    }

    fn isolating_verifier() -> MultiVerifier {
        MultiVerifier::new([
            VerifierOption::SkipBlockRootSignature,
            VerifierOption::SkipBlockSyncAggregateSignature,
        ])
    }

    fn assert_process_randao_error(
        state: &mut GloasBeaconState<Minimal>,
        body: BeaconBlockBody<Minimal>,
    ) {
        let result = process_randao(
            &Config::minimal(),
            &PubkeyCache::default(),
            state,
            &body,
            NullVerifier,
        );

        assert!(result.is_err());
    }
}
