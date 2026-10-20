#![expect(
    unused_crate_dependencies,
    reason = "Focused FOCIL consensus integration tests"
)]
use anyhow::Result;
use fork_choice_store::{Storage, Store, StoreConfig};
use pubkey_cache::PubkeyCache;
use ssz::{Hc, SszHash as _};
use std::sync::Arc;
use types::{
    combined::{BeaconState, SignedBeaconBlock},
    gloas::containers::{ExecutionPayloadEnvelope, SignedExecutionPayloadEnvelope},
    nonstandard::{Phase, StorageMode},
    phase0::{containers::BeaconBlockHeader, primitives::H256},
    preset::{Minimal, Preset},
    traits::{SignedBeaconBlock as _, SszValidatorList},
};

struct MemoryStorage;
impl Storage<Minimal> for MemoryStorage {
    fn storage_mode(&self) -> StorageMode {
        StorageMode::Prune
    }
    fn stored_state_by_block_root(
        &self,
        _: H256,
        _: Option<&dyn SszValidatorList>,
    ) -> Result<Option<Arc<BeaconState<Minimal>>>> {
        Ok(None)
    }
}

#[expect(
    clippy::default_trait_access,
    reason = "Each concrete block infers its signature type"
)]
fn setup(phase: Phase) -> (Store<Minimal, MemoryStorage>, H256) {
    let state: BeaconState<Minimal>;
    let block: SignedBeaconBlock<Minimal>;
    if phase == Phase::Heze {
        let body = types::heze::containers::BeaconBlockBody::<Minimal>::default();
        let raw = types::heze::beacon_state::BeaconState {
            slot: 8,
            latest_block_header: BeaconBlockHeader {
                slot: 8,
                body_root: body.hash_tree_root(),
                ..Default::default()
            },
            ..Default::default()
        };
        let root = raw.hash_tree_root();
        state = raw.into();
        block = SignedBeaconBlock::Heze(types::heze::containers::SignedBeaconBlock {
            message: Hc::new(types::heze::containers::BeaconBlock {
                slot: 8,
                state_root: root,
                body,
                ..Default::default()
            }),
            signature: Default::default(),
        });
    } else {
        let body = types::gloas::containers::BeaconBlockBody::<Minimal>::default();
        let raw = types::gloas::beacon_state::BeaconState {
            slot: 8,
            latest_block_header: BeaconBlockHeader {
                slot: 8,
                body_root: body.hash_tree_root(),
                ..Default::default()
            },
            ..Default::default()
        };
        let root = raw.hash_tree_root();
        state = raw.into();
        block = SignedBeaconBlock::Gloas(types::gloas::containers::SignedBeaconBlock {
            message: Hc::new(types::gloas::containers::BeaconBlock {
                slot: 8,
                state_root: root,
                body,
                ..Default::default()
            }),
            signature: Default::default(),
        });
    }
    let root = block.message().hash_tree_root();
    let store = Store::new(
        Arc::new(Minimal::default_config().upgrade_once(phase, 0)),
        Arc::new(PubkeyCache::default()),
        StoreConfig::default(),
        Arc::new(block),
        Arc::new(state),
        Arc::new(MemoryStorage),
        true,
        true,
        std::collections::HashSet::new(),
        Arc::default(),
    );
    (store, root)
}
fn available(store: &mut Store<Minimal, MemoryStorage>, root: H256) {
    let mut envelope = SignedExecutionPayloadEnvelope {
        message: ExecutionPayloadEnvelope {
            beacon_block_root: root,
            ..Default::default()
        },
        ..Default::default()
    };
    envelope.message.payload.slot_number = 8;
    store.apply_execution_payload_envelope(Arc::new(envelope), true);
}

#[test]
fn heze_focil_unsatisfied_payload_remains_valid_but_is_not_extended() {
    let (mut store, root) = setup(Phase::Heze);
    available(&mut store, root);
    store
        .record_payload_inclusion_list_satisfaction(root, Some(false))
        .expect("valid FOCIL test fixture");
    assert!(store.is_payload_verified(root));
    assert!(!store.is_payload_invalid(root));
    assert!(!store.is_payload_inclusion_list_satisfied(root));
    assert!(!store.should_extend_payload(root));
    // A validated result must remain stable if lists are subsequently pruned.
    store
        .record_payload_inclusion_list_satisfaction(root, Some(true))
        .expect("valid FOCIL test fixture");
    assert!(!store.is_payload_inclusion_list_satisfied(root));
    let (mut satisfied_store, satisfied_root) = setup(Phase::Heze);
    available(&mut satisfied_store, satisfied_root);
    satisfied_store
        .record_payload_inclusion_list_satisfaction(satisfied_root, Some(true))
        .expect("valid FOCIL test fixture");
    assert!(satisfied_store.should_extend_payload(satisfied_root));
}

#[test]
fn heze_focil_optimistic_result_is_replaced_by_validated_result() {
    let (mut store, root) = setup(Phase::Heze);
    store
        .record_payload_inclusion_list_satisfaction(root, None)
        .expect("valid FOCIL test fixture");
    assert!(!store.is_payload_inclusion_list_satisfied(root)); // Not locally delivered.
    available(&mut store, root);
    assert!(store.is_payload_inclusion_list_satisfied(root));
    store
        .record_payload_inclusion_list_satisfaction(root, Some(false))
        .expect("valid FOCIL test fixture");
    assert!(!store.should_extend_payload(root));
    store
        .record_payload_inclusion_list_satisfaction(root, None)
        .expect("valid FOCIL test fixture");
    assert!(!store.should_extend_payload(root)); // No optimistic downgrade of a known result.
    assert!(
        store
            .record_payload_inclusion_list_satisfaction(H256::repeat_byte(99), Some(true))
            .is_err()
    );
}

#[test]
fn heze_focil_does_not_change_gloas_payload_extension() {
    let (mut store, root) = setup(Phase::Gloas);
    available(&mut store, root);
    assert!(store.should_extend_payload(root));
    assert!(
        store
            .record_payload_inclusion_list_satisfaction(root, Some(false))
            .is_err()
    );
    assert!(store.should_extend_payload(root));
}

#[test]
fn heze_focil_invalid_inclusion_list_does_not_mutate_store() {
    let (mut store, _) = setup(Phase::Heze);
    let invalid = types::heze::containers::SignedInclusionList::<Minimal>::default();
    assert!(store.on_inclusion_list(invalid, 0).is_err());
    assert!(
        store
            .inclusion_list_store()
            .get_inclusion_list_transactions(0, H256::zero(), false)
            .is_empty()
    );
}
