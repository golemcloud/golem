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

//! Tests in this module are verifying the STUB WASM created by the stub generator
//! regardless of how the actual wasm generator is implemented. (Currently generates Rust code and compiles it)

use fs_extra::dir::CopyOptions;
use test_r::test;

use golem_wasm_ast::analysis::analysed_type::*;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunctionParameter, AnalysedInstance, AnalysedResourceId,
    AnalysedResourceMode, AnalysedType, AnalysisContext, NameTypePair, TypeHandle, TypeOption,
    TypeRecord, TypeStr,
};
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use golem_wasm_rpc_stubgen::commands::generate::generate_and_build_client;
use golem_wasm_rpc_stubgen::stub::{StubConfig, StubDefinition};
use tempfile::tempdir;
use wasm_rpc_stubgen_tests_integration::{test_data_path, wasm_rpc_override};

test_r::enable!();

#[test]
async fn all_wit_types() {
    let source = test_data_path().join("wit/all-wit-types");
    let source_wit_root = tempdir().unwrap();

    fs_extra::dir::copy(
        source,
        source_wit_root.path(),
        &CopyOptions::new().content_only(true),
    )
    .unwrap();

    let target_root = tempdir().unwrap();
    let canonical_target_root = target_root.path().canonicalize().unwrap();

    let def = StubDefinition::new(StubConfig {
        source_wit_root: source_wit_root.path().to_path_buf(),
        client_root: canonical_target_root,
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        wasm_rpc_override: wasm_rpc_override(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
    })
    .unwrap();

    let wasm_path = generate_and_build_client(&def, false).await.unwrap();

    let stub_bytes = std::fs::read(wasm_path).unwrap();
    let stub_component = Component::<IgnoreAllButMetadata>::from_bytes(&stub_bytes).unwrap();

    let state = AnalysisContext::new(stub_component);
    let stub_exports = state.get_top_level_exports().unwrap();

    assert_eq!(stub_exports.len(), 1);
    let AnalysedExport::Instance(exported_interface) = &stub_exports[0] else {
        panic!("unexpected export type")
    };

    assert_eq!(exported_interface.name, "test:main-client/api-client");

    for fun in &exported_interface.functions {
        println!("Function: {}", fun.name);
    }

    assert_has_rpc_resource_constructor(exported_interface, "iface1");
    assert_has_stub(exported_interface, "iface1", "no-op", vec![], None);
    assert_has_stub(
        exported_interface,
        "iface1",
        "get-bool",
        vec![],
        Some(bool()),
    );
    assert_has_stub(exported_interface, "iface1", "set-bool", vec![bool()], None);

    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-bool",
        vec![bool()],
        Some(bool()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-s8",
        vec![s8()],
        Some(s8()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-s16",
        vec![s16()],
        Some(s16()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-s32",
        vec![s32()],
        Some(s32()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-s64",
        vec![s64()],
        Some(s64()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-u8",
        vec![u8()],
        Some(u8()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-u16",
        vec![u16()],
        Some(u16()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-u32",
        vec![u32()],
        Some(u32()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-u64",
        vec![u64()],
        Some(u64()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-f32",
        vec![f32()],
        Some(f32()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-f64",
        vec![f64()],
        Some(f64()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-char",
        vec![chr()],
        Some(chr()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "identity-string",
        vec![str()],
        Some(str()),
    );

    let product_item = record(vec![
        field("product-id", str()),
        field("name", str()),
        field("price", f32()),
        field("quantity", u32()),
    ]);
    let order = record(vec![
        field("order-id", str()),
        field("items", list(product_item.clone())),
        field("total", f32()),
        field("timestamp", u64()),
    ]);

    assert_has_stub(
        exported_interface,
        "iface1",
        "get-orders",
        vec![],
        Some(list(order.clone())),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "set-orders",
        vec![list(order.clone())],
        None,
    );

    let permissions = flags(&["read", "write", "exec", "close"]);
    let metadata = record(vec![
        field("name", str()),
        field("origin", str()),
        field("perms", permissions.clone()),
    ]);

    assert_has_stub(
        exported_interface,
        "iface1",
        "apply-metadata",
        vec![option(metadata.clone())],
        Some(option(metadata.clone())),
    );

    assert_has_stub(
        exported_interface,
        "iface1",
        "get-option-bool",
        vec![],
        Some(option(bool())),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "set-option-bool",
        vec![option(bool())],
        None,
    );

    assert_has_stub(
        exported_interface,
        "iface1",
        "get-coordinates",
        vec![],
        Some(tuple(vec![s32(), s32()])),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "set-coordinates",
        vec![tuple(vec![s32(), s32()])],
        None,
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "get-coordinates-alias",
        vec![],
        Some(tuple(vec![s32(), s32()])),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "set-coordinates-alias",
        vec![tuple(vec![s32(), s32()])],
        None,
    );

    let point = record(vec![
        field("x", s32()),
        field("y", s32()),
        field("metadata", metadata.clone()),
    ]);
    assert_has_stub(
        exported_interface,
        "iface1",
        "tuple-to-point",
        vec![tuple(vec![s32(), s32()]), option(metadata.clone())],
        Some(result(point.clone(), str())),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "pt-log-error",
        vec![result(point.clone(), str())],
        Some(result_ok(point.clone())),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "validate-pt",
        vec![point.clone()],
        Some(result_err(str())),
    );

    let order_confirmation = record(vec![field("order-id", str())]);
    let checkout_result = variant(vec![
        case("error", str()),
        case("success", order_confirmation.clone()),
        unit_case("unknown"),
    ]);

    assert_has_stub(
        exported_interface,
        "iface1",
        "print-checkout-result",
        vec![checkout_result.clone()],
        Some(str()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "get-checkout-result",
        vec![],
        Some(checkout_result.clone()),
    );

    let color = r#enum(&["red", "green", "blue"]);
    assert_has_stub(
        exported_interface,
        "iface1",
        "get-color",
        vec![],
        Some(color.clone()),
    );
    assert_has_stub(
        exported_interface,
        "iface1",
        "set-color",
        vec![color.clone()],
        None,
    );

    assert_has_stub(
        exported_interface,
        "iface1",
        "validate-permissions",
        vec![permissions.clone()],
        Some(permissions.clone()),
    );
}

#[test]
async fn resource() {
    let source = test_data_path().join("wit/resources");
    let source_wit_root = tempdir().unwrap();

    fs_extra::dir::copy(
        source,
        source_wit_root.path(),
        &CopyOptions::new().content_only(true),
    )
    .unwrap();

    let target_root = tempdir().unwrap();
    let canonical_target_root = target_root.path().canonicalize().unwrap();

    let def = StubDefinition::new(StubConfig {
        source_wit_root: source_wit_root.path().to_path_buf(),
        client_root: canonical_target_root,
        selected_world: None,
        stub_crate_version: "1.0.0".to_string(),
        wasm_rpc_override: wasm_rpc_override(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
    })
    .unwrap();

    let wasm_path = generate_and_build_client(&def, false).await.unwrap();

    let stub_bytes = std::fs::read(wasm_path).unwrap();
    let stub_component = Component::<IgnoreAllButMetadata>::from_bytes(&stub_bytes).unwrap();

    let state = AnalysisContext::new(stub_component);
    let stub_exports = state.get_top_level_exports().unwrap();

    assert_eq!(stub_exports.len(), 1);
    let AnalysedExport::Instance(exported_interface) = &stub_exports[0] else {
        panic!("unexpected export type")
    };

    assert_eq!(exported_interface.name, "test:main-client/api-client");

    for fun in &exported_interface.functions {
        println!("Function: {}", fun.name);
    }

    assert_has_rpc_resource_constructor(exported_interface, "iface1");
    // TODO: asserts for "normal" resource
}

fn assert_has_rpc_resource_constructor(exported_interface: &AnalysedInstance, name: &str) {
    let fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == format!("[constructor]{name}"))
        .unwrap_or_else(|| panic!("missing constructor for {name}"));

    assert_eq!(fun.results.len(), 1);
    assert!(matches!(
        fun.results[0].typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));
    assert_eq!(
        fun.parameters,
        vec![AnalysedFunctionParameter {
            name: "location".to_string(),
            typ: AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "value".to_string(),
                    typ: AnalysedType::Str(TypeStr)
                }]
            })
        }]
    );
}

fn assert_has_stub(
    exported_interface: &AnalysedInstance,
    resource_name: &str,
    function_name: &str,
    parameters: Vec<AnalysedType>,
    return_type: Option<AnalysedType>,
) {
    let constructor = exported_interface
        .functions
        .iter()
        .find(|f| f.name == format!("[constructor]{resource_name}"))
        .unwrap_or_else(|| panic!("missing constructor for {resource_name}"));

    let resource_id = match &constructor.results[0].typ {
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            resource_id,
        }) => resource_id.clone(),
        _ => panic!("unexpected constructor return type"),
    };

    let async_fun_name = format!("[method]{resource_name}.{function_name}");
    let blocking_fun_name = format!("[method]{resource_name}.blocking-{function_name}");

    let async_fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == async_fun_name)
        .unwrap_or_else(|| panic!("missing async function {async_fun_name}"));
    let blocking_fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == blocking_fun_name)
        .unwrap_or_else(|| panic!("missing blocking function {blocking_fun_name}"));

    let async_parameter_types = async_fun
        .parameters
        .iter()
        .map(|p| &p.typ)
        .cloned()
        .collect::<Vec<_>>();
    let blocking_parameter_types = blocking_fun
        .parameters
        .iter()
        .map(|p| &p.typ)
        .cloned()
        .collect::<Vec<_>>();

    let parameters_with_self = [
        vec![AnalysedType::Handle(TypeHandle {
            resource_id,
            mode: AnalysedResourceMode::Borrowed,
        })],
        parameters,
    ]
    .concat();

    assert_eq!(async_parameter_types, parameters_with_self);
    assert_eq!(blocking_parameter_types, parameters_with_self);

    if let Some(return_type) = return_type {
        assert_eq!(async_fun.results.len(), 1);
        assert_eq!(blocking_fun.results.len(), 1);
        assert_eq!(blocking_fun.results[0].typ, return_type);

        let async_result_resource_id = match &async_fun.results[0].typ {
            AnalysedType::Handle(TypeHandle {
                mode: AnalysedResourceMode::Owned,
                resource_id,
            }) => resource_id.clone(),
            _ => panic!("unexpected async result return type"),
        };

        assert_valid_polling_resource(exported_interface, async_result_resource_id, return_type);
    } else {
        assert_eq!(async_fun.results.len(), 0);
        assert_eq!(blocking_fun.results.len(), 0);
    }
}

fn assert_valid_polling_resource(
    exported_interface: &AnalysedInstance,
    resource_id: AnalysedResourceId,
    return_type: AnalysedType,
) {
    let resource_methods = exported_interface
        .functions
        .iter()
        .filter(|r| r.is_method() && !r.parameters.is_empty())
        .filter(|r| {
            r.parameters[0].typ
                == AnalysedType::Handle(TypeHandle {
                    resource_id: resource_id.clone(),
                    mode: AnalysedResourceMode::Borrowed,
                })
        })
        .collect::<Vec<_>>();

    let subscribe_function = resource_methods
        .iter()
        .find(|f| f.name.ends_with(".subscribe"))
        .expect("missing subscribe function");
    let get_function = resource_methods
        .iter()
        .find(|f| f.name.ends_with(".get"))
        .expect("missing get function");

    assert_eq!(subscribe_function.results.len(), 1);
    assert!(matches!(
        subscribe_function.results[0].typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));

    assert_eq!(get_function.results.len(), 1);
    assert_eq!(
        get_function.results[0].typ,
        AnalysedType::Option(TypeOption {
            inner: Box::new(return_type)
        })
    );
}
