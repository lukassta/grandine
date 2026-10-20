//! Common, read-only fields shared by Gloas and Heze bids.
//! Never used for SSZ, signing, or storing the latest bid.
use crate::{
    bellatrix::primitives::Gas,
    deneb::primitives::KzgCommitment,
    gloas::primitives::BuilderIndex,
    phase0::primitives::{ExecutionAddress, ExecutionBlockHash, Gwei, H256, Slot},
    preset::Preset,
};
use core::marker::PhantomData;
use ssz::ProgressiveList;
#[derive(Clone, Debug)]
pub struct ExecutionPayloadBidFields<P: Preset> {
    pub parent_block_hash: ExecutionBlockHash,
    pub parent_block_root: H256,
    pub block_hash: ExecutionBlockHash,
    pub prev_randao: H256,
    pub fee_recipient: ExecutionAddress,
    pub gas_limit: Gas,
    pub builder_index: BuilderIndex,
    pub slot: Slot,
    pub value: Gwei,
    pub execution_payment: Gwei,
    pub blob_kzg_commitments: ProgressiveList<KzgCommitment>,
    pub execution_requests_root: H256,
    pub phantom: PhantomData<P>,
}
#[derive(Clone, Debug)]
pub struct SignedExecutionPayloadBidFields<P: Preset> {
    pub message: ExecutionPayloadBidFields<P>,
    pub signature: bls::SignatureBytes,
}
impl<P: Preset> SignedExecutionPayloadBidFields<P> {
    #[must_use]
    pub const fn blob_kzg_commitments(&self) -> &ProgressiveList<KzgCommitment> {
        &self.message.blob_kzg_commitments
    }
}

impl<P: Preset> From<&crate::gloas::containers::ExecutionPayloadBid<P>>
    for ExecutionPayloadBidFields<P>
{
    fn from(bid: &crate::gloas::containers::ExecutionPayloadBid<P>) -> Self {
        Self {
            parent_block_hash: bid.parent_block_hash,
            parent_block_root: bid.parent_block_root,
            block_hash: bid.block_hash,
            prev_randao: bid.prev_randao,
            fee_recipient: bid.fee_recipient,
            gas_limit: bid.gas_limit,
            builder_index: bid.builder_index,
            slot: bid.slot,
            value: bid.value,
            execution_payment: bid.execution_payment,
            blob_kzg_commitments: bid.blob_kzg_commitments.clone(),
            execution_requests_root: bid.execution_requests_root,
            phantom: bid.phantom,
        }
    }
}

impl<P: Preset> From<&crate::gloas::containers::SignedExecutionPayloadBid<P>>
    for SignedExecutionPayloadBidFields<P>
{
    fn from(bid: &crate::gloas::containers::SignedExecutionPayloadBid<P>) -> Self {
        Self {
            message: (&bid.message).into(),
            signature: bid.signature,
        }
    }
}

impl<P: Preset> From<&crate::heze::containers::ExecutionPayloadBid<P>>
    for ExecutionPayloadBidFields<P>
{
    fn from(bid: &crate::heze::containers::ExecutionPayloadBid<P>) -> Self {
        Self {
            parent_block_hash: bid.parent_block_hash,
            parent_block_root: bid.parent_block_root,
            block_hash: bid.block_hash,
            prev_randao: bid.prev_randao,
            fee_recipient: bid.fee_recipient,
            gas_limit: bid.gas_limit,
            builder_index: bid.builder_index,
            slot: bid.slot,
            value: bid.value,
            execution_payment: bid.execution_payment,
            blob_kzg_commitments: bid.blob_kzg_commitments.clone(),
            execution_requests_root: bid.execution_requests_root,
            phantom: bid.phantom,
        }
    }
}

impl<P: Preset> From<&crate::heze::containers::SignedExecutionPayloadBid<P>>
    for SignedExecutionPayloadBidFields<P>
{
    fn from(bid: &crate::heze::containers::SignedExecutionPayloadBid<P>) -> Self {
        Self {
            message: (&bid.message).into(),
            signature: bid.signature,
        }
    }
}
