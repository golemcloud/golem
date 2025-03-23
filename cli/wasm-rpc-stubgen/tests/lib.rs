use assert2::assert;
use golem_wasm_rpc_stubgen::stub::RustDependencyOverride;
use std::path::Path;
use test_r::tag_suite;

#[cfg(test)]
test_r::enable!();

mod add_dep;
mod cargo;
mod compose;
mod stub_wasm;
mod wit;

tag_suite!(cargo, uses_cargo);
tag_suite!(compose, uses_cargo);
tag_suite!(stub_wasm, uses_cargo);

static TEST_DATA_PATH: &str = "test-data";

pub fn test_data_path() -> &'static Path {
    Path::new(TEST_DATA_PATH)
}

pub fn golem_rust_override() -> RustDependencyOverride {
    RustDependencyOverride {
        path_override: None,
        version_override: None,
    }
}

pub fn cargo_component_build(path: &Path) {
    let status = std::process::Command::new("cargo")
        .arg("component")
        .arg("build")
        .current_dir(path)
        .status()
        .unwrap();
    assert!(status.success());
}
