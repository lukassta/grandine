//! Heze test-first baseline, using only the combined fork dispatcher.
//! Missing Heze support is a failure, including for negative block vectors.
#![expect(
    unused_crate_dependencies,
    reason = "This integration target uses a subset of the production crate dependencies"
)]

use anyhow::{Context as _, Result, ensure};
use core::{panic::AssertUnwindSafe, str::FromStr as _};
use execution_engine::NullExecutionEngine;
use fs_err as fs;
use helper_functions::{
    slot_report::NullSlotReport,
    verifier::{NullVerifier, SingleVerifier},
};
use pubkey_cache::PubkeyCache;
use spec_test_utils::{BlsSetting, Case};
use ssz::SszHash as _;

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
    traits::BeaconState as _,
};

fn root() -> PathBuf {
    std::env::var_os("HEZE_VECTORS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors/heze"),
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
    let dir = root().join(preset).join("heze/sanity").join(kind);
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
    let pre = bytes(case, "pre");
    // This checks actual production fork support. There is no Gloas fallback,
    // unconditional failure or should_panic assertion in this test target.
    let phase = Phase::from_str("heze").context(
        "HEZE_NOT_IMPLEMENTED: Grandine Phase has no Heze variant; a separate Heze phase, types and dispatch are required")?;
    let mut state =
        BeaconState::<P>::from_ssz_at_phase(phase, &pre).context("Heze pre-state SSZ")?;
    ensure!(state.phase() == phase, "decoded state has the wrong fork");
    let config = P::default_config().upgrade_once(phase, 0);
    let cache = PubkeyCache::default();
    let outcome = if kind == "slots" {
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
