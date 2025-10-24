// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_cli::wasm_rpc_stubgen::stub::RustDependencyOverride;
use std::path::Path;
use test_r::tag_suite;

mod add_dep;
mod cargo;
mod compose;
mod stub_wasm;
mod wit;

tag_suite!(add_dep, group1);
tag_suite!(cargo, group1);
tag_suite!(compose, group1);
tag_suite!(stub_wasm, group1);
tag_suite!(wit, group1);

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
