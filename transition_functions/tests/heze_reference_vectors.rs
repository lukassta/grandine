//! Heze test-first baseline, using only the combined fork dispatcher.
//! Missing Heze support is a failure, including for negative block vectors.
#![expect(
    unused_crate_dependencies,
    reason = "This integration target uses a subset of the production crate dependencies"
)]

use anyhow::{Context as _, Result, ensure};
use core::{panic::AssertUnwindSafe, str::FromStr as _};
use execution_engine::NullExecutionEngine;
use helper_functions::{
    slot_report::NullSlotReport,
    verifier::{NullVerifier, SingleVerifier},
};
use pubkey_cache::PubkeyCache;
use spec_test_utils::{BlsSetting, Case};
use ssz::{SszHash as _, SszList as _, SszReadDefault as _};
use std::fs;

use std::{
    io::Write as _,
    path::{Path, PathBuf},
};
use transition_functions::{
    combined,
    unphased::{ProcessSlots, StateRootPolicy},
};
use types::{
    combined::{BeaconState, SignedBeaconBlock},
    nonstandard::Phase,
    preset::{Mainnet, Minimal, Preset},
    traits::{BeaconState as _, SszValidatorList as _},
};

fn root() -> PathBuf {
    std::env::var_os("HEZE_VECTORS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors/reference"),
        PathBuf::from,
    )
}
fn cases(preset: &str, kind: &str) -> Result<Vec<PathBuf>> {
    fn visit(path: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
        if path.join("pre.ssz_snappy").is_file() {
            paths.push(path.to_path_buf());
        } else {
            for entry in fs::read_dir(path)? {
                let p = entry?.path();
                if p.is_dir() {
                    visit(&p, paths)?;
                }
            }
        }
        Ok(())
    }
    let category = match kind {
        "execution_payload_bid" => "heze/operations",
        "pending_deposits" => "heze/epoch_processing",
        "fork" => "heze/fork",
        _ => "heze/sanity",
    };
    let dir = root().join(preset).join(category).join(kind);
    ensure!(dir.is_dir(), "MISSING_FIXTURES: {}", dir.display());
    let mut paths = vec![];
    visit(&dir, &mut paths)?;
    paths.sort();
    ensure!(!paths.is_empty(), "zero Heze vector cases selected");
    Ok(paths)
}
fn bytes(case: Case<'_>, name: &str) -> Vec<u8> {
    case.bytes(format!("{name}.ssz_snappy"))
}
fn execute<P: Preset>(case: Case<'_>, kind: &str) -> Result<()> {
    let phase = Phase::from_str("heze").context("Heze phase support")?;
    execute_at_phase::<P>(case, kind, phase)
}
fn execute_at_phase<P: Preset>(case: Case<'_>, kind: &str, phase: Phase) -> Result<()> {
    let pre = bytes(case, "pre");
    let pre_phase = if kind == "fork" { Phase::Gloas } else { phase };
    let mut state =
        BeaconState::<P>::from_ssz_at_phase(pre_phase, &pre).context("pre-state SSZ")?;
    ensure!(
        state.phase() == pre_phase,
        "decoded state has the wrong fork"
    );
    let config = P::default_config().upgrade_once(phase, 0);
    let cache = PubkeyCache::default();
    let outcome = if kind == "fork" {
        let BeaconState::Gloas(pre) = &state else {
            anyhow::bail!("expected Gloas before Heze fork")
        };
        let post = helper_functions::fork::upgrade_to_heze(&config, pre.as_ref().clone())?;
        state = post.into();
        Ok(())
    } else if kind == "pending_deposits" {
        match &mut state {
            BeaconState::Heze(pre) => {
                transition_functions::heze::process_pending_deposits(&config, &cache, pre)
            }
            BeaconState::Gloas(pre) => {
                transition_functions::gloas::process_pending_deposits(&config, &cache, pre)
            }
            _ => anyhow::bail!("unexpected phase for pending deposit vector"),
        }
    } else if kind == "execution_payload_bid" {
        let bid = types::heze::containers::SignedExecutionPayloadBid::<P>::from_ssz_default(
            bytes(case, "execution_payload_bid"),
        )?;
        ensure!(
            bid.message.inclusion_list_bits.count_ones() > 0,
            "expected nonzero FOCIL bits"
        );
        let BeaconState::Heze(pre) = &mut state else {
            anyhow::bail!("expected Heze state")
        };
        transition_functions::heze::process_execution_payload_bid(&config, &cache, pre, &bid)
    } else if kind == "slots" {
        let target = state
            .slot()
            .checked_add(case.yaml::<u64>("slots"))
            .context("slot overflow")?;
        combined::process_slots(&config, &cache, &mut state, target)
    } else {
        let meta = case.meta();
        ensure!(meta.blocks_count > 0, "no blocks in block vector");
        (|| -> Result<()> {
            for i in 0..meta.blocks_count {
                let block = SignedBeaconBlock::from_ssz_at_phase(
                    phase,
                    &bytes(case, &format!("blocks_{i}")),
                )?;
                if matches!(meta.bls_setting, BlsSetting::Ignored) {
                    combined::custom_state_transition(
                        &config,
                        &cache,
                        &mut state,
                        &block,
                        ProcessSlots::Always,
                        StateRootPolicy::Verify,
                        NullExecutionEngine,
                        NullVerifier,
                        NullSlotReport,
                    )?;
                } else {
                    combined::custom_state_transition(
                        &config,
                        &cache,
                        &mut state,
                        &block,
                        ProcessSlots::Always,
                        StateRootPolicy::Verify,
                        NullExecutionEngine,
                        SingleVerifier,
                        NullSlotReport,
                    )?;
                }
            }
            Ok(())
        })()
    };
    if case.exists("post") {
        outcome?;
        let post = BeaconState::<P>::from_ssz_at_phase(phase, &bytes(case, "post"))?;
        ensure!(
            state.hash_tree_root() == post.hash_tree_root(),
            "Heze post-state root mismatch"
        );
    } else {
        ensure!(outcome.is_err(), "invalid Heze vector accepted");
    }
    Ok(())
}
fn run<P: Preset>(preset: &str, kind: &str) -> Result<()> {
    let paths = cases(preset, kind)?;
    let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors/reports");
    fs::create_dir_all(&reports)?;
    let mut report = fs::File::create(reports.join(format!("heze-{preset}-{kind}.tsv")))?;
    writeln!(report, "status\tcase\tdetail")?;
    let (mut passed, mut failed) = (0usize, 0usize);
    for path in &paths {
        let case = Case::from(path.to_str().context("UTF-8 fixture path")?);
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| execute::<P>(case, kind)))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("PANIC during Heze vector execution")));
        let relative = path.strip_prefix(root())?;
        match result {
            Ok(()) => {
                passed = passed.checked_add(1).context("case count overflow")?;
                writeln!(report, "PASS\t{}\t", relative.display())?;
            }
            Err(e) => {
                failed = failed.checked_add(1).context("case count overflow")?;
                writeln!(report, "FAIL\t{}\t{e:#}", relative.display())?;
                writeln!(
                    std::io::stderr().lock(),
                    "FAIL {}: {e:#}",
                    relative.display()
                )?;
            }
        }
    }
    writeln!(
        std::io::stderr().lock(),
        "Heze {preset}/{kind}: {passed} passed, {failed} failed"
    )?;
    ensure!(
        failed == 0,
        "{failed}/{} Heze {preset}/{kind} cases failed; see test-vectors/reports",
        paths.len()
    );
    Ok(())
}
fn inventory(preset: &str) -> Result<()> {
    let mut count = 0usize;
    for kind in ["slots", "blocks"] {
        for path in cases(preset, kind)? {
            let case = Case::from(path.to_str().context("UTF-8 fixture path")?);
            ensure!(!bytes(case, "pre").is_empty(), "empty pre-state");
            if case.exists("post") {
                ensure!(!bytes(case, "post").is_empty(), "empty post-state");
            }
            if kind == "blocks" {
                let meta = case.meta();
                ensure!(meta.blocks_count > 0, "no blocks");
                for i in 0..meta.blocks_count {
                    ensure!(
                        !bytes(case, &format!("blocks_{i}")).is_empty(),
                        "empty block"
                    );
                }
            } else {
                ensure!(case.yaml::<u64>("slots") > 0, "nonpositive slot count");
            }
            count = count.checked_add(1).context("case count overflow")?;
        }
    }
    ensure!(
        count == 9,
        "expected 9 focused {preset} cases, found {count}"
    );
    writeln!(
        std::io::stderr().lock(),
        "Heze {preset}: {count} fixture cases present and Snappy data readable"
    )?;
    Ok(())
}
#[test]
fn heze_minimal_fixture_inventory() -> Result<()> {
    inventory("minimal")
}
#[test]
fn heze_mainnet_fixture_inventory() -> Result<()> {
    inventory("mainnet")
}
#[test]
fn heze_minimal_slots() -> Result<()> {
    run::<Minimal>("minimal", "slots")
}
#[test]
fn heze_mainnet_slots() -> Result<()> {
    run::<Mainnet>("mainnet", "slots")
}
#[test]
fn heze_minimal_blocks() -> Result<()> {
    run::<Minimal>("minimal", "blocks")
}
#[test]
fn heze_mainnet_blocks() -> Result<()> {
    run::<Mainnet>("mainnet", "blocks")
}

#[test]
fn heze_minimal_focil_bid() -> Result<()> {
    run::<Minimal>("minimal", "execution_payload_bid")
}
#[test]
fn heze_mainnet_focil_bid() -> Result<()> {
    run::<Mainnet>("mainnet", "execution_payload_bid")
}

#[test]
fn heze_focil_bid_bits_are_covered_by_builder_signature() -> Result<()> {
    let paths = cases("minimal", "execution_payload_bid")?;
    let path = paths.first().context("missing FOCIL builder-bid case")?;
    let case = Case::from(path.to_str().context("fixture path")?);
    let mut state =
        types::heze::beacon_state::BeaconState::<Minimal>::from_ssz_default(bytes(case, "pre"))?;
    let mut bid = types::heze::containers::SignedExecutionPayloadBid::<Minimal>::from_ssz_default(
        bytes(case, "execution_payload_bid"),
    )?;
    let before = state.hash_tree_root();
    bid.message
        .inclusion_list_bits
        .set(1, !bid.message.inclusion_list_bits[1]);
    let config = Minimal::default_config().upgrade_once(Phase::Heze, 0);
    let outcome = transition_functions::heze::process_execution_payload_bid(
        &config,
        &PubkeyCache::default(),
        &mut state,
        &bid,
    );
    ensure!(
        outcome.is_err(),
        "changing inclusion-list bits must invalidate builder signature"
    );
    ensure!(
        state.hash_tree_root() == before,
        "invalid bid changed consensus state"
    );
    Ok(())
}

#[test]
fn heze_upgrade_preserves_gloas_and_initializes_focil_bits() -> Result<()> {
    use types::gloas::beacon_state::BeaconState as GloasState;
    let mut pre = GloasState::<Minimal>::default();
    pre.fork.current_version = types::phase0::primitives::H32([7, 0, 0, 1]);
    pre.latest_execution_payload_bid.block_hash = types::phase0::primitives::H256::repeat_byte(42);
    let before = pre.hash_tree_root();
    let config = Minimal::default_config();
    let post = helper_functions::fork::upgrade_to_heze(&config, pre.clone())?;
    ensure!(
        post.fork.previous_version == pre.fork.current_version,
        "wrong previous fork version"
    );
    ensure!(
        post.fork.current_version == config.heze_fork_version,
        "wrong Heze fork version"
    );
    ensure!(
        post.latest_execution_payload_bid.block_hash == pre.latest_execution_payload_bid.block_hash,
        "upgrade lost payload bid"
    );
    ensure!(
        post.latest_execution_payload_bid
            .inclusion_list_bits
            .count_ones()
            == 0,
        "fork must initialize all inclusion-list bits to false"
    );
    ensure!(
        pre.hash_tree_root() == before,
        "upgrade altered the Gloas input"
    );
    pre.eth1_deposit_index = 1;
    ensure!(
        helper_functions::fork::upgrade_to_heze(&config, pre).is_err(),
        "Heze upgrade must reject unprocessed legacy deposits"
    );
    Ok(())
}

#[test]
fn heze_preserves_gloas_reference_transitions() -> Result<()> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test-vectors/reference/minimal/gloas/sanity");
    for (kind, names) in [
        ("slots", &["slots_1", "empty_epoch"][..]),
        (
            "blocks",
            &[
                "empty_block_transition",
                "invalid_incorrect_state_root",
                "invalid_all_zeroed_sig",
            ][..],
        ),
    ] {
        for name in names {
            let path = dir.join(kind).join("pyspec_tests").join(name);
            let case = Case::from(path.to_str().context("Gloas fixture path")?);
            execute_at_phase::<Minimal>(case, kind, Phase::Gloas)?;
        }
    }
    Ok(())
}

#[test]
fn heze_minimal_pending_deposits() -> Result<()> {
    run::<Minimal>("minimal", "pending_deposits")
}
#[test]
fn heze_mainnet_pending_deposits() -> Result<()> {
    run::<Mainnet>("mainnet", "pending_deposits")
}
#[test]
fn heze_minimal_fork() -> Result<()> {
    run::<Minimal>("minimal", "fork")
}
#[test]
fn heze_mainnet_fork() -> Result<()> {
    run::<Mainnet>("mainnet", "fork")
}
#[test]
fn heze_preserves_gloas_bls_validator_registration() -> Result<()> {
    let path = root().join(
        "minimal/gloas/epoch_processing/pending_deposits/pyspec_tests/apply_pending_deposit_min_activation");
    let case = Case::from(path.to_str().context("Gloas fixture path")?);
    let pre =
        types::gloas::beacon_state::BeaconState::<Minimal>::from_ssz_default(bytes(case, "pre"))?;
    let pending = pre.pending_deposits.get(0)?;
    ensure!(
        pending.withdrawal_credentials[0] == 0,
        "regression must exercise a new BLS validator"
    );
    let post =
        types::gloas::beacon_state::BeaconState::<Minimal>::from_ssz_default(bytes(case, "post"))?;
    ensure!(
        post.validators.len_u64() == pre.validators.len_u64() + 1,
        "Gloas must still register the validator"
    );
    execute_at_phase::<Minimal>(case, "pending_deposits", Phase::Gloas)
}
