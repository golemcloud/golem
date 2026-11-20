//! Reflection retention contracts on the prebuilt, named retention fixtures. The fixtures come
//! from `test-components/size-analysis/fixtures`, which `test-components/build-components.sh rust`
//! builds and copies to `test-components/`.

use golem_size_analysis::retention::{Evidence, component_evidence, evaluate_retention};
use std::collections::BTreeMap;
use std::path::PathBuf;
use test_r::test;

test_r::enable!();

fn evidence(fixture: &str) -> Evidence {
    let path = PathBuf::from("../test-components").join(format!("{fixture}.wasm"));
    let component = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "missing retention fixture {} ({error}); build it with \
             `cd test-components && ./build-components.sh rust`",
            path.display()
        )
    });
    component_evidence(&component).unwrap()
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
