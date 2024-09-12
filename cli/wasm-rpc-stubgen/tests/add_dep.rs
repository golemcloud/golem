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
use golem_wasm_rpc_stubgen::stub::{get_unresolved_packages, StubDefinition};
use golem_wasm_rpc_stubgen::WasmRpcOverride;
use std::path::Path;
use tempfile::TempDir;
use wit_parser::Resolve;

#[test]
fn all_wit_types_no_collision() {
    let stub_dir = init_stub("all-wit-types");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(&stub_wit_root, &dest_wit_root, false, false).unwrap();

    assert_valid_wit_root(&dest_wit_root);

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

    assert_valid_wit_root(&dest_wit_root);

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

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wit_dep(&dest_wit_root, "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_wit_root, "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit = std::fs::read_to_string(alternative_stub_wit_root.join("_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main-stub/_stub.wit", &stub_wit);

    let original_wit =
        std::fs::read_to_string(Path::new("test-data").join("all-wit-types-alternative/main.wit"))
            .unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_main/main.wit", &original_wit);
}

#[test]
fn many_ways_to_export_no_collision() {
    let stub_dir = init_stub("many-ways-to-export");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(&stub_wit_root, &dest_wit_root, false, false).unwrap();

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wit_dep(&dest_wit_root, "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_wit_root, "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit = std::fs::read_to_string(stub_wit_root.join("_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_exports-stub/_stub.wit", &stub_wit);

    let original_wit =
        std::fs::read_to_string(Path::new("test-data").join("many-ways-to-export/main.wit"))
            .unwrap();
    assert_has_wit_dep(&dest_wit_root, "test_exports/main.wit", &original_wit);

    let original_sub_wit = std::fs::read_to_string(
        Path::new("test-data").join("many-ways-to-export/deps/sub/sub.wit"),
    )
    .unwrap();
    assert_has_wit_dep(&dest_wit_root, "sub/sub.wit", &original_sub_wit);
}

#[test]
fn direct_circular() {
    let stub_a_dir = init_stub("direct-circular-a");
    let stub_b_dir = init_stub("direct-circular-b");

    let dest_a = init_caller("direct-circular-a");
    let dest_b = init_caller("direct-circular-b");

    add_stub_dependency(&stub_a_dir.path().join("wit"), dest_b.path(), false, false).unwrap();
    add_stub_dependency(&stub_b_dir.path().join("wit"), dest_a.path(), false, false).unwrap();

    // TODO: these won't be necessary after implementing https://github.com/golemcloud/wasm-rpc/issues/66
    uncomment_imports(&dest_a.path().join("a.wit"));
    uncomment_imports(&dest_b.path().join("b.wit"));

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wit_dep(&dest_a.path(), "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_a.path(), "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit_b = std::fs::read_to_string(stub_b_dir.path().join("wit/_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_a.path(), "test_b-stub/_stub.wit", &stub_wit_b);

    let original_b =
        std::fs::read_to_string(Path::new("test-data").join("direct-circular-b/b.wit")).unwrap();
    assert_has_wit_dep_similar(&dest_a.path(), "test_b/b.wit", &original_b);

    assert_has_wit_dep(&dest_b.path(), "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_b.path(), "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit_a = std::fs::read_to_string(stub_a_dir.path().join("wit/_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_b.path(), "test_a-stub/_stub.wit", &stub_wit_a);

    let original_a =
        std::fs::read_to_string(Path::new("test-data").join("direct-circular-a/a.wit")).unwrap();
    assert_has_wit_dep_similar(&dest_b.path(), "test_a/a.wit", &original_a);
}

#[test]
fn direct_circular_readd() {
    let stub_a_dir = init_stub("direct-circular-a");
    let stub_b_dir = init_stub("direct-circular-b");

    let dest_a = init_caller("direct-circular-a");
    let dest_b = init_caller("direct-circular-b");

    add_stub_dependency(&stub_a_dir.path().join("wit"), dest_b.path(), false, false).unwrap();
    add_stub_dependency(&stub_b_dir.path().join("wit"), dest_a.path(), false, false).unwrap();

    // TODO: these won't be necessary after implementing https://github.com/golemcloud/wasm-rpc/issues/66
    uncomment_imports(&dest_a.path().join("a.wit"));
    uncomment_imports(&dest_b.path().join("b.wit"));

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    // At this point we simulate doing stub generation and add-stub-dependency _again_ on the a.wit and b.wit which are already have the corresponding
    // stubs imported

    regenerate_stub(stub_a_dir.path(), dest_a.path());
    regenerate_stub(stub_b_dir.path(), dest_b.path());

    println!("Second round of add_stub_dependency calls");
    add_stub_dependency(&stub_a_dir.path().join("wit"), dest_b.path(), true, false).unwrap();
    add_stub_dependency(&stub_b_dir.path().join("wit"), dest_a.path(), true, false).unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wit_dep(&dest_a.path(), "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_a.path(), "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit_b = std::fs::read_to_string(stub_b_dir.path().join("wit/_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_a.path(), "test_b-stub/_stub.wit", &stub_wit_b);

    let original_b =
        std::fs::read_to_string(Path::new("test-data").join("direct-circular-b/b.wit")).unwrap();
    assert_has_wit_dep_similar(&dest_a.path(), "test_b/b.wit", &original_b);

    assert_has_wit_dep(&dest_b.path(), "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_b.path(), "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit_a = std::fs::read_to_string(stub_a_dir.path().join("wit/_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_b.path(), "test_a-stub/_stub.wit", &stub_wit_a);

    let original_a =
        std::fs::read_to_string(Path::new("test-data").join("direct-circular-a/a.wit")).unwrap();
    assert_has_wit_dep_similar(&dest_b.path(), "test_a/a.wit", &original_a);
}

#[test]
fn direct_circular_same_world_name() {
    let stub_a_dir = init_stub("direct-circular-a-same-world-name");
    let stub_b_dir = init_stub("direct-circular-b-same-world-name");

    let dest_a = init_caller("direct-circular-a-same-world-name");
    let dest_b = init_caller("direct-circular-b-same-world-name");

    add_stub_dependency(&stub_a_dir.path().join("wit"), dest_b.path(), false, false).unwrap();
    add_stub_dependency(&stub_b_dir.path().join("wit"), dest_a.path(), false, false).unwrap();

    // TODO: these won't be necessary after implementing https://github.com/golemcloud/wasm-rpc/issues/66
    uncomment_imports(&dest_a.path().join("a.wit"));
    uncomment_imports(&dest_b.path().join("b.wit"));

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wit_dep(&dest_a.path(), "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_a.path(), "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit_b = std::fs::read_to_string(stub_b_dir.path().join("wit/_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_a.path(), "test_b-stub/_stub.wit", &stub_wit_b);

    let original_b = std::fs::read_to_string(
        Path::new("test-data").join("direct-circular-b-same-world-name/b.wit"),
    )
    .unwrap();
    assert_has_wit_dep_similar(&dest_a.path(), "test_b/b.wit", &original_b);

    assert_has_wit_dep(&dest_b.path(), "io/poll.wit", WASI_POLL_WIT);
    assert_has_wit_dep(&dest_b.path(), "wasm-rpc/wasm-rpc.wit", WASM_RPC_WIT);

    let stub_wit_a = std::fs::read_to_string(stub_a_dir.path().join("wit/_stub.wit")).unwrap();
    assert_has_wit_dep(&dest_b.path(), "test_a-stub/_stub.wit", &stub_wit_a);

    let original_a = std::fs::read_to_string(
        Path::new("test-data").join("direct-circular-a-same-world-name/a.wit"),
    )
    .unwrap();
    assert_has_wit_dep_similar(&dest_b.path(), "test_a/a.wit", &original_a);
}

// TODO: test update-cargo feature

fn init_stub(name: &str) -> TempDir {
    let tempdir = TempDir::new().unwrap();
    let canonical_target_root = tempdir.path().canonicalize().unwrap();

    let source_wit_root = Path::new("test-data").join(name);
    let def = StubDefinition::new(
        &source_wit_root,
        &canonical_target_root,
        &None,
        "1.0.0",
        &WasmRpcOverride::default(),
        false,
    )
    .unwrap();
    let _ = generate_stub_wit_dir(&def).unwrap();
    tempdir
}

fn regenerate_stub(stub_dir: &Path, source_wit_root: &Path) {
    let def = StubDefinition::new(
        &source_wit_root,
        &stub_dir,
        &None,
        "1.0.0",
        &WasmRpcOverride::default(),
        false,
    )
    .unwrap();
    let _ = generate_stub_wit_dir(&def).unwrap();
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

fn assert_valid_wit_root(wit_root: &Path) {
    let (final_root, final_deps) = get_unresolved_packages(&wit_root).unwrap();

    let mut final_resolve = Resolve::new();
    for unresolved in final_deps.iter().cloned() {
        final_resolve.push(unresolved).unwrap();
    }
    final_resolve.push(final_root.clone()).unwrap();
}

/// Asserts that the destination WIT root has a dependency with the given name and contents.
fn assert_has_wit_dep(wit_dir: &Path, name: &str, expected_contents: &str) {
    let wit_file = wit_dir.join("deps").join(name);
    let contents = std::fs::read_to_string(&wit_file)
        .unwrap_or_else(|_| panic!("Could not find {wit_file:?}"));
    assert_eq!(contents, expected_contents, "checking {wit_file:?}");
}

/// Asserts that the destination WIT root has a dependency with the given name and it's contents are
/// similar to the expected one - meaning each non-comment line can be found in the expected contents
/// but missing lines are allowed.
fn assert_has_wit_dep_similar(wit_dir: &Path, name: &str, expected_contents: &str) {
    let wit_file = wit_dir.join("deps").join(name);
    let contents = std::fs::read_to_string(&wit_file)
        .unwrap_or_else(|_| panic!("Could not find {wit_file:?}"));

    for line in contents.lines() {
        if !line.starts_with("//") {
            assert!(expected_contents.contains(line), "checking {wit_file:?}");
        }
    }
}

fn uncomment_imports(path: &Path) {
    let contents = std::fs::read_to_string(path).unwrap();
    let uncommented = contents.replace("//!!", "");
    std::fs::write(path, uncommented).unwrap();
}
