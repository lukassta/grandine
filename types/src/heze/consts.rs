//! EIP-7805 constants at consensus-specs 12c8b24.
use crate::phase0::primitives::{DomainType, H32};
use hex_literal::hex;

pub const DOMAIN_INCLUSION_LIST_COMMITTEE: DomainType = H32(hex!("10000000"));
pub const INCLUSION_LIST_COMMITTEE_SIZE: usize = 16;

pub use crate::gloas::consts::{BUILDER_INDEX_SELF_BUILD, PAYLOAD_BUILDER_VERSION};
