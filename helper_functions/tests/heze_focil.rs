#![expect(
    unused_crate_dependencies,
    reason = "Integration tests exercise only FOCIL"
)]
use helper_functions::heze::{
    InclusionListStore, inclusion_list_committee_from_indices, validate_inclusion_list_bounds,
};
use try_from_iterator::TryFromIterator as _;
use types::{
    heze::containers::{InclusionList, SignedInclusionList},
    phase0::primitives::H256,
    preset::Minimal,
};

fn list(validator: u64, transactions: &[&[u8]]) -> SignedInclusionList<Minimal> {
    SignedInclusionList {
        message: InclusionList {
            slot: 10,
            validator_index: validator,
            dependent_root: H256::repeat_byte(1),
            transactions: ssz::ProgressiveList::try_from_iter(
                transactions
                    .iter()
                    .map(|bytes| bytes.to_vec().try_into().expect("valid FOCIL test fixture")),
            )
            .expect("valid FOCIL test fixture"),
        },
        ..Default::default()
    }
}

#[test]
fn heze_focil_committee_wraps_in_order_and_rejects_empty() {
    inclusion_list_committee_from_indices(&[]).expect_err("empty committees must be rejected");
    let committee =
        inclusion_list_committee_from_indices(&[4, 2, 9]).expect("valid FOCIL test fixture");
    assert_eq!(
        committee.iter().copied().collect::<Vec<_>>(),
        vec![4, 2, 9, 4, 2, 9, 4, 2, 9, 4, 2, 9, 4, 2, 9, 4]
    );
}

#[test]
fn heze_focil_deduplicates_and_filters_timeliness() {
    let mut store = InclusionListStore::default();
    store.process_inclusion_list(list(1, &[&[1], &[2], &[1]]), true);
    store.process_inclusion_list(list(2, &[&[2], &[3]]), false);
    let root = H256::repeat_byte(1);
    let bytes = |timely| {
        store
            .get_inclusion_list_transactions(10, root, timely)
            .iter()
            .map(|tx| tx.as_bytes().to_vec())
            .collect::<Vec<_>>()
    };
    assert_eq!(bytes(true), vec![vec![1], vec![2]]);
    assert_eq!(bytes(false), vec![vec![1], vec![2], vec![3]]);
    assert!(
        store
            .get_inclusion_list_transactions(11, root, false)
            .is_empty()
    );
    assert!(
        store
            .get_inclusion_list_transactions(10, H256::zero(), false)
            .is_empty()
    );
}

#[test]
fn heze_focil_duplicate_does_not_upgrade_timeliness() {
    let mut store = InclusionListStore::default();
    let signed = list(1, &[&[1]]);
    store.process_inclusion_list(signed.clone(), false);
    store.process_inclusion_list(signed, true);
    assert!(
        store
            .get_inclusion_list_transactions(10, H256::repeat_byte(1), true)
            .is_empty()
    );
}

#[test]
fn heze_focil_late_equivocation_removes_timely_list() {
    let mut store = InclusionListStore::default();
    store.process_inclusion_list(list(1, &[&[1]]), true);
    store.process_inclusion_list(list(2, &[&[2]]), true);
    store.process_inclusion_list(list(1, &[&[9]]), false);
    let txs = store.get_inclusion_list_transactions(10, H256::repeat_byte(1), true);
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0].as_bytes(), &[2]);
    // An equivocator can participate in another slot.
    let mut next = list(1, &[&[3]]);
    next.message.slot = 11;
    store.process_inclusion_list(next, true);
    assert_eq!(
        store
            .get_inclusion_list_transactions(11, H256::repeat_byte(1), true)
            .len(),
        1
    );
}

#[test]
fn heze_focil_bits_include_every_repeated_committee_position() {
    let committee =
        inclusion_list_committee_from_indices(&[1, 2]).expect("valid FOCIL test fixture");
    let mut store = InclusionListStore::default();
    store.process_inclusion_list(list(1, &[&[1]]), true);
    let root = H256::repeat_byte(1);
    let mut bits = store.get_inclusion_list_bits(&committee, 10, root, true);
    for index in 0..16 {
        assert_eq!(bits[index], index % 2 == 0);
    }
    assert!(store.is_inclusion_list_bits_inclusive(&committee, 10, root, bits, true));
    bits.set(0, false);
    assert!(!store.is_inclusion_list_bits_inclusive(&committee, 10, root, bits, true));
    bits.set(0, true);
    bits.set(1, true); // Extra advertised bits are allowed.
    assert!(store.is_inclusion_list_bits_inclusive(&committee, 10, root, bits, true));
}

#[test]
fn heze_focil_retains_boundary_and_equivocation_records() {
    let mut store = InclusionListStore::default();
    store.process_inclusion_list(list(1, &[&[1]]), true);
    store.process_inclusion_list(list(2, &[&[2]]), true);
    store.process_inclusion_list(list(2, &[&[3]]), false);
    store.prune(11, 1);
    assert_eq!(
        store
            .get_inclusion_list_transactions(10, H256::repeat_byte(1), false)
            .len(),
        1
    );
    store.prune(12, 1);
    assert!(
        store
            .get_inclusion_list_transactions(10, H256::repeat_byte(1), false)
            .is_empty()
    );
}

#[test]
fn heze_focil_bounds_reject_empty_future_expired_and_oversized_lists() {
    let valid = list(1, &[&[1, 2], &[3]]);
    validate_inclusion_list_bounds(&valid.message, 11, 1, 3)
        .expect("valid list at retention boundary");
    assert!(validate_inclusion_list_bounds(&valid.message, 9, 1, 3).is_err());
    assert!(validate_inclusion_list_bounds(&valid.message, 12, 1, 3).is_err());
    assert!(validate_inclusion_list_bounds(&valid.message, 10, 1, 2).is_err());
    assert!(validate_inclusion_list_bounds(&list(1, &[]).message, 10, 1, 3).is_err());
    assert!(validate_inclusion_list_bounds(&list(1, &[&[]]).message, 10, 1, 3).is_err());
    assert!(validate_inclusion_list_bounds(&list(1, &[&[1], &[]]).message, 10, 1, 3).is_err());
}

#[test]
fn heze_focil_signature_binds_message_fork_and_genesis_root() {
    use bls::traits::SecretKey as _;
    use helper_functions::heze::{inclusion_list_signing_root, is_valid_inclusion_list_signature};
    use types::traits::SszValidatorListMut as _;
    use types::{nonstandard::Phase, phase0::containers::Validator, preset::Preset};

    let key = bls::SecretKey::try_from(bls::SecretKeyBytes::from([1_u8; 32]))
        .expect("valid FOCIL test fixture");
    let mut state = types::heze::beacon_state::BeaconState::<Minimal> {
        genesis_validators_root: H256::repeat_byte(42),
        ..Default::default()
    };
    state
        .validators
        .push(Validator {
            pubkey: key.to_public_key().into(),
            ..Default::default()
        })
        .expect("valid FOCIL test fixture");
    let config = Minimal::default_config().upgrade_once(Phase::Heze, 0);
    let mut signed = list(0, &[&[1, 2, 3]]);
    let epoch = helper_functions::misc::compute_epoch_at_slot::<Minimal>(signed.message.slot);
    let root = inclusion_list_signing_root(
        &config,
        &signed.message,
        config.version_at_epoch(epoch),
        state.genesis_validators_root,
    );
    signed.signature = key.sign(root).into();
    assert!(is_valid_inclusion_list_signature(&config, &state, &signed));

    let mut other_chain = state.clone();
    other_chain.genesis_validators_root = H256::repeat_byte(43);
    assert!(!is_valid_inclusion_list_signature(
        &config,
        &other_chain,
        &signed
    ));
    let mut other_fork = Minimal::default_config().upgrade_once(Phase::Heze, 0);
    other_fork.heze_fork_version = types::phase0::primitives::H32([9, 0, 0, 1]);
    assert!(!is_valid_inclusion_list_signature(
        &other_fork,
        &state,
        &signed
    ));
    signed.message.transactions = list(0, &[&[9]]).message.transactions;
    assert!(!is_valid_inclusion_list_signature(&config, &state, &signed));
    signed.message.validator_index = u64::MAX;
    assert!(!is_valid_inclusion_list_signature(&config, &state, &signed));
}

#[test]
fn heze_focil_committee_and_ssz_match_pinned_python_spec() {
    use spec_test_utils::Case;
    use ssz::{SszHash as _, SszReadDefault as _};
    #[derive(serde::Deserialize)]
    struct CommitteeCase {
        slot: u64,
        committee: Vec<u64>,
    }
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-vectors");
    let pre_dir = fixtures.join("heze/minimal/heze/sanity/slots/pyspec_tests/slots_1");
    let pre = Case::from(pre_dir.to_str().expect("valid FOCIL test fixture"));
    let state = types::heze::beacon_state::BeaconState::<Minimal>::from_ssz_default(
        pre.bytes("pre.ssz_snappy"),
    )
    .expect("valid FOCIL test fixture");
    let oracle_dir = fixtures.join("focil-oracle");
    let oracle = Case::from(oracle_dir.to_str().expect("valid FOCIL test fixture"));
    for case in oracle.yaml::<Vec<CommitteeCase>>("committees") {
        let committee = helper_functions::heze::get_inclusion_list_committee(&state, case.slot)
            .expect("valid FOCIL test fixture");
        assert_eq!(
            committee.iter().copied().collect::<Vec<_>>(),
            case.committee
        );
    }
    let message =
        InclusionList::<Minimal>::from_ssz_default(oracle.bytes("inclusion_list.ssz_snappy"))
            .expect("valid FOCIL test fixture");
    assert_eq!(message.hash_tree_root(), oracle.yaml::<H256>("root"));
}
