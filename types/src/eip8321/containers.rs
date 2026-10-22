//! EIP-8321 containers at consensus-specs 12c8b24, built on Heze.
pub use crate::heze::containers::{
    Attestation, AttesterSlashing, BuilderPendingPayment, BuilderPendingWithdrawal,
    ExecutionPayloadBid, ExecutionRequests, PayloadAttestation, SignedExecutionPayloadBid,
};
use crate::{
    altair::containers::SyncAggregate,
    capella::containers::SignedBlsToExecutionChange,
    phase0::{
        containers::{ProposerSlashing, SignedVoluntaryExit},
        primitives::{Epoch, H256, Slot, ValidatorIndex},
    },
    preset::Preset,
};
use bls::SignatureBytes;
use serde::{Deserialize, Serialize};
use ssz::{Hc, ProgressiveList, Ssz};
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
pub struct BeaconBlock<P: Preset> {
    #[serde(with = "serde_utils::string_or_native")]
    pub slot: Slot,
    #[serde(with = "serde_utils::string_or_native")]
    pub proposer_index: ValidatorIndex,
    pub parent_root: H256,
    pub state_root: H256,
    pub body: BeaconBlockBody<P>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
#[ssz(stable(active = [1, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1]))]
pub struct BeaconBlockBody<P: Preset> {
    pub randao_reveal: SignatureBytes,
    pub graffiti: H256,
    pub proposer_slashings: ProgressiveList<ProposerSlashing>,
    pub attester_slashings: ProgressiveList<AttesterSlashing<P>>,
    pub attestations: ProgressiveList<Attestation<P>>,
    pub voluntary_exits: ProgressiveList<SignedVoluntaryExit>,
    pub sync_aggregate: SyncAggregate<P>,
    pub bls_to_execution_changes: ProgressiveList<SignedBlsToExecutionChange>,
    pub signed_execution_payload_bid: SignedExecutionPayloadBid<P>,
    pub payload_attestations: ProgressiveList<PayloadAttestation<P>>,
    pub parent_execution_requests: ExecutionRequests<P>,
    pub hash_chain_reveal: H256,
    pub randao_commitment_registrations: ProgressiveList<SignedRandaoCommitmentRegistration>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
pub struct SignedBeaconBlock<P: Preset> {
    pub message: Hc<BeaconBlock<P>>,
    pub signature: SignatureBytes,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(deny_unknown_fields)]
pub struct RandaoCommitmentRegistration {
    #[serde(with = "serde_utils::string_or_native")]
    pub validator_index: ValidatorIndex,
    pub commitment: H256,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(deny_unknown_fields)]
pub struct SignedRandaoCommitmentRegistration {
    pub message: RandaoCommitmentRegistration,
    pub signature: SignatureBytes,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(deny_unknown_fields)]
pub struct PendingRandaoCommitment {
    #[serde(with = "serde_utils::string_or_native")]
    pub validator_index: ValidatorIndex,
    pub commitment: H256,
    #[serde(with = "serde_utils::string_or_native")]
    pub activation_epoch: Epoch,
}
