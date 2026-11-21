//! Reflection retention contracts on the prebuilt, named retention fixtures. The fixtures come
//! from `test-components/size-analysis/fixtures`, which `test-components/build-components.sh rust`
//! builds and copies to `test-components/`.

use golem_size_analysis::contract::{
    GOLEM_GUEST_EXPORTS, check_export_contract, component_contract,
};
use golem_size_analysis::retention::{Evidence, component_evidence, evaluate_retention};
use std::collections::BTreeMap;
use std::path::PathBuf;
use test_r::test;

test_r::enable!();

fn component(fixture: &str) -> Vec<u8> {
    let path = PathBuf::from("../test-components").join(format!("{fixture}.wasm"));
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "missing retention fixture {} ({error}); build it with \
             `cd test-components && ./build-components.sh rust`",
            path.display()
        )
    })
}

fn evidence(fixture: &str) -> Evidence {
    component_evidence(&component(fixture)).unwrap()
}

#[test]
fn empty_preserves_canonical_floor_contract() {
    let floor = component_contract(&component("retention_canonical_floor")).unwrap();
    let empty = component_contract(&component("retention_empty")).unwrap();
    check_export_contract(&floor, &empty, &GOLEM_GUEST_EXPORTS).unwrap();
}

#[test]
fn reflection_retention_contracts_hold() {
    let negative = BTreeMap::from([
        ("empty".to_string(), evidence("retention_empty")),
        (
            "non-reflective".to_string(),
            evidence("retention_non_reflective"),
        ),
    ]);
    let positive = BTreeMap::from([("reflection".to_string(), evidence("retention_reflection"))]);
    evaluate_retention(&negative, &positive, true).unwrap();
}
