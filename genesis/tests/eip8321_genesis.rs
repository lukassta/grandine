//! Eip8321 genesis regression, independent of the downloaded legacy spec archive.
#![expect(
    unused_crate_dependencies,
    reason = "This focused integration target uses only the genesis builder dependencies"
)]
use anyhow::Result;
use bls::{SecretKey, SecretKeyBytes, traits::SecretKey as _};
use genesis::{Incremental, beacon_block};
use helper_functions::signing::SignForAllForks as _;
use pubkey_cache::PubkeyCache;
use ssz::SszList as _;
use types::{
    config::Config,
    nonstandard::Phase,
    phase0::{
        containers::{DepositData, DepositMessage},
        primitives::{ExecutionBlockHash, H256},
    },
    preset::{Minimal, Preset as _},
    traits::{BeaconState as _, SignedBeaconBlock as _},
};

#[test]
fn eip8321_genesis_finishes_without_legacy_fields() -> Result<()> {
    let config = Config::minimal().start_and_stay_in(Phase::Eip8321);
    let cache = PubkeyCache::default();
    let secret_key = SecretKey::try_from(SecretKeyBytes::from([b'?'; 32]))?;
    let message = DepositMessage {
        pubkey: secret_key.to_public_key().into(),
        withdrawal_credentials: H256::zero(),
        amount: Minimal::MAX_EFFECTIVE_BALANCE / 2,
    };
    let data = DepositData {
        pubkey: message.pubkey,
        withdrawal_credentials: message.withdrawal_credentials,
        amount: message.amount,
        signature: message.sign(&Minimal::default_config(), &secret_key).into(),
    };
    let mut builder = Incremental::<Minimal>::new(&config);
    builder.add_deposit_data(&cache, data, 0)?;
    builder.add_deposit_data(&cache, data, 1)?;
    let (state, _) = builder.finish(&cache, ExecutionBlockHash::repeat_byte(9), None)?;
    assert_eq!(state.phase(), Phase::Eip8321);
    assert!(state.eth1_deposits().is_none());
    assert_eq!(state.validators().len_usize(), 1);
    assert_eq!(state.fork().current_version, config.eip8321_fork_version);
    assert_eq!(state.fork().previous_version, config.eip8321_fork_version);
    let types::combined::BeaconState::Eip8321(post) = &state else {
        anyhow::bail!("wrong genesis phase");
    };
    assert_eq!(post.randao_commitments.len_u64(), 1);
    assert!(post.randao_commitments.get(0)?.is_zero());
    assert_eq!(post.pending_randao_commitments.len_u64(), 0);
    let block = beacon_block(&state);
    assert_eq!(
        state.latest_block_header().body_root,
        block.message().body().hash_tree_root()
    );
    Ok(())
}
