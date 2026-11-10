use crate::phase0::primitives::DomainType;
pub const DOMAIN_RANDAO_COMMITMENT_REGISTRATION: DomainType =
    crate::phase0::primitives::H32([0x11, 0, 0, 0]);
pub const HASH_CHAIN_RANDAO_DST: &[u8] = b"HASH_CHAIN_RANDAO";
pub const COMMITMENT_REGISTRATION_DELAY: u64 = 3;
pub const MAX_RANDAO_COMMITMENT_REGISTRATIONS: usize = 128;
