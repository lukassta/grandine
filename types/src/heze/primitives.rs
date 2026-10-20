use crate::{gloas::primitives::Transaction, phase0::primitives::ValidatorIndex};
use ssz::{BitVector, ContiguousVector, ProgressiveList};
use typenum::U16;

pub type InclusionListBits = BitVector<U16>;
pub type InclusionListCommittee = ContiguousVector<ValidatorIndex, U16>;
pub type Transactions<P> = ProgressiveList<Transaction<P>>;
