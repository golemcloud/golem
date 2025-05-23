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

//! Tests for the 'compose with stub' command.

// TODO: test compose with multiple stubs

use crate::stubgen::{cargo_component_build, golem_rust_override, test_data_path};
use fs_extra::dir::CopyOptions;
use golem_cli::model::app::AppComponentName;
use golem_cli::wasm_rpc_stubgen::commands::composition::compose;
use golem_cli::wasm_rpc_stubgen::commands::dependencies::add_stub_dependency;
use golem_cli::wasm_rpc_stubgen::commands::generate::generate_and_build_client;
use golem_cli::wasm_rpc_stubgen::stub::{StubConfig, StubDefinition};
use golem_cli::wasm_rpc_stubgen::wit_generate::UpdateCargoToml;
use golem_wasm_ast::component::Component;
use golem_wasm_ast::DefaultAst;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use test_r::test;

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

    cargo_component_build(caller_dir.path());

    let component_wasm = caller_dir
        .path()
        .join("target")
        .join("wasm32-wasip1")
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
        client_root: canonical_target_root,
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        golem_rust_override: golem_rust_override(),
        extract_source_exports_package: true,
        seal_cargo_workspace: true,
        component_name: AppComponentName::from("test:component"),
        is_ephemeral: false,
    })
    .unwrap();
    let wasm_path = generate_and_build_client(&def, false).await.unwrap();
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

fn assert_is_component(wasm_path: &Path) {
    let _component: Component<DefaultAst> =
        Component::from_bytes(&std::fs::read(wasm_path).unwrap()).unwrap();
}
