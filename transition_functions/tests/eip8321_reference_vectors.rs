//! Official vectors from consensus-specs 12c8b24, not modified expectations.
#![expect(unused_crate_dependencies, reason = "Focused integration target")]
use anyhow::{Context as _, Result, ensure};
use core::panic::AssertUnwindSafe;
use execution_engine::NullExecutionEngine;
use helper_functions::{
    slot_report::NullSlotReport,
    verifier::{NullVerifier, SingleVerifier},
};
use pubkey_cache::PubkeyCache;
use spec_test_utils::{BlsSetting, Case};
use ssz::{SszHash as _, SszReadDefault as _};
use std::fs;
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};
use transition_functions::{
    combined, eip8321,
    unphased::{ProcessSlots, StateRootPolicy},
};
use types::{
    combined::{BeaconState, SignedBeaconBlock},
    nonstandard::Phase,
    preset::{Mainnet, Minimal, Preset},
};

fn root() -> PathBuf {
    std::env::var_os("EIP8321_VECTORS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors/reference"),
        PathBuf::from,
    )
}
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
fn run<P: Preset>(preset: &str, category: &str) -> Result<()> {
    let dir = root().join(preset).join("eip8321").join(category);
    ensure!(dir.is_dir(), "missing fixtures: {}", dir.display());
    let mut paths = vec![];
    visit(&dir, &mut paths)?;
    paths.sort();
    let expected = [
        (("minimal", "operations/randao"), 8),
        (("minimal", "operations/randao_commitment_registration"), 10),
        (
            ("minimal", "epoch_processing/pending_randao_commitments"),
            5,
        ),
        (("minimal", "epoch_processing/pending_deposits"), 49),
        (("minimal", "sanity/slots"), 17),
        (("minimal", "sanity/blocks"), 91),
        (("minimal", "finality/finality"), 5),
        (("minimal", "fork/fork"), 7),
        (("mainnet", "operations/randao"), 8),
        (("mainnet", "operations/randao_commitment_registration"), 10),
        (
            ("mainnet", "epoch_processing/pending_randao_commitments"),
            5,
        ),
        (("mainnet", "epoch_processing/pending_deposits"), 48),
        (("mainnet", "sanity/slots"), 17),
        (("mainnet", "sanity/blocks"), 77),
        (("mainnet", "finality/finality"), 5),
        (("mainnet", "fork/fork"), 6),
    ]
    .into_iter()
    .find_map(|(key, count)| (key == (preset, category)).then_some(count))
    .context("unlisted vector category")?;
    ensure!(
        paths.len() == expected,
        "fixture count mismatch: expected {expected}, got {}",
        paths.len()
    );
    let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors/reports");
    fs::create_dir_all(&reports)?;
    let mut report = fs::File::create(reports.join(format!(
        "eip8321-{preset}-{}.tsv",
        category.replace('/', "-")
    )))?;
    writeln!(report, "status\tcase\tdetail")?;
    let mut failed: usize = 0;
    for path in &paths {
        let case = Case::from(path.to_str().context("fixture path")?);
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| execute::<P>(case, category)))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("panic while executing vector")));
        match result {
            Ok(()) => writeln!(report, "PASS\t{}\t", path.strip_prefix(root())?.display())?,
            Err(e) => {
                failed = failed.checked_add(1).context("failure count overflow")?;
                writeln!(
                    report,
                    "FAIL\t{}\t{e:#}",
                    path.strip_prefix(root())?.display()
                )?;
                writeln!(std::io::stderr().lock(), "FAIL {}: {e:#}", path.display())?;
            }
        }
    }
    writeln!(
        std::io::stderr().lock(),
        "EIP8321 {preset}/{category}: {} passed, {failed} failed",
        paths
            .len()
            .checked_sub(failed)
            .context("invalid case count")?
    )?;
    ensure!(failed == 0, "{failed}/{} vectors failed", paths.len());
    Ok(())
}
#[expect(
    clippy::too_many_lines,
    reason = "One dispatcher for eight official vector categories"
)]
fn execute<P: Preset>(case: Case<'_>, category: &str) -> Result<()> {
    let config = P::default_config().upgrade_once(Phase::Eip8321, 0);
    let cache = PubkeyCache::default();
    let phase = if category == "fork/fork" {
        Phase::Heze
    } else {
        Phase::Eip8321
    };
    let mut state = BeaconState::<P>::from_ssz_at_phase(phase, &case.bytes("pre.ssz_snappy"))
        .context("pre-state decode")?;
    // Input decode errors are harness failures, including for negative vectors.
    match category {
        "operations/randao" => {
            types::eip8321::containers::BeaconBlock::<P>::from_ssz_default(
                case.bytes("block.ssz_snappy"),
            )?;
        }
        "operations/randao_commitment_registration" => {
            types::eip8321::containers::SignedRandaoCommitmentRegistration::from_ssz_default(
                case.bytes("randao_commitment_registration.ssz_snappy"),
            )?;
        }
        "sanity/blocks" | "finality/finality" => {
            for i in 0..case.meta().blocks_count {
                SignedBeaconBlock::<P>::from_ssz_at_phase(
                    Phase::Eip8321,
                    &case.bytes(format!("blocks_{i}.ssz_snappy")),
                )?;
            }
        }
        _ => {}
    }
    let bls = case.meta().bls_setting;
    let outcome = (|| -> Result<()> {
        match category {
            "fork/fork" => {
                let BeaconState::Heze(pre) = &state else {
                    anyhow::bail!("wrong pre-fork phase")
                };
                state = helper_functions::fork::upgrade_to_eip8321(&config, pre.as_ref().clone())?
                    .into();
            }
            "sanity/slots" => {
                use types::traits::BeaconState as _;
                let target = state
                    .slot()
                    .checked_add(case.yaml::<u64>("slots"))
                    .context("slot overflow")?;
                combined::process_slots(&config, &cache, &mut state, target)?;
            }
            "sanity/blocks" | "finality/finality" => {
                let meta = case.meta();
                ensure!(meta.blocks_count > 0, "zero blocks");
                for i in 0..meta.blocks_count {
                    let block = SignedBeaconBlock::from_ssz_at_phase(
                        Phase::Eip8321,
                        &case.bytes(format!("blocks_{i}.ssz_snappy")),
                    )?;
                    if matches!(bls, BlsSetting::Ignored) {
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
            }
            _ => {
                let BeaconState::Eip8321(pre) = &mut state else {
                    anyhow::bail!("wrong operation phase")
                };
                match category {
                    "operations/randao" => {
                        let block = types::eip8321::containers::BeaconBlock::<P>::from_ssz_default(
                            case.bytes("block.ssz_snappy"),
                        )?;
                        if matches!(bls, BlsSetting::Ignored) {
                            eip8321::process_randao(
                                &config,
                                &cache,
                                pre,
                                &block.body,
                                NullVerifier,
                            )?;
                        } else {
                            eip8321::process_randao(
                                &config,
                                &cache,
                                pre,
                                &block.body,
                                SingleVerifier,
                            )?;
                        }
                    }
                    "operations/randao_commitment_registration" => {
                        let op=types::eip8321::containers::SignedRandaoCommitmentRegistration::from_ssz_default(case.bytes("randao_commitment_registration.ssz_snappy"))?;
                        if matches!(bls, BlsSetting::Ignored) {
                            eip8321::process_randao_commitment_registration(
                                &config,
                                &cache,
                                pre,
                                &op,
                                NullVerifier,
                            )?;
                        } else {
                            eip8321::process_randao_commitment_registration(
                                &config,
                                &cache,
                                pre,
                                &op,
                                SingleVerifier,
                            )?;
                        }
                    }
                    "epoch_processing/pending_randao_commitments" => {
                        eip8321::process_pending_randao_commitments(pre)?
                    }
                    "epoch_processing/pending_deposits" => {
                        eip8321::process_pending_deposits(&config, &cache, pre)?
                    }
                    _ => anyhow::bail!("unhandled category {category}"),
                }
            }
        }
        Ok(())
    })();
    if case.exists("post") {
        outcome.context("valid vector rejected")?;
        let post =
            BeaconState::<P>::from_ssz_at_phase(Phase::Eip8321, &case.bytes("post.ssz_snappy"))?;
        ensure!(
            state.hash_tree_root() == post.hash_tree_root(),
            "post-state root mismatch"
        );
    } else {
        ensure!(outcome.is_err(), "invalid vector accepted");
    }
    Ok(())
}
macro_rules! vectors {
    ($name:ident,$p:ty,$preset:literal,$category:literal) => {
        #[test]
        fn $name() -> Result<()> {
            run::<$p>($preset, $category)
        }
    };
}
vectors!(
    eip8321_minimal_operations_randao,
    Minimal,
    "minimal",
    "operations/randao"
);
vectors!(
    eip8321_minimal_operations_randao_commitment_registration,
    Minimal,
    "minimal",
    "operations/randao_commitment_registration"
);
vectors!(
    eip8321_minimal_epoch_processing_pending_randao_commitments,
    Minimal,
    "minimal",
    "epoch_processing/pending_randao_commitments"
);
vectors!(
    eip8321_minimal_epoch_processing_pending_deposits,
    Minimal,
    "minimal",
    "epoch_processing/pending_deposits"
);
vectors!(
    eip8321_minimal_sanity_slots,
    Minimal,
    "minimal",
    "sanity/slots"
);
vectors!(
    eip8321_minimal_sanity_blocks,
    Minimal,
    "minimal",
    "sanity/blocks"
);
vectors!(
    eip8321_minimal_finality_finality,
    Minimal,
    "minimal",
    "finality/finality"
);
vectors!(eip8321_minimal_fork_fork, Minimal, "minimal", "fork/fork");
vectors!(
    eip8321_mainnet_operations_randao,
    Mainnet,
    "mainnet",
    "operations/randao"
);
vectors!(
    eip8321_mainnet_operations_randao_commitment_registration,
    Mainnet,
    "mainnet",
    "operations/randao_commitment_registration"
);
vectors!(
    eip8321_mainnet_epoch_processing_pending_randao_commitments,
    Mainnet,
    "mainnet",
    "epoch_processing/pending_randao_commitments"
);
vectors!(
    eip8321_mainnet_epoch_processing_pending_deposits,
    Mainnet,
    "mainnet",
    "epoch_processing/pending_deposits"
);
vectors!(
    eip8321_mainnet_sanity_slots,
    Mainnet,
    "mainnet",
    "sanity/slots"
);
vectors!(
    eip8321_mainnet_sanity_blocks,
    Mainnet,
    "mainnet",
    "sanity/blocks"
);
vectors!(
    eip8321_mainnet_finality_finality,
    Mainnet,
    "mainnet",
    "finality/finality"
);
vectors!(eip8321_mainnet_fork_fork, Mainnet, "mainnet", "fork/fork");
