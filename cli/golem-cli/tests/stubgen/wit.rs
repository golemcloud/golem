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

//! Tests in this module are verifying the STUB WIT created by the stub generator

use test_r::test;

use fs_extra::dir::CopyOptions;
use golem_cli::model::app::AppComponentName;
use golem_cli::wasm_rpc_stubgen::commands::generate::generate_client_wit_dir;
use golem_cli::wasm_rpc_stubgen::stub::{RustDependencyOverride, StubConfig, StubDefinition};
use std::path::Path;
use tempfile::{tempdir, TempDir};
use wit_parser::{FunctionKind, Resolve, TypeDefKind, TypeOwner};

test_r::enable!();

#[test]
fn all_wit_types() {
    let source_wit_root = init_source("all-wit-types");
    let target_root = tempdir().unwrap();

    let def = StubDefinition::new(StubConfig {
        source_wit_root: source_wit_root.path().to_path_buf(),
        client_root: target_root.path().to_path_buf(),
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        golem_rust_override: RustDependencyOverride::default(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:component"),
        is_ephemeral: false,
    })
    .unwrap();
    let resolve = generate_client_wit_dir(&def).unwrap().resolve;

    assert_has_package_name(&resolve, "test:main-client");
    assert_has_world(&resolve, "wasm-rpc-client-api");
    assert_has_interface(&resolve, "api-client");

    assert_has_stub_function(&resolve, "api-client", "iface1", "no-op", false);
    assert_has_stub_function(&resolve, "api-client", "iface1", "get-bool", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "set-bool", false);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-bool", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-s8", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-s16", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-s32", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-s64", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-u8", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-u16", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-u32", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-u64", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-f32", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-f64", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-char", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "identity-string", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "get-orders", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "set-orders", false);
    assert_has_stub_function(&resolve, "api-client", "iface1", "apply-metadata", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "get-option-bool", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "set-option-bool", false);
    assert_has_stub_function(&resolve, "api-client", "iface1", "get-coordinates", true);
    assert_has_stub_function(
        &resolve,
        "api-client",
        "iface1",
        "get-coordinates-alias",
        true,
    );
    assert_has_stub_function(&resolve, "api-client", "iface1", "set-coordinates", false);
    assert_has_stub_function(
        &resolve,
        "api-client",
        "iface1",
        "set-coordinates-alias",
        false,
    );
    assert_has_stub_function(&resolve, "api-client", "iface1", "tuple-to-point", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "pt-log-error", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "validate-pt", true);
    assert_has_stub_function(
        &resolve,
        "api-client",
        "iface1",
        "print-checkout-result",
        true,
    );
    assert_has_stub_function(
        &resolve,
        "api-client",
        "iface1",
        "get-checkout-result",
        true,
    );
    assert_has_stub_function(&resolve, "api-client", "iface1", "get-color", true);
    assert_has_stub_function(&resolve, "api-client", "iface1", "set-color", false);
    assert_has_stub_function(
        &resolve,
        "api-client",
        "iface1",
        "validate-permissions",
        true,
    );

    assert_defines_enum(&resolve, "test:main-exports", "iface1", "color");
    assert_defines_flags(&resolve, "test:main-exports", "iface1", "permissions");
    assert_defines_record(&resolve, "test:main-exports", "iface1", "metadata");
    assert_defines_record(&resolve, "test:main-exports", "iface1", "point");
    assert_defines_record(&resolve, "test:main-exports", "iface1", "product-item");
    assert_defines_record(&resolve, "test:main-exports", "iface1", "order");
    assert_defines_record(
        &resolve,
        "test:main-exports",
        "iface1",
        "order-confirmation",
    );
    assert_defines_tuple_alias(&resolve, "test:main-exports", "iface1", "point-tuple");
    assert_defines_variant(&resolve, "test:main-exports", "iface1", "checkout-result");

    // TODO: duplicated rec1 types
}

#[test]
fn many_ways_to_export() {
    let source_wit_root = init_source("many-ways-to-export");
    let target_root = tempdir().unwrap();

    let def = StubDefinition::new(StubConfig {
        source_wit_root: source_wit_root.path().to_path_buf(),
        client_root: target_root.path().to_path_buf(),
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        golem_rust_override: RustDependencyOverride::default(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:component"),
        is_ephemeral: false,
    })
    .unwrap();
    let resolve = generate_client_wit_dir(&def).unwrap().resolve;

    assert_has_package_name(&resolve, "test:exports-client");

    assert_has_world(&resolve, "wasm-rpc-client-api");
    assert_has_interface(&resolve, "api-client");

    assert_has_stub_function(
        &resolve,
        "api-client",
        "api-inline-functions",
        "func1",
        false,
    );
    assert_has_stub_function(&resolve, "api-client", "iface1", "func2", false);
    // TODO:
    // assert_has_stub_function(&resolve, "api-client", "iface2", "func3", true);
    assert_has_stub_function(&resolve, "api-client", "api-inline-iface", "func4", false);
    assert_has_stub_function(&resolve, "api-client", "iface4", "func5", false);

    // TODO: add asserts for non-unique types
}

fn assert_has_package_name(resolve: &Resolve, package_name: &str) {
    assert!(resolve
        .packages
        .iter()
        .any(|(_pkg_id, pkg)| pkg.name.to_string() == *package_name))
}

fn assert_has_world(resolve: &Resolve, world_name: &str) {
    assert!(resolve
        .worlds
        .iter()
        .any(|(_world_id, world)| world.name == world_name))
}

fn assert_has_interface(resolve: &Resolve, interface_name: &str) {
    assert!(resolve
        .interfaces
        .iter()
        .any(|(_iface_id, iface)| iface.name == Some(interface_name.to_string())))
}

fn assert_has_stub_function(
    resolve: &Resolve,
    interface_name: &str,
    resource_name: &str,
    function_name: &str,
    has_result: bool,
) {
    let (_, iface) = resolve
        .interfaces
        .iter()
        .find(|(_iface_id, iface)| iface.name == Some(interface_name.to_string()))
        .unwrap();
    let (_, resource_id) = iface
        .types
        .iter()
        .find(|(name, _typ_id)| name.as_str() == resource_name)
        .unwrap();
    let resource_typ = resolve.types.get(*resource_id).unwrap();
    assert_eq!(resource_typ.kind, TypeDefKind::Resource);

    let async_function_name = format!("[method]{resource_name}.{function_name}");
    let blocking_function_name = format!("[method]{resource_name}.blocking-{function_name}");

    let (_, _async_function) = iface
        .functions
        .iter()
        .find(|(_, fun)| {
            fun.kind == FunctionKind::Method(*resource_id) && fun.name == async_function_name
        })
        .unwrap_or_else(|| {
            panic!("Could not find method {async_function_name} in interface {interface_name}")
        });
    let (_, _blocking_function) = iface
        .functions
        .iter()
        .find(|(_, fun)| {
            fun.kind == FunctionKind::Method(*resource_id) && fun.name == blocking_function_name
        })
        .unwrap_or_else(|| {
            panic!("Could not find method {blocking_function_name} in interface {interface_name}")
        });

    if has_result {
        // for functions with a result value the async version returns a generated resource type
        let future_result_name = format!("future-{function_name}-result");
        let (_, result_resource_id) = iface
            .types
            .iter()
            .find(|(name, _typ_id)| name.as_str() == future_result_name)
            .unwrap();
        let result_resource_typ = resolve.types.get(*result_resource_id).unwrap();
        assert_eq!(result_resource_typ.kind, TypeDefKind::Resource);
    }
}

fn assert_defines_enum(
    resolve: &Resolve,
    package_name: &str,
    interface_name: &str,
    enum_name: &str,
) {
    assert!(resolve
        .types
        .iter()
        .any(|(_, typ)| typ.name == Some(enum_name.to_string())
            && matches!(typ.kind, TypeDefKind::Enum(_))
            && is_owned_by_interface(resolve, &typ.owner, package_name, interface_name)))
}

fn assert_defines_flags(
    resolve: &Resolve,
    package_name: &str,
    interface_name: &str,
    flags_name: &str,
) {
    assert!(resolve
        .types
        .iter()
        .any(|(_, typ)| typ.name == Some(flags_name.to_string())
            && matches!(typ.kind, TypeDefKind::Flags(_))
            && is_owned_by_interface(resolve, &typ.owner, package_name, interface_name)))
}

fn assert_defines_record(
    resolve: &Resolve,
    package_name: &str,
    interface_name: &str,
    record_name: &str,
) {
    assert!(resolve
        .types
        .iter()
        .any(|(_, typ)| typ.name == Some(record_name.to_string())
            && matches!(typ.kind, TypeDefKind::Record(_))
            && is_owned_by_interface(resolve, &typ.owner, package_name, interface_name)))
}

fn assert_defines_tuple_alias(
    resolve: &Resolve,
    package_name: &str,
    interface_name: &str,
    alias_name: &str,
) {
    assert!(resolve
        .types
        .iter()
        .any(|(_, typ)| typ.name == Some(alias_name.to_string())
            && matches!(typ.kind, TypeDefKind::Tuple(_))
            && is_owned_by_interface(resolve, &typ.owner, package_name, interface_name)))
}

fn assert_defines_variant(
    resolve: &Resolve,
    package_name: &str,
    interface_name: &str,
    variant_name: &str,
) {
    assert!(resolve
        .types
        .iter()
        .any(|(_, typ)| typ.name == Some(variant_name.to_string())
            && matches!(typ.kind, TypeDefKind::Variant(_))
            && is_owned_by_interface(resolve, &typ.owner, package_name, interface_name)))
}

fn is_owned_by_interface(
    resolve: &Resolve,
    owner: &TypeOwner,
    package_name: &str,
    interface_name: &str,
) -> bool {
    match owner {
        TypeOwner::World(_) => false,
        TypeOwner::Interface(iface_id) => {
            let interface = resolve.interfaces.get(*iface_id).unwrap();
            interface.name == Some(interface_name.to_string())
                && interface
                    .package
                    .and_then(|package_id| resolve.packages.get(package_id))
                    .map(|package| package.name.to_string())
                    == Some(package_name.to_string())
        }
        TypeOwner::None => false,
    }
}

fn init_source(name: &str) -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let source = Path::new("test-data/wit").join(name);

    fs_extra::dir::copy(
        source,
        temp_dir.path(),
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .unwrap();

    temp_dir
}
