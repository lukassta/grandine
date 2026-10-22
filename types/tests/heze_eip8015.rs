#![expect(unused_crate_dependencies, reason = "Focused Heze schema regressions")]
use types::{
    combined::BeaconState,
    heze::{beacon_state::BeaconState as HezeState, containers::BeaconBlockBody},
    preset::Minimal,
    traits::{BeaconBlockBody as _, BeaconState as _},
};

#[test]
fn heze_eip8015_state_rejects_retired_fields() -> Result<(), serde_json::Error> {
    let value = serde_json::to_value(HezeState::<Minimal>::default())?;
    for field in [
        "eth1_data",
        "eth1_data_votes",
        "eth1_deposit_index",
        "deposit_requests_start_index",
    ] {
        assert!(
            value.get(field).is_none(),
            "retired field {field} is still serialized"
        );
        let mut legacy = value.clone();
        legacy[field] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<HezeState<Minimal>>(legacy).is_err(),
            "Heze JSON must reject retired field {field}"
        );
    }
    Ok(())
}

#[test]
fn heze_eip8015_body_rejects_retired_fields() -> Result<(), serde_json::Error> {
    let value = serde_json::to_value(BeaconBlockBody::<Minimal>::default())?;
    for field in ["eth1_data", "deposits"] {
        assert!(
            value.get(field).is_none(),
            "retired field {field} is still serialized"
        );
        let mut legacy = value.clone();
        legacy[field] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<BeaconBlockBody<Minimal>>(legacy).is_err(),
            "Heze JSON must reject retired field {field}"
        );
    }
    Ok(())
}

#[test]
fn heze_eip8015_legacy_capabilities_end_at_gloas() {
    let mut state: BeaconState<Minimal> = HezeState::default().into();
    assert!(state.pre_heze().is_none());
    assert!(state.pre_heze_mut().is_none());
    assert!(state.deposit_requests_start_index().is_none());
    assert!(BeaconBlockBody::<Minimal>::default().pre_heze().is_none());
    let state: BeaconState<Minimal> = types::gloas::beacon_state::BeaconState::default().into();
    assert!(state.pre_heze().is_some());
    assert!(state.deposit_requests_start_index().is_some());
}
