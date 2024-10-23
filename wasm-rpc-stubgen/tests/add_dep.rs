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

use test_r::test;

use assert2::assert;
use fs_extra::dir::CopyOptions;
use golem_wasm_rpc::{WASI_POLL_WIT, WASM_RPC_WIT};
use golem_wasm_rpc_stubgen::commands::dependencies::{add_stub_dependency, UpdateCargoToml};
use golem_wasm_rpc_stubgen::commands::generate::generate_stub_wit_dir;
use golem_wasm_rpc_stubgen::stub::StubDefinition;
use golem_wasm_rpc_stubgen::wit_resolve::ResolvedWitDir;
use golem_wasm_rpc_stubgen::WasmRpcOverride;
use semver::Version;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wit_encoder::{packages_from_parsed, Package, PackageName};
use wit_parser::Resolve;

test_r::enable!();

#[test]
fn all_wit_types_no_collision() {
    let stub_dir = init_stub("all-wit-types");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(
        &stub_wit_root,
        &dest_wit_root,
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wasm_rpc_wit_deps(&dest_wit_root);

    assert_has_same_wit_package(
        &PackageName::new("test", "main-stub", None),
        &dest_wit_root,
        &stub_wit_root,
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "main", None),
        &dest_wit_root,
        &Path::new("test-data").join("all-wit-types/main.wit"),
    );
}

#[test]
fn all_wit_types_overwrite_protection() {
    let stub_dir = init_stub("all-wit-types");
    let alternative_stub_dir = init_stub("all-wit-types-alternative");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let alternative_stub_wit_root = alternative_stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(
        &stub_wit_root,
        &dest_wit_root,
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &alternative_stub_wit_root,
        &dest_wit_root,
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wasm_rpc_wit_deps(&dest_wit_root);

    assert_has_same_wit_package(
        &PackageName::new("test", "main-stub", None),
        &dest_wit_root,
        &stub_wit_root,
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "main", None),
        &dest_wit_root,
        &Path::new("test-data").join("all-wit-types/main.wit"),
    );
}

#[test]
fn all_wit_types_overwrite_protection_disabled() {
    let stub_dir = init_stub("all-wit-types");
    let alternative_stub_dir = init_stub("all-wit-types-alternative");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let alternative_stub_wit_root = alternative_stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(
        &stub_wit_root,
        &dest_wit_root,
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &alternative_stub_wit_root,
        &dest_wit_root,
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wasm_rpc_wit_deps(&dest_wit_root);

    assert_has_same_wit_package(
        &PackageName::new("test", "main-stub", None),
        &dest_wit_root,
        &alternative_stub_wit_root,
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "main", None),
        &dest_wit_root,
        &Path::new("test-data").join("all-wit-types-alternative/main.wit"),
    );
}

#[test]
fn many_ways_to_export_no_collision() {
    let stub_dir = init_stub("many-ways-to-export");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_stub_dependency(
        &stub_wit_root,
        &dest_wit_root,
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wasm_rpc_wit_deps(&dest_wit_root);

    assert_has_same_wit_package(
        &PackageName::new("test", "exports-stub", None),
        &dest_wit_root,
        &stub_wit_root,
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "exports", None),
        &dest_wit_root,
        &Path::new("test-data").join("many-ways-to-export"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "sub", None),
        &dest_wit_root,
        &Path::new("test-data").join("many-ways-to-export/deps/sub/sub.wit"),
    );
}

#[test]
fn direct_circular() {
    let stub_a_dir = init_stub("direct-circular-a");
    let stub_b_dir = init_stub("direct-circular-b");

    let dest_a = init_caller("direct-circular-a");
    let dest_b = init_caller("direct-circular-b");

    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_b.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_b_dir.path().join("wit"),
        dest_a.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();


    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-stub", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b", None),
        dest_a.path(),
        &Path::new("test-data").join("direct-circular-b/b.wit"),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-stub", None),
        dest_b.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a", None),
        dest_b.path(),
        &Path::new("test-data").join("direct-circular-a/a.wit"),
    );
}

#[test]
fn direct_circular_readd() {
    let stub_a_dir = init_stub("direct-circular-a");
    let stub_b_dir = init_stub("direct-circular-b");

    let dest_a = init_caller("direct-circular-a");
    let dest_b = init_caller("direct-circular-b");

    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_b.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_b_dir.path().join("wit"),
        dest_a.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    uncomment_imports(&dest_a.path().join("a.wit"));
    uncomment_imports(&dest_b.path().join("b.wit"));

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    // At this point we simulate doing stub generation and add-stub-dependency _again_ on the a.wit and b.wit which are already have the corresponding
    // stubs imported

    regenerate_stub(stub_a_dir.path(), dest_a.path());
    regenerate_stub(stub_b_dir.path(), dest_b.path());

    println!("Second round of add_stub_dependency calls");
    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_b.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_b_dir.path().join("wit"),
        dest_a.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-stub", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    // TODO: diff on circular import
    /*assert_has_same_wit_package(
        &PackageName::new("test", "b", None),
        dest_a.path(),
        dest_b.path(),
    );*/

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-stub", None),
        dest_b.path(),
        &stub_a_dir.path().join("wit"),
    );

    // TODO: diff on circular import
    /*
    assert_has_same_wit_package(
        &PackageName::new("test", "a", None),
        dest_b.path(),
        dest_a.path(),
    );
    */
}

#[test]
fn direct_circular_same_world_name() {
    let stub_a_dir = init_stub("direct-circular-a-same-world-name");
    let stub_b_dir = init_stub("direct-circular-b-same-world-name");

    let dest_a = init_caller("direct-circular-a-same-world-name");
    let dest_b = init_caller("direct-circular-b-same-world-name");

    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_b.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_b_dir.path().join("wit"),
        dest_a.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-stub", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b", None),
        dest_a.path(),
        &Path::new("test-data").join("direct-circular-b-same-world-name/b.wit"),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-stub", None),
        dest_b.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a", None),
        dest_b.path(),
        &Path::new("test-data").join("direct-circular-a-same-world-name/a.wit"),
    );
}

#[test]
fn indirect_circular() {
    let stub_a_dir = init_stub("indirect-circular-a");
    let stub_b_dir = init_stub("indirect-circular-b");
    let stub_c_dir = init_stub("indirect-circular-c");

    let dest_a = init_caller("indirect-circular-a");
    let dest_b = init_caller("indirect-circular-b");
    let dest_c = init_caller("indirect-circular-c");

    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_c.path(),
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_b_dir.path().join("wit"),
        dest_a.path(),
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_c_dir.path().join("wit"),
        dest_b.path(),
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    // TODO: these won't be necessary after implementing https://github.com/golemcloud/wasm-rpc/issues/66
    uncomment_imports(&dest_a.path().join("a.wit"));
    uncomment_imports(&dest_b.path().join("b.wit"));
    uncomment_imports(&dest_c.path().join("c.wit"));

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());
    assert_valid_wit_root(dest_c.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-stub", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b", None),
        dest_a.path(),
        &Path::new("test-data").join("indirect-circular-b/b.wit"),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "c-stub", None),
        dest_b.path(),
        &stub_c_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "c", None),
        dest_b.path(),
        &Path::new("test-data").join("indirect-circular-c/c.wit"),
    );

    assert_has_wasm_rpc_wit_deps(dest_c.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-stub", None),
        dest_c.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a", None),
        dest_c.path(),
        &Path::new("test-data").join("indirect-circular-a/a.wit"),
    );
}

#[test]
fn indirect_circular_readd() {
    let stub_a_dir = init_stub("indirect-circular-a");
    let stub_b_dir = init_stub("indirect-circular-b");
    let stub_c_dir = init_stub("indirect-circular-c");

    let dest_a = init_caller("indirect-circular-a");
    let dest_b = init_caller("indirect-circular-b");
    let dest_c = init_caller("indirect-circular-c");

    println!("dest_a: {:?}", dest_a.path());
    println!("dest_b: {:?}", dest_b.path());
    println!("dest_c: {:?}", dest_c.path());

    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_c.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_b_dir.path().join("wit"),
        dest_a.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_c_dir.path().join("wit"),
        dest_b.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());
    assert_valid_wit_root(dest_c.path());

    // At this point we simulate doing stub generation and add-stub-dependency _again_ on the a.wit and b.wit which are already have the corresponding
    // stubs imported

    regenerate_stub(stub_a_dir.path(), dest_a.path());
    regenerate_stub(stub_b_dir.path(), dest_b.path());
    regenerate_stub(stub_c_dir.path(), dest_c.path());

    println!("Second round of add_stub_dependency calls");
    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_c.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_b_dir.path().join("wit"),
        dest_a.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();
    add_stub_dependency(
        &stub_c_dir.path().join("wit"),
        dest_b.path(),
        true,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());
    assert_valid_wit_root(dest_c.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-stub", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b", None),
        dest_a.path(),
        dest_b.path(),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "c-stub", None),
        dest_b.path(),
        &stub_c_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "c", None),
        dest_b.path(),
        dest_c.path(),
    );

    assert_has_wasm_rpc_wit_deps(dest_c.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-stub", None),
        dest_c.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a", None),
        dest_c.path(),
        dest_a.path(),
    );
}

#[test]
fn self_circular() {
    let stub_a_dir = init_stub("self-circular");
    let inlined_stub_a_dir = init_stub_inlined("self-circular");

    let dest_a = init_caller("self-circular");

    add_stub_dependency(
        &stub_a_dir.path().join("wit"),
        dest_a.path(),
        false,
        UpdateCargoToml::NoUpdate,
    )
    .unwrap();

    // TODO: these won't be necessary after implementing https://github.com/golemcloud/wasm-rpc/issues/66
    uncomment_imports(&dest_a.path().join("a.wit"));

    assert_valid_wit_root(dest_a.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-stub", None),
        dest_a.path(),
        &inlined_stub_a_dir.path().join("wit"),
    );
}

fn init_stub(name: &str) -> TempDir {
    init_stub_internal(name, false)
}

fn init_stub_inlined(name: &str) -> TempDir {
    init_stub_internal(name, true)
}

fn init_stub_internal(name: &str, always_inline_types: bool) -> TempDir {
    let tempdir = TempDir::new().unwrap();
    let canonical_target_root = tempdir.path().canonicalize().unwrap();

    let source_wit_root = Path::new("test-data").join(name);
    let def = StubDefinition::new(
        &source_wit_root,
        &canonical_target_root,
        &None,
        "1.0.0",
        &WasmRpcOverride::default(),
        always_inline_types,
    )
    .unwrap();
    let _ = generate_stub_wit_dir(&def).unwrap();
    tempdir
}

fn regenerate_stub(stub_dir: &Path, source_wit_root: &Path) {
    let def = StubDefinition::new(
        source_wit_root,
        stub_dir,
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
    ResolvedWitDir::new(wit_root).unwrap();
}

trait WitSource {
    fn resolve(&self) -> anyhow::Result<Resolve>;

    fn encoded_packages(&self) -> anyhow::Result<Vec<Package>> {
        Ok(packages_from_parsed(&self.resolve()?))
    }

    fn encoded_package(&self, package_name: &PackageName) -> anyhow::Result<Package> {
        self.encoded_packages()?
            .into_iter()
            .find(|package| package.name() == package_name)
            .ok_or_else(|| anyhow::anyhow!("package {} not found", package_name))
    }

    fn encoded_package_wit(&self, package_name: &PackageName) -> anyhow::Result<String> {
        self.encoded_package(package_name)
            .map(|package| package.to_string())
    }
}

impl WitSource for &Path {
    fn resolve(&self) -> anyhow::Result<Resolve> {
        let mut resolve = Resolve::new();
        let _ = resolve.push_path(self)?;
        Ok(resolve)
    }
}

impl WitSource for &PathBuf {
    fn resolve(&self) -> anyhow::Result<Resolve> {
        let mut resolve = Resolve::new();
        let _ = resolve.push_path(self)?;
        Ok(resolve)
    }
}

impl WitSource for &[(&str, &str)] {
    fn resolve(&self) -> anyhow::Result<Resolve> {
        let mut resolve = Resolve::new();
        for (name, source) in *self {
            let _ = resolve.push_str(name, source)?;
        }
        Ok(resolve)
    }
}

/// Asserts that both wit sources contains the same effective (encoded) wit package.
fn assert_has_same_wit_package(
    package_name: &PackageName,
    actual_wit_source: impl WitSource,
    expected_wit_source: impl WitSource,
) {
    let actual_wit = actual_wit_source.encoded_package_wit(package_name).unwrap();
    let expected_wit = expected_wit_source
        .encoded_package_wit(package_name)
        .unwrap();
    assert!(actual_wit == expected_wit)
}

fn assert_has_wasm_rpc_wit_deps(wit_dir: &Path) {
    let deps = vec![("poll", WASI_POLL_WIT), ("wasm-rpc", WASM_RPC_WIT)];

    assert_has_same_wit_package(
        &PackageName::new("wasi", "io", Some(Version::new(0, 2, 0))),
        wit_dir,
        deps.as_slice(),
    );
    assert_has_same_wit_package(
        &PackageName::new("golem", "rpc", Some(Version::new(0, 1, 0))),
        wit_dir,
        deps.as_slice(),
    );
}

fn uncomment_imports(path: &Path) {
    let contents = std::fs::read_to_string(path).unwrap();
    let uncommented = contents.replace("//!!", "");
    std::fs::write(path, uncommented).unwrap();
}
