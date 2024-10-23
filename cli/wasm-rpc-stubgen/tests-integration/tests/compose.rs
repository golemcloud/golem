// Copyright 2024 Golem Cloud
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
use golem_wasm_ast::component::{Component, ComponentExternName};
use golem_wasm_ast::{DefaultAst, IgnoreAllButMetadata};
use golem_wasm_rpc_stubgen::commands::composition::compose;
use golem_wasm_rpc_stubgen::commands::dependencies::{add_stub_dependency, UpdateCargoToml};
use golem_wasm_rpc_stubgen::commands::generate::generate_and_build_stub;
use golem_wasm_rpc_stubgen::stub::StubDefinition;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wasm_rpc_stubgen_tests_integration::{test_data_path, wasm_rpc_override};

test_r::enable!();

#[test]
async fn compose_with_single_stub() {
    let (stub_dir, stub_wasm) = init_stub("all-wit-types").await;
    let caller_dir = init_caller("caller-no-dep-importstub");

    add_stub_dependency(
        &stub_dir.path().join("wit"),
        &caller_dir.path().join("wit"),
        true,
        UpdateCargoToml::Update,
    )
    .unwrap();

    println!(
        "{}",
        std::fs::read_to_string(stub_dir.path().join("wit/_stub.wit")).unwrap()
    );

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

    assert_not_importing(&dest_wasm, "test:main-stub/stub-api");
}

#[test]
async fn compose_with_single_stub_not_importing_stub() {
    let (stub_dir, stub_wasm) = init_stub("all-wit-types").await;
    let caller_dir = init_caller("caller-no-dep");

    add_stub_dependency(
        &stub_dir.path().join("wit"),
        &caller_dir.path().join("wit"),
        false,
        UpdateCargoToml::NoUpdate,
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

    assert_not_importing(&dest_wasm, "test:main-stub/stub-api");
}

async fn init_stub(name: &str) -> (TempDir, PathBuf) {
    let tempdir = TempDir::new().unwrap();

    let source_wit_root = test_data_path().join(name);
    let canonical_target_root = tempdir.path().canonicalize().unwrap();

    let def = StubDefinition::new(
        &source_wit_root,
        &canonical_target_root,
        &None,
        "1.0.0",
        &wasm_rpc_override(),
        false,
    )
    .unwrap();
    let wasm_path = generate_and_build_stub(&def).await.unwrap();
    (tempdir, wasm_path)
}

fn init_caller(name: &str) -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let source = test_data_path().join(name);

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

fn assert_not_importing(wasm_path: &Path, import_name: &str) {
    let component_bytes = std::fs::read(wasm_path).unwrap();
    let component: Component<IgnoreAllButMetadata> =
        Component::from_bytes(&component_bytes).unwrap();
    component.imports().iter().all(|import| {
        let ComponentExternName::Name(name) = &import.name;
        name != import_name
    });
}
