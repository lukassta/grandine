//! Additional project regressions; these are not official consensus-spec test cases.
#![expect(unused_crate_dependencies, reason = "Focused integration target")]
use anyhow::{Context as _, Result, ensure};
use helper_functions::{
    slot_report::NullSlotReport,
    verifier::{NullVerifier, SingleVerifier},
};
use pubkey_cache::PubkeyCache;
use spec_test_utils::Case;
use ssz::{SszHash as _, SszReadDefault as _};
use std::path::{Path, PathBuf};
use transition_functions::{combined, eip8321};
use types::{
    combined::BeaconState,
    nonstandard::Phase,
    preset::{Minimal, Preset},
};
fn root() -> PathBuf {
    std::env::var_os("EIP8321_VECTORS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors/reference"),
        PathBuf::from,
    )
}
/// Additional Rust regression for genesis binding, fork independence and replay.
#[test]
fn eip8321_registration_domain_and_replay() -> Result<()> {
    let path = root()
        .join("minimal/eip8321/operations/randao_commitment_registration/pyspec_tests/success");
    let case = Case::from(path.to_str().context("fixture path")?);
    let original = types::eip8321::beacon_state::BeaconState::<Minimal>::from_ssz_default(
        case.bytes("pre.ssz_snappy"),
    )?;
    let registration =
        types::eip8321::containers::SignedRandaoCommitmentRegistration::from_ssz_default(
            case.bytes("randao_commitment_registration.ssz_snappy"),
        )?;
    let config = Minimal::default_config();
    let cache = PubkeyCache::default();
    let mut state = original.clone();
    state.fork.current_version = types::phase0::primitives::H32([0x44; 4]);
    eip8321::process_randao_commitment_registration(
        &config,
        &cache,
        &mut state,
        &registration,
        SingleVerifier,
    )?;
    let after = state.hash_tree_root();
    ensure!(
        eip8321::process_randao_commitment_registration(
            &config,
            &cache,
            &mut state,
            &registration,
            SingleVerifier
        )
        .is_err(),
        "pending registration replay accepted"
    );
    ensure!(
        state.hash_tree_root() == after,
        "failed registration mutated state"
    );
    let mut wrong_genesis = original;
    wrong_genesis.genesis_validators_root = types::phase0::primitives::H256::repeat_byte(0x44);
    ensure!(
        eip8321::process_randao_commitment_registration(
            &config,
            &cache,
            &mut wrong_genesis,
            &registration,
            SingleVerifier
        )
        .is_err(),
        "registration valid against another genesis"
    );
    Ok(())
}

#[test]
fn eip8321_chain_consumes_one_link_per_proposal_and_rejects_replay() -> Result<()> {
    use ssz::{SszList as _, SszListMut as _};
    let path = root().join("minimal/eip8321/operations/randao/pyspec_tests/hash_chain_reveal");
    let case = Case::from(path.to_str().context("fixture path")?);
    let mut state = types::eip8321::beacon_state::BeaconState::<Minimal>::from_ssz_default(
        case.bytes("pre.ssz_snappy"),
    )?;
    let mut block = types::eip8321::containers::BeaconBlock::<Minimal>::from_ssz_default(
        case.bytes("block.ssz_snappy"),
    )?;
    let config = Minimal::default_config();
    let cache = PubkeyCache::default();
    let index = helper_functions::accessors::get_beacon_proposer_index(&config, &state)?;
    let seed = types::phase0::primitives::H256::repeat_byte(42);
    let link = |value: types::phase0::primitives::H256| {
        let mut hasher = blake3::Hasher::new();
        hasher.update(types::eip8321::consts::HASH_CHAIN_RANDAO_DST);
        hasher.update(value.as_bytes());
        types::phase0::primitives::H256(*hasher.finalize().as_bytes())
    };
    let reveal = link(seed);
    *state.randao_commitments.get_mut(index)? = link(reveal);
    block.body.hash_chain_reveal = reveal;
    eip8321::process_randao(&config, &cache, &mut state, &block.body, NullVerifier)?;
    ensure!(
        *state.randao_commitments.get(index)? == reveal,
        "first proposal did not consume a link"
    );
    block.body.hash_chain_reveal = seed;
    eip8321::process_randao(&config, &cache, &mut state, &block.body, NullVerifier)?;
    ensure!(
        *state.randao_commitments.get(index)? == seed,
        "second proposal in same epoch did not consume a link"
    );
    let before = state.hash_tree_root();
    ensure!(
        eip8321::process_randao(&config, &cache, &mut state, &block.body, NullVerifier).is_err(),
        "reveal replay accepted"
    );
    ensure!(
        state.hash_tree_root() == before,
        "invalid reveal changed state"
    );
    Ok(())
}

#[test]
fn eip8321_registration_budget_and_same_block_duplicate() -> Result<()> {
    use try_from_iterator::TryFromIterator as _;
    let path = root()
        .join("minimal/eip8321/operations/randao_commitment_registration/pyspec_tests/success");
    let case = Case::from(path.to_str().context("fixture path")?);
    let original = types::eip8321::beacon_state::BeaconState::<Minimal>::from_ssz_default(
        case.bytes("pre.ssz_snappy"),
    )?;
    let registration =
        types::eip8321::containers::SignedRandaoCommitmentRegistration::from_ssz_default(
            case.bytes("randao_commitment_registration.ssz_snappy"),
        )?;
    let config = Minimal::default_config();
    let cache = PubkeyCache::default();
    let mut body = types::eip8321::containers::BeaconBlockBody::<Minimal> {
        randao_commitment_registrations: ssz::ProgressiveList::try_from_iter(
            core::iter::repeat_n(registration, 129),
        )?,
        ..Default::default()
    };
    let mut state = original;
    let before = state.hash_tree_root();
    let error = eip8321::process_operations(
        &config,
        &cache,
        &mut state,
        &body,
        0,
        SingleVerifier,
        NullSlotReport,
    )
    .expect_err("129 registrations accepted");
    ensure!(
        error
            .to_string()
            .contains("RANDAO commitment registrations"),
        "failed for unrelated reason: {error}"
    );
    ensure!(
        state.hash_tree_root() == before,
        "over-budget block applied registrations"
    );
    body.randao_commitment_registrations =
        ssz::ProgressiveList::try_from_iter([registration, registration])?;
    ensure!(
        eip8321::process_operations(
            &config,
            &cache,
            &mut state,
            &body,
            0,
            SingleVerifier,
            NullSlotReport
        )
        .is_err(),
        "same-block duplicate accepted"
    );
    Ok(())
}

#[test]
fn eip8321_combined_slots_upgrade_at_epoch_boundary() -> Result<()> {
    use types::traits::BeaconState as _;
    let path = root().join("minimal/eip8321/fork/fork/pyspec_tests/fork_base_state");
    let case = Case::from(path.to_str().context("fixture path")?);
    let pre =
        BeaconState::<Minimal>::from_ssz_at_phase(Phase::Heze, &case.bytes("pre.ssz_snappy"))?;
    let config = Minimal::default_config().start_and_stay_in(Phase::Heze);
    let cache = PubkeyCache::default();
    let target = helper_functions::misc::compute_start_slot_at_epoch::<Minimal>(1);
    let mut expected = pre.clone();
    combined::process_slots(&config, &cache, &mut expected, target)?;
    let BeaconState::Heze(expected) = expected else {
        anyhow::bail!("wrong pre-fork phase")
    };
    let mut fork_config = config;
    fork_config.eip8321_fork_epoch = 1;
    let expected = helper_functions::fork::upgrade_to_eip8321(&fork_config, expected.into_inner())?;
    let mut actual = pre;
    combined::process_slots(&fork_config, &cache, &mut actual, target)?;
    ensure!(
        actual.phase() == Phase::Eip8321,
        "combined dispatcher did not upgrade at fork slot"
    );
    ensure!(actual.slot() == target, "wrong upgrade slot");
    ensure!(
        actual.hash_tree_root() == expected.hash_tree_root(),
        "fork boundary changed pre-fork epoch processing"
    );
    Ok(())
}
