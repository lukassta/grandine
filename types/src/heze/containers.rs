//! Neutral Heze scaffold: inherited wire layout, before EIP feature changes.
pub use crate::gloas::containers::{
    Attestation, AttesterSlashing, BuilderPendingPayment, BuilderPendingWithdrawal,
    ExecutionRequests, PayloadAttestation,
};
use crate::{
    altair::containers::SyncAggregate,
    bellatrix::primitives::Gas,
    capella::containers::SignedBlsToExecutionChange,
    deneb::primitives::KzgCommitment,
    gloas::primitives::BuilderIndex,
    phase0::{
        containers::{Deposit, Eth1Data, ProposerSlashing, SignedVoluntaryExit},
        primitives::{ExecutionAddress, ExecutionBlockHash, Gwei, H256, Slot, ValidatorIndex},
    },
    preset::Preset,
};
use bls::SignatureBytes;
use core::marker::PhantomData;
use serde::{Deserialize, Serialize};
use ssz::{Hc, ProgressiveList, Ssz};

#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
#[ssz(stable(active = [1; 12]))]
pub struct ExecutionPayloadBid<P: Preset> {
    pub parent_block_hash: ExecutionBlockHash,
    pub parent_block_root: H256,
    pub block_hash: ExecutionBlockHash,
    pub prev_randao: H256,
    pub fee_recipient: ExecutionAddress,
    #[serde(with = "serde_utils::string_or_native")]
    pub gas_limit: Gas,
    #[serde(with = "serde_utils::string_or_native")]
    pub builder_index: BuilderIndex,
    #[serde(with = "serde_utils::string_or_native")]
    pub slot: Slot,
    #[serde(with = "serde_utils::string_or_native")]
    pub value: Gwei,
    #[serde(with = "serde_utils::string_or_native")]
    pub execution_payment: Gwei,
    pub blob_kzg_commitments: ProgressiveList<KzgCommitment>,
    pub execution_requests_root: H256,
    #[ssz(skip)]
    #[serde(skip)]
    pub phantom: PhantomData<P>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
pub struct SignedExecutionPayloadBid<P: Preset> {
    pub message: ExecutionPayloadBid<P>,
    pub signature: SignatureBytes,
}

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
#[ssz(stable(active = [1; 13]))]
pub struct BeaconBlockBody<P: Preset> {
    pub randao_reveal: SignatureBytes,
    pub eth1_data: Eth1Data,
    pub graffiti: H256,
    pub proposer_slashings: ProgressiveList<ProposerSlashing>,
    pub attester_slashings: ProgressiveList<AttesterSlashing<P>>,
    pub attestations: ProgressiveList<Attestation<P>>,
    pub deposits: ProgressiveList<Deposit>,
    pub voluntary_exits: ProgressiveList<SignedVoluntaryExit>,
    pub sync_aggregate: SyncAggregate<P>,
    pub bls_to_execution_changes: ProgressiveList<SignedBlsToExecutionChange>,
    pub signed_execution_payload_bid: SignedExecutionPayloadBid<P>,
    pub payload_attestations: ProgressiveList<PayloadAttestation<P>>,
    pub parent_execution_requests: ExecutionRequests<P>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
pub struct SignedBeaconBlock<P: Preset> {
    pub message: Hc<BeaconBlock<P>>,
    pub signature: SignatureBytes,
}
impl<P: Preset> SignedExecutionPayloadBid<P> {
    #[must_use]
    pub const fn blob_kzg_commitments(&self) -> &ProgressiveList<KzgCommitment> {
        &self.message.blob_kzg_commitments
    }
}
