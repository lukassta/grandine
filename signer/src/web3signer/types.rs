use bls::SignatureBytes;
use serde::{Deserialize, Serialize};
use types::{phase0::primitives::H256, preset::Preset};

use crate::types::{ForkInfo, SigningMessage};

#[derive(Debug, Serialize)]
#[serde(bound = "")]
pub struct SigningRequest<'block, P: Preset> {
    // `type` is a keyword in Rust.
    #[serde(rename = "type")]
    message_type: MessageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    fork_info: Option<ForkInfo<P>>,
    #[serde(rename = "signingRoot")]
    signing_root: H256,
    #[serde(flatten)]
    message: SigningMessage<'block, P>,
}

impl<'block, P: Preset> SigningRequest<'block, P> {
    pub const fn new(
        message: SigningMessage<'block, P>,
        signing_root: H256,
        fork_info: Option<ForkInfo<P>>,
    ) -> Self {
        let message_type = match message {
            SigningMessage::AggregationSlot { .. } => MessageType::AggregationSlot,
            SigningMessage::AggregateAndProof(_) => MessageType::AggregateAndProof,
            SigningMessage::Attestation(_) => MessageType::Attestation,
            SigningMessage::BeaconBlock { .. } => MessageType::BlockV2,
            SigningMessage::ExecutionPayloadEnvelope(_) => MessageType::ExecutionPayloadEnvelope,
            SigningMessage::InclusionList(_) => MessageType::InclusionList,
            SigningMessage::RandaoReveal { .. } => MessageType::RandaoReveal,
            SigningMessage::PayloadAttestation(_) => MessageType::PayloadAttestation,
            SigningMessage::SyncCommitteeMessage { .. } => MessageType::SyncCommitteeMessage,
            SigningMessage::SyncAggregatorSelectionData(_) => {
                MessageType::SyncCommitteeSelectionProof
            }
            SigningMessage::ContributionAndProof(_) => {
                MessageType::SyncCommitteeContributionAndProof
            }
            SigningMessage::ValidatorRegistration(_) => MessageType::ValidatorRegistration,
            SigningMessage::VoluntaryExit(_) => MessageType::VoluntaryExit,
            SigningMessage::ProposerPreferences(_) => MessageType::ProposerPreferences,
            SigningMessage::RandaoCommitmentRegistration(_) => {
                MessageType::RandaoCommitmentRegistration
            }
        };

        Self {
            message_type,
            fork_info,
            signing_root,
            message,
        }
    }
}

// TODO(gloas): Currently, Web3signer only plans to support payload attestation message signing,
// See this PR for details: https://github.com/Consensys/web3signer/pull/1159, but we expect them
// to add support for builder message signing in the future. When that happens, we can remove this.
// TODO
#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(test, derive(Clone, Copy, Deserialize))]
enum MessageType {
    AggregationSlot,
    AggregateAndProof,
    Attestation,
    BlockV2,
    ExecutionPayloadEnvelope,
    InclusionList,
    RandaoReveal,
    PayloadAttestation,
    SyncCommitteeMessage,
    SyncCommitteeSelectionProof,
    SyncCommitteeContributionAndProof,
    ValidatorRegistration,
    VoluntaryExit,
    ProposerPreferences,
    RandaoCommitmentRegistration,
}

#[derive(Debug, Deserialize)]
pub struct SigningResponse {
    pub signature: SignatureBytes,
}

#[cfg(test)]
mod tests {
    use types::{heze::containers::InclusionList, preset::Mainnet};

    use super::*;

    // This exists mainly to ensure `MessageType::BlockV2` is renamed correctly.
    #[test]
    fn message_type_is_serialized_correctly() {
        assert_eq!(
            serde_aux::serde_introspection::serde_introspect::<MessageType>(),
            [
                "AGGREGATION_SLOT",
                "AGGREGATE_AND_PROOF",
                "ATTESTATION",
                "BLOCK_V2",
                "EXECUTION_PAYLOAD_ENVELOPE",
                "INCLUSION_LIST",
                "RANDAO_REVEAL",
                "PAYLOAD_ATTESTATION",
                "SYNC_COMMITTEE_MESSAGE",
                "SYNC_COMMITTEE_SELECTION_PROOF",
                "SYNC_COMMITTEE_CONTRIBUTION_AND_PROOF",
                "VALIDATOR_REGISTRATION",
                "VOLUNTARY_EXIT",
                "PROPOSER_PREFERENCES",
                "RANDAO_COMMITMENT_REGISTRATION",
            ],
        );
    }

    #[test]
    fn inclusion_list_request_is_serialized_correctly() {
        let request = SigningRequest::new(
            SigningMessage::InclusionList(InclusionList::<Mainnet>::default()),
            H256::zero(),
            None,
        );
        let request = serde_json::to_value(request).expect("signing request should serialize");

        assert_eq!(request["type"], "INCLUSION_LIST");
        assert!(request.get("inclusion_list").is_some());
    }
}
