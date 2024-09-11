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

//! Tests for the 'add stub as a dependency' mechanism.

use fs_extra::dir::CopyOptions;
use golem_wasm_rpc::{WASI_POLL_WIT, WASM_RPC_WIT};
use golem_wasm_rpc_stubgen::commands::dependencies::add_stub_dependency;
use golem_wasm_rpc_stubgen::commands::generate::generate_stub_wit_dir;
use golem_wasm_rpc_stubgen::stub::StubDefinition;
use golem_wasm_rpc_stubgen::WasmRpcOverride;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn all_wit_types_no_collision() {
    let stub_dir = init_stub("all-wit-types");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(&stub_wit_root, &dest_wit_root, false, false).unwrap();

    assert_has_wit_dep(&dest_wit_root, "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_wit_root, "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit = std::fs::read_to_string(stub_wit_root.join("_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main-stub/_stub.wit", &stub_wit);

    let original_wit =
        std::fs::read_to_string(Path::new("test-data").join("all-wit-types/main.wit")).unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main/main.wit", &original_wit);
}

#[test]
fn all_wit_types_overwrite_protection() {
    let stub_dir = init_stub("all-wit-types");
    let alternative_stub_dir = init_stub("all-wit-types-alternative");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let alternative_stub_wit_root = alternative_stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(&stub_wit_root, &dest_wit_root, false, false).unwrap();
    add_stub_dependency(&alternative_stub_wit_root, &dest_wit_root, false, false).unwrap();

    assert_has_wit_dep(&dest_wit_root, "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_wit_root, "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit = std::fs::read_to_string(stub_wit_root.join("_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main-stub/_stub.wit", &stub_wit);

    let original_wit =
        std::fs::read_to_string(Path::new("test-data").join("all-wit-types/main.wit")).unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main/main.wit", &original_wit);
}

#[test]
fn all_wit_types_overwrite_protection_disabled() {
    let stub_dir = init_stub("all-wit-types");
    let alternative_stub_dir = init_stub("all-wit-types-alternative");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let alternative_stub_wit_root = alternative_stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(&stub_wit_root, &dest_wit_root, false, false).unwrap();
    add_stub_dependency(&alternative_stub_wit_root, &dest_wit_root, true, false).unwrap();

    assert_has_wit_dep(&dest_wit_root, "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_wit_root, "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit = std::fs::read_to_string(alternative_stub_wit_root.join("_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main-stub/_stub.wit", &stub_wit);

    let original_wit =
        std::fs::read_to_string(Path::new("test-data").join("all-wit-types-alternative/main.wit"))
            .unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main/main.wit", &original_wit);
}

fn init_stub(name: &str) -> TempDir {
    let tempdir = TempDir::new().unwrap();

    let source_wit_root = Path::new("test-data").join(name);
    let def = StubDefinition::new(
        &source_wit_root,
        tempdir.path(),
        &None,
        "1.0.0",
        &WasmRpcOverride::default(),
        false,
    )
    .unwrap();
    let _ = generate_stub_wit_dir(&def).unwrap();
    tempdir
}

fn init_caller(name: &str) -> TempDir {
    let tempdir = TempDir::new().unwrap();
    let source = Path::new("test-data").join(name);

    fs_extra::dir::copy(
        source,
        tempdir.path(),
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .unwrap();

    tempdir
}

fn assert_has_wit_dep(wit_dir: &Path, name: &str, expected_contents: &str) {
    let wit_file = wit_dir.join("deps").join(name);
    let contents =
        std::fs::read_to_string(&wit_file).unwrap_or_else(|_| panic!("Could not find {wit_file:?}"));
    assert_eq!(contents, expected_contents);
}
