// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Tests for the 'compose with stub' command.

// TODO: test compose with multiple stubs

use test_r::test;

use fs_extra::dir::CopyOptions;
use golem_wasm_ast::component::Component;
use golem_wasm_ast::DefaultAst;
use golem_wasm_rpc_stubgen::commands::composition::compose;
use golem_wasm_rpc_stubgen::commands::dependencies::{add_stub_dependency, UpdateCargoToml};
use golem_wasm_rpc_stubgen::commands::generate::generate_and_build_stub;
use golem_wasm_rpc_stubgen::stub::{StubConfig, StubDefinition};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wasm_rpc_stubgen_tests_integration::{test_data_path, wasm_rpc_override};

test_r::enable!();

#[test]
async fn compose_with_single_stub() {
    let (_source_dir, stub_dir, stub_wasm) = init_stub("all-wit-types").await;
    let caller_dir = init_caller("caller-no-dep-importstub");

    add_stub_dependency(
        &stub_dir.path().join("wit"),
        &caller_dir.path().join("wit"),
        UpdateCargoToml::Update,
    )
    .unwrap();

    compile_rust(caller_dir.path());

    let component_wasm = caller_dir
        .path()
        .join("target")
        .join("wasm32-wasi")
        .join("debug")
        .join("caller_no_dep.wasm");

    assert_is_component(&stub_wasm);
    assert_is_component(&component_wasm);

    let dest_wasm = caller_dir.path().join("target/result.wasm");
    compose(&component_wasm, &[stub_wasm], &dest_wasm)
        .await
        .unwrap();
}

async fn init_stub(name: &str) -> (TempDir, TempDir, PathBuf) {
    let source_dir = TempDir::new().unwrap();
    let source_wit_root = source_dir.path().canonicalize().unwrap();

    fs_extra::dir::copy(
        test_data_path().join("wit").join(name),
        &source_wit_root,
        &CopyOptions::new().content_only(true),
    )
    .unwrap();

    let stub_dir = TempDir::new().unwrap();
    let canonical_target_root = stub_dir.path().canonicalize().unwrap();

    let def = StubDefinition::new(StubConfig {
        source_wit_root,
        target_root: canonical_target_root,
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        wasm_rpc_override: wasm_rpc_override(),
        extract_source_interface_package: true,
        seal_cargo_workspace: true,
    })
    .unwrap();
    let wasm_path = generate_and_build_stub(&def, false).await.unwrap();
    (source_dir, stub_dir, wasm_path)
}

fn init_caller(name: &str) -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let source = test_data_path().join("wit").join(name);

    fs_extra::dir::copy(
        source,
        temp_dir.path(),
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .unwrap();

    temp_dir
}

fn compile_rust(path: &Path) {
    let status = std::process::Command::new("cargo")
        .arg("component")
        .arg("build")
        .current_dir(path)
        .status()
        .unwrap();
    assert!(status.success());
}

fn assert_is_component(wasm_path: &Path) {
    let _component: Component<DefaultAst> =
        Component::from_bytes(&std::fs::read(wasm_path).unwrap()).unwrap();
}
