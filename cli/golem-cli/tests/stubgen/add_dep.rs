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

//! Tests for the 'add stub as a dependency' mechanism.

use test_r::test;

use assert2::assert;
use fs_extra::dir::CopyOptions;
use golem_cli::model::app::AppComponentName;
use golem_cli::wasm_rpc_stubgen::commands::generate::generate_client_wit_dir;
use golem_cli::wasm_rpc_stubgen::stub::{RustDependencyOverride, StubConfig, StubDefinition};
use golem_cli::wasm_rpc_stubgen::wit_generate::{
    add_client_as_dependency_to_wit_dir, AddClientAsDepConfig, UpdateCargoToml,
};
use golem_cli::wasm_rpc_stubgen::wit_resolve::ResolvedWitDir;
use golem_cli::wasm_rpc_stubgen::{
    GOLEM_RPC_WIT, GOLEM_RPC_WIT_VERSION, WASI_CLOCKS_WIT, WASI_IO_WIT, WASI_WIT_VERSION,
};
use itertools::Itertools;
use semver::Version;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tempfile::TempDir;
use wit_encoder::{packages_from_parsed, Package, PackageName};
use wit_parser::Resolve;

test_r::enable!();

#[test]
fn all_wit_types_no_collision() {
    let (_source_dir, stub_dir) = init_stub("all-wit-types");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_wit_root.clone(),
        dest_wit_root: dest_wit_root.clone(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wasm_rpc_wit_deps(&dest_wit_root);

    assert_has_same_wit_package(
        &PackageName::new("test", "main-client", None),
        &dest_wit_root,
        &stub_wit_root,
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "main-exports", None),
        &dest_wit_root,
        &stub_wit_root,
    );
}

#[test]
fn all_wit_types_re_add_with_changes() {
    let (source_dir, stub_dir) = init_stub("all-wit-types");
    let (alternative_source_dir, alternative_stub_dir) = init_stub("all-wit-types-alternative");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let alternative_stub_wit_root = alternative_stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_wit_root.clone(),
        dest_wit_root: dest_wit_root.clone(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);
    assert_has_wasm_rpc_wit_deps(&dest_wit_root);
    assert_has_same_wit_package(
        &PackageName::new("test", "main-exports", None),
        source_dir.path(),
        &stub_wit_root,
    );
    assert_has_same_wit_package(
        &PackageName::new("test", "main-exports", None),
        source_dir.path(),
        &dest_wit_root,
    );
    assert_has_same_wit_package(
        &PackageName::new("test", "main-client", None),
        &stub_wit_root,
        &dest_wit_root,
    );

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: alternative_stub_wit_root.clone(),
        dest_wit_root: dest_wit_root.clone(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);
    assert_has_wasm_rpc_wit_deps(&dest_wit_root);
    assert_has_same_wit_package(
        &PackageName::new("test", "main-exports", None),
        alternative_source_dir.path(),
        &alternative_stub_wit_root,
    );
    assert_has_same_wit_package(
        &PackageName::new("test", "main-exports", None),
        alternative_source_dir.path(),
        &dest_wit_root,
    );
    assert_has_same_wit_package(
        &PackageName::new("test", "main-client", None),
        &alternative_stub_wit_root,
        &dest_wit_root,
    );
}

#[test]
fn many_ways_to_export_no_collision() {
    let (source_dir, stub_dir) = init_stub("many-ways-to-export");
    let dest_dir = init_caller("caller-no-dep");

    let stub_wit_root = stub_dir.path().join("wit");
    let dest_wit_root = dest_dir.path().join("wit");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_wit_root.clone(),
        dest_wit_root: dest_wit_root.clone(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(&dest_wit_root);

    assert_has_wasm_rpc_wit_deps(&dest_wit_root);

    assert_has_same_wit_package(
        &PackageName::new("test", "exports-client", None),
        &dest_wit_root,
        &stub_wit_root,
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "exports-exports", None),
        source_dir.path(),
        &dest_wit_root,
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "sub", None),
        &dest_wit_root,
        Path::new("test-data/wit/many-ways-to-export/deps/sub/sub.wit"),
    );
}

#[test]
fn direct_circular() {
    let (_source_a_dir, stub_a_dir) = init_stub("direct-circular-a");
    let (_source_b_dir, stub_b_dir) = init_stub("direct-circular-b");

    let dest_a = init_caller("direct-circular-a");
    let dest_b = init_caller("direct-circular-b");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_b.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_b_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-client", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b-exports", None),
        dest_a.path(),
        _source_b_dir.path(),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-client", None),
        dest_b.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a-exports", None),
        dest_b.path(),
        _source_a_dir.path(),
    );
}

#[test]
fn direct_circular_readd() {
    let (_source_a_dir, stub_a_dir) = init_stub("direct-circular-a");
    let (_source_b_dir, stub_b_dir) = init_stub("direct-circular-b");

    let dest_a = init_caller("direct-circular-a");
    let dest_b = init_caller("direct-circular-b");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_b.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_b_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    // At this point we simulate doing stub generation and add-stub-dependency _again_ on the a.wit and b.wit which are already have the corresponding
    // stubs imported

    regenerate_stub(stub_a_dir.path(), dest_a.path());
    regenerate_stub(stub_b_dir.path(), dest_b.path());

    println!("Second round of add_stub_dependency calls");
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_b.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_b_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-client", None),
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
        &PackageName::new("test", "a-client", None),
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
    let (source_a_dir, stub_a_dir) = init_stub("direct-circular-a-same-world-name");
    let (source_b_dir, stub_b_dir) = init_stub("direct-circular-b-same-world-name");

    let dest_a = init_caller("direct-circular-a-same-world-name");
    let dest_b = init_caller("direct-circular-b-same-world-name");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_b.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_b_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-client", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b-exports", None),
        dest_a.path(),
        source_b_dir.path(),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-client", None),
        dest_b.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a-exports", None),
        dest_b.path(),
        source_a_dir.path(),
    );
}

#[test]
fn indirect_circular() {
    let (source_a_dir, stub_a_dir) = init_stub("indirect-circular-a");
    let (_source_b_dir, stub_b_dir) = init_stub("indirect-circular-b");
    let (_source_c_dir, stub_c_dir) = init_stub("indirect-circular-c");

    let dest_a = init_caller("indirect-circular-a");
    let dest_b = init_caller("indirect-circular-b");
    let dest_c = init_caller("indirect-circular-c");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_c.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_b_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_c_dir.path().join("wit"),
        dest_wit_root: dest_b.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());
    assert_valid_wit_root(dest_c.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-client", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b-exports", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "c-client", None),
        dest_b.path(),
        &stub_c_dir.path().join("wit"),
    );

    assert_has_wasm_rpc_wit_deps(dest_c.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-client", None),
        dest_c.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a-exports", None),
        dest_c.path(),
        source_a_dir.path(),
    );
}

#[test]
fn indirect_circular_readd() {
    let (_source_a_dir, stub_a_dir) = init_stub("indirect-circular-a");
    let (_source_b_dir, stub_b_dir) = init_stub("indirect-circular-b");
    let (_source_c_dir, stub_c_dir) = init_stub("indirect-circular-c");

    let dest_a = init_caller("indirect-circular-a");
    let dest_b = init_caller("indirect-circular-b");
    let dest_c = init_caller("indirect-circular-c");

    println!("dest_a: {:?}", dest_a.path());
    println!("dest_b: {:?}", dest_b.path());
    println!("dest_c: {:?}", dest_c.path());

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_c.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_b_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_c_dir.path().join("wit"),
        dest_wit_root: dest_b.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
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
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_c.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_b_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();
    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_c_dir.path().join("wit"),
        dest_wit_root: dest_b.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(dest_a.path());
    assert_valid_wit_root(dest_b.path());
    assert_valid_wit_root(dest_c.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "b-client", None),
        dest_a.path(),
        &stub_b_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "b-exports", None),
        dest_a.path(),
        dest_b.path(),
    );

    assert_has_wasm_rpc_wit_deps(dest_b.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "c-client", None),
        dest_b.path(),
        &stub_c_dir.path().join("wit"),
    );

    assert_has_no_package_by_name(&PackageName::new("test", "c-exports", None), dest_b.path());
    assert_has_package_by_name(&PackageName::new("test", "c-exports", None), dest_c.path());

    assert_has_wasm_rpc_wit_deps(dest_c.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-client", None),
        dest_c.path(),
        &stub_a_dir.path().join("wit"),
    );

    assert_has_same_wit_package(
        &PackageName::new("test", "a-exports", None),
        dest_c.path(),
        dest_a.path(),
    );
    assert_has_no_package_by_name(&PackageName::new("test", "a", None), dest_c.path());
    assert_has_package_by_name(&PackageName::new("test", "a", None), dest_a.path());
}

#[test]
fn self_circular() {
    let (_source_a_dir, stub_a_dir) = init_stub("self-circular");

    let dest_a = init_caller("self-circular");

    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_a_dir.path().join("wit"),
        dest_wit_root: dest_a.path().to_path_buf(),
        update_cargo_toml: UpdateCargoToml::NoUpdate,
    })
    .unwrap();

    assert_valid_wit_root(dest_a.path());

    assert_has_wasm_rpc_wit_deps(dest_a.path());

    assert_has_same_wit_package(
        &PackageName::new("test", "a-client", None),
        dest_a.path(),
        &stub_a_dir.path().join("wit"),
    );
}

fn init_stub(name: &str) -> (TempDir, TempDir) {
    let source = tempfile::Builder::new()
        .disable_cleanup(true)
        .tempdir()
        .unwrap();
    let canonical_source = source.path().canonicalize().unwrap();

    fs_extra::dir::copy(
        Path::new("test-data/wit").join(name),
        &canonical_source,
        &CopyOptions::new().content_only(true),
    )
    .unwrap();

    let target = tempfile::Builder::new()
        .disable_cleanup(true)
        .tempdir()
        .unwrap();
    let canonical_target = target.path().canonicalize().unwrap();

    let def = StubDefinition::new(StubConfig {
        source_wit_root: canonical_source,
        client_root: canonical_target,
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        golem_rust_override: RustDependencyOverride::default(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:component"),
        is_ephemeral: false,
    })
    .unwrap();
    let _ = generate_client_wit_dir(&def).unwrap();
    (source, target)
}

fn regenerate_stub(stub_dir: &Path, source_wit_root: &Path) {
    let def = StubDefinition::new(StubConfig {
        source_wit_root: source_wit_root.to_path_buf(),
        client_root: stub_dir.to_path_buf(),
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        golem_rust_override: RustDependencyOverride::default(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:component"),
        is_ephemeral: false,
    })
    .unwrap();
    let _ = generate_client_wit_dir(&def).unwrap();
}

fn init_caller(name: &str) -> TempDir {
    let temp_dir = tempfile::Builder::new()
        .disable_cleanup(true)
        .tempdir()
        .unwrap();
    let source = Path::new("test-data/wit").join(name);

    fs_extra::dir::copy(
        source,
        temp_dir.path(),
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .unwrap();

    temp_dir
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

impl WitSource for &[(&str, &[(&str, &str)])] {
    fn resolve(&self) -> anyhow::Result<Resolve> {
        let mut resolve = Resolve::new();
        for (name, sources) in *self {
            let mut sources: Vec<_> = sources.iter().collect();
            sources.sort_by_key(|(name, _)| *name);

            let split_sources = sources
                .iter()
                .map(|(_, source)| {
                    let lines = source.lines().collect::<Vec<_>>();
                    if lines[0].starts_with("package ") {
                        (lines[0], lines[1..].join("\n"))
                    } else {
                        ("", lines.join("\n"))
                    }
                })
                .collect::<Vec<_>>();

            let mut merged_sources = split_sources
                .iter()
                .find(|(hdr, _)| !hdr.is_empty())
                .unwrap()
                .0
                .to_string();
            merged_sources.push('\n');
            merged_sources.push_str(&split_sources.iter().map(|(_, source)| source).join("\n"));
            merged_sources.push('\n');
            let _ = resolve.push_str(name, &merged_sources)?;
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
    assert_eq!(actual_wit, expected_wit)
}

fn assert_has_no_package_by_name(package_name: &PackageName, wit_source: impl WitSource) {
    assert!(wit_source.encoded_package(package_name).is_err())
}

fn assert_has_package_by_name(package_name: &PackageName, wit_source: impl WitSource) {
    assert!(wit_source.encoded_package(package_name).is_ok())
}

fn assert_has_wasm_rpc_wit_deps(wit_dir: &Path) {
    let deps: Vec<(&str, &[(&str, &str)])> = vec![
        ("wasi:io", WASI_IO_WIT),
        ("wasi:clocks", WASI_CLOCKS_WIT),
        ("wasm-rpc", &[("wasm-rpc.wit", GOLEM_RPC_WIT)]),
    ];

    assert_has_same_wit_package(
        &PackageName::new(
            "wasi",
            "io",
            Some(Version::from_str(WASI_WIT_VERSION).unwrap()),
        ),
        wit_dir,
        deps.as_slice(),
    );
    assert_has_same_wit_package(
        &PackageName::new(
            "wasi",
            "clocks",
            Some(Version::from_str(WASI_WIT_VERSION).unwrap()),
        ),
        wit_dir,
        deps.as_slice(),
    );
    assert_has_same_wit_package(
        &PackageName::new(
            "golem",
            "rpc",
            Some(Version::from_str(GOLEM_RPC_WIT_VERSION).unwrap()),
        ),
        wit_dir,
        deps.as_slice(),
    );
}
