use bls::SignatureBytes;
use serde::{Deserialize, Serialize};
use ssz::{BitVector, Hc, ProgressiveList, Ssz};

use crate::{
    altair::containers::SyncAggregate,
    bellatrix::primitives::Gas,
    capella::containers::SignedBlsToExecutionChange,
    deneb::primitives::KzgCommitment,
    gloas::{
        containers::{Attestation, AttesterSlashing, ExecutionRequests, PayloadAttestation},
        primitives::{BuilderIndex, Transaction},
    },
    phase0::{
        containers::{ProposerSlashing, SignedVoluntaryExit},
        primitives::{ExecutionAddress, ExecutionBlockHash, Gwei, H256, Slot, ValidatorIndex},
    },
    preset::Preset,
};

#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
pub struct InclusionList<P: Preset> {
    #[serde(with = "serde_utils::string_or_native")]
    pub slot: Slot,
    #[serde(with = "serde_utils::string_or_native")]
    pub validator_index: ValidatorIndex,
    pub dependent_root: H256,
    pub transactions: ProgressiveList<Transaction<P>>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
pub struct SignedInclusionList<P: Preset> {
    pub message: InclusionList<P>,
    pub signature: SignatureBytes,
}

// Not a spec container. Identifies one validator's inclusion list for a slot
// and dependent root, as known to a node requesting it by committee index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)]
pub struct InclusionListIdentifier {
    #[serde(with = "serde_utils::string_or_native")]
    pub slot: Slot,
    pub dependent_root: H256,
    #[serde(with = "serde_utils::string_or_native")]
    pub validator_index: ValidatorIndex,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
#[ssz(stable(active = [1; 13]))]
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
    pub inclusion_list_bits: BitVector<P::InclusionListCommitteeSize>,
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
#[ssz(stable(active = [1,0,1,1,1,1,0,1,1,1,1,1,1]))]
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
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize, Ssz)]
#[serde(bound = "", deny_unknown_fields)]
pub struct SignedBeaconBlock<P: Preset> {
    pub message: Hc<BeaconBlock<P>>,
    pub signature: SignatureBytes,
}
