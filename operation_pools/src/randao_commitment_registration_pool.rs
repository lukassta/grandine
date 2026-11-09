use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use eth1_api::ApiController;
use fork_choice_control::Wait;
use futures::{
    channel::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot::{self, Sender},
    },
    stream::StreamExt as _,
};
use helper_functions::verifier::SingleVerifier;
use logging::{debug_with_peers, warn_with_peers};
use ssz::SszList as _;
use transition_functions::eip8321;
use types::{
    combined::BeaconState,
    eip8321::containers::SignedRandaoCommitmentRegistration,
    nonstandard::ValidationOutcome,
    phase0::primitives::ValidatorIndex,
    preset::Preset,
    traits::BeaconState as _,
};

use crate::{
    messages::PoolToP2pMessage,
    misc::{Origin, PoolAdditionOutcome, PoolRejectionReason},
};

/// Pool of hash-chain RANDAO commitment registrations awaiting inclusion in a block.
///
/// A registration only takes effect once it is included in a block and
/// `COMMITMENT_REGISTRATION_DELAY` epochs have passed, and activation is a property of the
/// canonical state rather than of this node's broadcast history. If the including block is
/// orphaned the pending entry never enters the canonical queue, and the message stays valid
/// and includable — so entries are retained until the *finalized* state shows the validator
/// registered, rather than being dropped as soon as a block carries them.
pub struct RandaoCommitmentRegistrationPool {
    tx: UnboundedSender<PoolMessage>,
}

impl RandaoCommitmentRegistrationPool {
    #[must_use]
    pub fn new<P: Preset, W: Wait>(
        controller: ApiController<P, W>,
        pool_to_p2p_tx: UnboundedSender<PoolToP2pMessage>,
    ) -> (Arc<Self>, Service<P, W>) {
        let (tx, rx) = futures::channel::mpsc::unbounded();

        let pool = Arc::new(Self { tx });

        let service = Service {
            controller,
            registrations: HashMap::new(),
            local_indices: Vec::new(),
            pool_to_p2p_tx,
            rx,
        };

        (pool, service)
    }

    pub fn discard_old_randao_commitment_registrations(&self) {
        PoolMessage::DiscardOldRegistrations.send(&self.tx)
    }

    pub async fn handle_external_signed_randao_commitment_registration(
        &self,
        registration: Arc<SignedRandaoCommitmentRegistration>,
        origin: Origin,
    ) -> Result<PoolAdditionOutcome> {
        let (sender, receiver) = oneshot::channel();

        PoolMessage::HandleExternalRegistration(registration, origin, Some(sender)).send(&self.tx);

        receiver.await.map_err(Into::into)
    }

    pub fn notify_external_signed_randao_commitment_registration(
        &self,
        registration: Arc<SignedRandaoCommitmentRegistration>,
        origin: Origin,
    ) {
        PoolMessage::HandleExternalRegistration(registration, origin, None).send(&self.tx)
    }

    /// Add a registration produced by one of this node's own validators.
    ///
    /// The spec notes a node *should* prioritize locally received registrations so they reach
    /// the chain through self-published blocks even if the rest of the network censors them.
    pub fn notify_own_signed_randao_commitment_registration(
        &self,
        registration: Arc<SignedRandaoCommitmentRegistration>,
    ) {
        PoolMessage::HandleOwnRegistration(registration).send(&self.tx)
    }

    pub async fn signed_randao_commitment_registrations(
        &self,
    ) -> Result<Vec<SignedRandaoCommitmentRegistration>> {
        let (sender, receiver) = oneshot::channel();
        PoolMessage::RequestSignedRegistrations(sender).send(&self.tx);
        receiver.await.map_err(Into::into)
    }

    pub fn stop(&self) {
        PoolMessage::Stop.send(&self.tx)
    }
}

pub struct Service<P: Preset, W: Wait> {
    controller: ApiController<P, W>,
    registrations: HashMap<ValidatorIndex, SignedRandaoCommitmentRegistration>,
    /// Own validators' indices, most recent last, so local registrations can be offered to
    /// the block producer ahead of everyone else's.
    local_indices: Vec<ValidatorIndex>,
    pool_to_p2p_tx: UnboundedSender<PoolToP2pMessage>,
    rx: UnboundedReceiver<PoolMessage>,
}

impl<P: Preset, W: Wait> Service<P, W> {
    pub async fn run(mut self) -> Result<()> {
        while let Some(message) = self.rx.next().await {
            let success = match message {
                PoolMessage::DiscardOldRegistrations => {
                    self.discard_old_registrations();
                    true
                }
                PoolMessage::HandleExternalRegistration(registration, origin, sender) => {
                    let outcome = self.handle_external_registration(&registration, origin);

                    sender
                        .map(|sender| sender.send(outcome).is_ok())
                        .unwrap_or(true)
                }
                PoolMessage::HandleOwnRegistration(registration) => {
                    let validator_index = registration.message.validator_index;

                    self.registrations.insert(validator_index, *registration);

                    if !self.local_indices.contains(&validator_index) {
                        self.local_indices.push(validator_index);
                    }

                    true
                }
                PoolMessage::RequestSignedRegistrations(sender) => {
                    sender.send(self.signed_registrations()).is_ok()
                }
                PoolMessage::Stop => break,
            };

            if !success {
                warn_with_peers!("failed to send response because the receiver was dropped");
            }
        }

        Ok(())
    }

    /// Local registrations first, then everyone else's.
    fn signed_registrations(&self) -> Vec<SignedRandaoCommitmentRegistration> {
        let local = self
            .local_indices
            .iter()
            .filter_map(|index| self.registrations.get(index).copied());

        let external = self
            .registrations
            .iter()
            .filter(|(index, _)| !self.local_indices.contains(index))
            .map(|(_, registration)| *registration);

        local.chain(external).collect()
    }

    fn handle_external_registration(
        &mut self,
        registration: &SignedRandaoCommitmentRegistration,
        origin: Origin,
    ) -> PoolAdditionOutcome {
        let validation_outcome =
            tokio::task::block_in_place(|| self.validate_registration(registration));

        match validation_outcome {
            Ok(ValidationOutcome::Accept) => {
                match origin {
                    Origin::Api => {
                        PoolToP2pMessage::PublishSignedRandaoCommitmentRegistration(Arc::new(
                            *registration,
                        ))
                        .send(&self.pool_to_p2p_tx);
                    }
                    Origin::Gossip(gossip_id) => {
                        PoolToP2pMessage::Accept(gossip_id).send(&self.pool_to_p2p_tx);
                    }
                }

                self.registrations
                    .insert(registration.message.validator_index, *registration);

                PoolAdditionOutcome::Accept
            }
            Ok(ValidationOutcome::Ignore(_)) => {
                if let Origin::Gossip(gossip_id) = origin {
                    PoolToP2pMessage::Ignore(gossip_id).send(&self.pool_to_p2p_tx);
                }

                PoolAdditionOutcome::Ignore
            }
            Err(error) => {
                if let Origin::Gossip(gossip_id) = origin {
                    PoolToP2pMessage::Reject(
                        gossip_id,
                        PoolRejectionReason::InvalidRandaoCommitmentRegistration,
                    )
                    .send(&self.pool_to_p2p_tx);
                }

                warn_with_peers!(
                    "external RANDAO commitment registration rejected \
                     (error: {error}, registration: {registration:?})",
                );

                PoolAdditionOutcome::Reject(
                    PoolRejectionReason::InvalidRandaoCommitmentRegistration,
                    error,
                )
            }
        }
    }

    /// Gossip validation for `randao_commitment_registration`.
    ///
    /// The ignore-versus-reject split is what the spec's
    /// `validate_randao_commitment_registration_gossip` prescribes: a registration that is
    /// merely redundant is ignored, while a malformed or unauthenticated one is rejected and
    /// penalised. The redundancy cases are checked here and the remainder is delegated to
    /// `process_randao_commitment_registration`, so gossip and block processing cannot drift
    /// apart on what counts as valid.
    fn validate_registration(
        &self,
        registration: &SignedRandaoCommitmentRegistration,
    ) -> Result<ValidationOutcome> {
        let state = self
            .controller
            .preprocessed_state_at_current_slot_blocking()?;

        // [IGNORE] The head state has upgraded to EIP-8321.
        let BeaconState::Eip8321(eip8321_state) = state.as_ref() else {
            debug_with_peers!(
                "RANDAO commitment registration received before the EIP-8321 fork \
                 (registration: {registration:?}, slot: {})",
                state.slot(),
            );

            return Ok(ValidationOutcome::Ignore(false));
        };

        let index = registration.message.validator_index;

        // [REJECT] The validator index is valid.
        //
        // Checked before the ignore cases below because they index the registry.
        if index >= eip8321_state.validators().len_u64() {
            anyhow::bail!("unknown RANDAO registration validator: {index}");
        }

        // [IGNORE] This is the first valid registration received for the validator.
        if self.registrations.contains_key(&index) {
            return Ok(ValidationOutcome::Ignore(false));
        }

        // [IGNORE] The validator is not registered yet.
        if !eip8321_state.randao_commitments.get(index)?.is_zero() {
            return Ok(ValidationOutcome::Ignore(false));
        }

        // [IGNORE] The validator has no registration pending in the node's view.
        if eip8321_state
            .pending_randao_commitments
            .into_iter()
            .any(|pending| pending.validator_index == index)
        {
            return Ok(ValidationOutcome::Ignore(false));
        }

        // Everything left — a zero commitment or an invalid signature — is a [REJECT], and is
        // exactly what the consensus-side handler enforces. Run it against a throwaway copy
        // so the head state is untouched.
        let mut state_copy = eip8321_state.clone();

        eip8321::process_randao_commitment_registration(
            self.controller.chain_config(),
            self.controller.pubkey_cache(),
            &mut state_copy,
            registration,
            SingleVerifier,
        )?;

        Ok(ValidationOutcome::Accept)
    }

    /// Drop registrations that the finalized state shows as already registered.
    ///
    /// Deliberately keyed on the finalized state: a registration included in a block that is
    /// later orphaned must stay in the pool, because its pending entry never reached the
    /// canonical queue and the message remains includable.
    fn discard_old_registrations(&mut self) {
        let finalized_state = self.controller.last_finalized_state().value;

        let BeaconState::Eip8321(state) = finalized_state.as_ref() else {
            return;
        };

        self.registrations.retain(|index, _| {
            match state.randao_commitments.get(*index) {
                Ok(commitment) => commitment.is_zero(),
                // Too recent to appear in the finalized registry; keep it.
                Err(error) => {
                    debug_with_peers!(
                        "RANDAO commitment registration is too recent to discard: {error}"
                    );
                    true
                }
            }
        });

        self.local_indices
            .retain(|index| self.registrations.contains_key(index));
    }
}

enum PoolMessage {
    DiscardOldRegistrations,
    HandleExternalRegistration(
        Arc<SignedRandaoCommitmentRegistration>,
        Origin,
        Option<Sender<PoolAdditionOutcome>>,
    ),
    HandleOwnRegistration(Arc<SignedRandaoCommitmentRegistration>),
    RequestSignedRegistrations(Sender<Vec<SignedRandaoCommitmentRegistration>>),
    Stop,
}

impl PoolMessage {
    pub fn send(self, tx: &UnboundedSender<Self>) {
        if let Err(message) = tx.unbounded_send(self) {
            debug_with_peers!("internal send failed because the receiver was dropped: {message:?}");
        }
    }
}
