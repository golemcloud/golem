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

//! Tests in this module are verifying the STUB WASM created by the stub generator
//! regardless of how the actual wasm generator is implemented. (Currently generates Rust code and compiles it)

use crate::stubgen::{golem_rust_override, test_data_path};
use fs_extra::dir::CopyOptions;
use golem_cli::model::app::AppComponentName;
use golem_cli::wasm_rpc_stubgen::commands::generate::generate_and_build_client;
use golem_cli::wasm_rpc_stubgen::stub::{StubConfig, StubDefinition};
use golem_wasm_ast::analysis::analysed_type::*;
use golem_wasm_ast::analysis::wit_parser::WitAnalysisContext;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunctionParameter, AnalysedInstance, AnalysedResourceId,
    AnalysedResourceMode, AnalysedType, TypeHandle,
};
use tempfile::tempdir;
use test_r::test;

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
        golem_rust_override: golem_rust_override(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:component"),
        is_ephemeral: false,
    })
    .unwrap();

    let wasm_path = generate_and_build_client(&def, false).await.unwrap();

    let stub_bytes = std::fs::read(wasm_path).unwrap();
    let state = WitAnalysisContext::new(&stub_bytes).unwrap();
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
    ])
    .named("product-item")
    .owned("test:main-exports/iface1");
    let order = record(vec![
        field("order-id", str()),
        field("items", list(product_item.clone())),
        field("total", f32()),
        field("timestamp", u64()),
    ])
    .named("order")
    .owned("test:main-exports/iface1");

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

    let permissions = flags(&["read", "write", "exec", "close"])
        .named("permissions")
        .owned("test:main-exports/iface1");
    let metadata = record(vec![
        field("name", str()),
        field("origin", str()),
        field("perms", permissions.clone()),
    ])
    .named("metadata")
    .owned("test:main-exports/iface1");

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
    ])
    .named("point")
    .owned("test:main-exports/iface1");
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

    let order_confirmation = record(vec![field("order-id", str())])
        .named("order-confirmation")
        .owned("test:main-exports/iface1");
    let checkout_result = variant(vec![
        case("error", str()),
        case("success", order_confirmation.clone()),
        unit_case("unknown"),
    ])
    .named("checkout-result")
    .owned("test:main-exports/iface1");

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

    let color = r#enum(&["red", "green", "blue"])
        .named("color")
        .owned("test:main-exports/iface1");
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
        golem_rust_override: golem_rust_override(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:component"),
        is_ephemeral: false,
    })
    .unwrap();

    let wasm_path = generate_and_build_client(&def, false).await.unwrap();

    let stub_bytes = std::fs::read(wasm_path).unwrap();

    let state = WitAnalysisContext::new(&stub_bytes).unwrap();
    let stub_exports = state.get_top_level_exports().unwrap();

    assert_eq!(stub_exports.len(), 1);
    let AnalysedExport::Instance(exported_interface) = &stub_exports[0] else {
        panic!("unexpected export type")
    };

    assert_eq!(exported_interface.name, "test:main-client/api-client");

    assert_has_rpc_resource_constructor(exported_interface, "iface1");
    assert_has_resource(
        exported_interface,
        "resource1",
        &[AnalysedFunctionParameter {
            name: "name".to_string(),
            typ: str(),
        }],
    );
}

#[test]
async fn circular_resources() {
    let source = test_data_path().join("wit/circular-resources");
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
        golem_rust_override: golem_rust_override(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:main"),
        is_ephemeral: false,
    })
    .unwrap();

    let wasm_path = generate_and_build_client(&def, false).await.unwrap();

    let stub_bytes = std::fs::read(wasm_path).unwrap();
    let state = WitAnalysisContext::new(&stub_bytes).unwrap();
    let stub_exports = state.get_top_level_exports().unwrap();

    assert_eq!(stub_exports.len(), 1);
    let AnalysedExport::Instance(exported_interface) = &stub_exports[0] else {
        panic!("unexpected export type")
    };

    assert_eq!(exported_interface.name, "test:main-client/api-client");

    assert_has_rpc_resource_constructor(exported_interface, "iface");
    assert_has_resource(
        exported_interface,
        "resource1",
        &[AnalysedFunctionParameter {
            name: "name".to_string(),
            typ: str(),
        }],
    );
    assert_has_resource(
        exported_interface,
        "resource2",
        &[AnalysedFunctionParameter {
            name: "name".to_string(),
            typ: str(),
        }],
    );
}

#[test]
#[ignore] // wit parser currently fails on inline types and resources with `Type not part of an interface`
async fn inline_resources() {
    let source = test_data_path().join("wit/inline-resources");
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
        golem_rust_override: golem_rust_override(),
        extract_source_exports_package: true,
        seal_cargo_workspace: false,
        component_name: AppComponentName::from("test:main"),
        is_ephemeral: false,
    })
    .unwrap();

    let wasm_path = generate_and_build_client(&def, false).await.unwrap();

    let stub_bytes = std::fs::read(wasm_path).unwrap();
    let state = WitAnalysisContext::new(&stub_bytes).unwrap();
    let stub_exports = state.get_top_level_exports().unwrap();

    assert_eq!(stub_exports.len(), 1);
    let AnalysedExport::Instance(exported_interface) = &stub_exports[0] else {
        panic!("unexpected export type")
    };

    assert_eq!(exported_interface.name, "test:main-client/api-client");
    assert_has_resource(
        exported_interface,
        "resource1",
        &[AnalysedFunctionParameter {
            name: "name".to_string(),
            typ: str(),
        }],
    );
}

fn assert_has_rpc_resource_constructor(exported_interface: &AnalysedInstance, name: &str) {
    let fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == format!("[constructor]{name}"))
        .unwrap_or_else(|| panic!("missing constructor for {name}"));

    assert!(fun.result.is_some());
    assert!(matches!(
        fun.result.as_ref().unwrap().typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));
    assert_eq!(
        fun.parameters,
        vec![AnalysedFunctionParameter {
            name: "worker-name".to_string(),
            typ: str()
        }]
    );

    let custom_fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == format!("[static]{name}.custom"))
        .unwrap_or_else(|| panic!("missing custom constructor for {name}"));

    assert!(custom_fun.result.is_some());
    assert!(matches!(
        custom_fun.result.as_ref().unwrap().typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));
    assert_eq!(
        custom_fun.parameters,
        vec![AnalysedFunctionParameter {
            name: "worker-id".to_string(),
            typ: record(vec![
                field(
                    "component-id",
                    record(vec![field(
                        "uuid",
                        record(vec![field("high-bits", u64()), field("low-bits", u64()),])
                            .named("uuid")
                            .owned("golem:rpc@0.2.2/types")
                    )])
                    .named("component-id")
                    .owned("golem:rpc@0.2.2/types")
                ),
                field("worker-name", str()),
            ])
            .named("worker-id")
            .owned("golem:rpc@0.2.2/types")
        }]
    );
}

fn assert_has_resource(
    exported_interface: &AnalysedInstance,
    name: &str,
    constructor_parameters: &[AnalysedFunctionParameter],
) {
    let fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == format!("[constructor]{name}"))
        .unwrap_or_else(|| panic!("missing constructor for {name}"));

    assert!(fun.result.is_some());
    assert!(matches!(
        fun.result.as_ref().unwrap().typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));
    assert_eq!(
        fun.parameters,
        [
            vec![AnalysedFunctionParameter {
                name: "worker-name".to_string(),
                typ: str()
            }],
            constructor_parameters.to_vec()
        ]
        .concat()
    );

    let custom_fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == format!("[static]{name}.custom"))
        .unwrap_or_else(|| panic!("missing custom constructor for {name}"));

    assert!(custom_fun.result.is_some());
    assert!(matches!(
        &custom_fun.result.as_ref().unwrap().typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));
    assert_eq!(
        custom_fun.parameters,
        [
            vec![AnalysedFunctionParameter {
                name: "worker-id".to_string(),
                typ: record(vec![
                    field(
                        "component-id",
                        record(vec![field(
                            "uuid",
                            record(vec![field("high-bits", u64()), field("low-bits", u64()),])
                                .named("uuid")
                                .owned("golem:rpc@0.2.2/types")
                        ),])
                        .named("component-id")
                        .owned("golem:rpc@0.2.2/types")
                    ),
                    field("worker-name", str()),
                ])
                .named("worker-id")
                .owned("golem:rpc@0.2.2/types")
            }],
            constructor_parameters.to_vec()
        ]
        .concat()
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

    let resource_id = match &constructor.result.as_ref().unwrap().typ {
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            resource_id,
            ..
        }) => *resource_id,
        _ => panic!("unexpected constructor return type"),
    };

    let async_fun_name = format!("[method]{resource_name}.{function_name}");
    let blocking_fun_name = format!("[method]{resource_name}.blocking-{function_name}");
    let scheduled_fun_name = format!("[method]{resource_name}.schedule-{function_name}");

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
    let scheduled_fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == scheduled_fun_name)
        .unwrap_or_else(|| panic!("missing scheduled function {scheduled_fun_name}"));

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
    let scheduled_parameter_types = scheduled_fun
        .parameters
        .iter()
        .map(|p| &p.typ)
        .cloned()
        .collect::<Vec<_>>();

    let parameters_with_self = [
        vec![AnalysedType::Handle(TypeHandle {
            resource_id,
            mode: AnalysedResourceMode::Borrowed,
            name: None,
            owner: None,
        })
        .named("iface1")
        .owned("test:main-client/api-client")],
        parameters,
    ]
    .concat();

    let scheduled_function_parameters = [
        parameters_with_self.clone(),
        // schedule_for parameter
        vec![
            record(vec![field("seconds", u64()), field("nanoseconds", u32())])
                .named("datetime")
                .owned("wasi:clocks@0.2.3/wall-clock"),
        ],
    ]
    .concat();

    assert_eq!(async_parameter_types, parameters_with_self);
    assert_eq!(blocking_parameter_types, parameters_with_self);
    assert_eq!(scheduled_parameter_types, scheduled_function_parameters);

    if let Some(return_type) = return_type {
        assert!(async_fun.result.is_some());
        assert!(blocking_fun.result.is_some());
        assert_eq!(blocking_fun.result.as_ref().unwrap().typ, return_type);

        let async_result_resource_id = match &async_fun.result.as_ref().unwrap().typ {
            AnalysedType::Handle(TypeHandle {
                mode: AnalysedResourceMode::Owned,
                resource_id,
                name: Some(_),
                owner: Some(_),
            }) => *resource_id,
            _ => panic!("unexpected async result return type"),
        };

        assert_valid_polling_resource(exported_interface, async_result_resource_id, return_type);
    } else {
        assert!(async_fun.result.is_none());
        assert!(blocking_fun.result.is_none());
    }

    assert!(scheduled_fun.result.is_some());
    assert!(matches!(
        scheduled_fun.result.as_ref().unwrap().typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));
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
            matches!(
                &r.parameters[0].typ,
                &AnalysedType::Handle(TypeHandle {
                    resource_id: id,
                    mode: AnalysedResourceMode::Borrowed,
                    name: Some(_),
                    owner: Some(_),
                }) if id == resource_id
            )
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

    assert!(subscribe_function.result.is_some());
    assert!(matches!(
        subscribe_function.result.as_ref().unwrap().typ,
        AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            ..
        })
    ));

    assert!(get_function.result.is_some());
    assert_eq!(
        get_function.result.as_ref().unwrap().typ,
        option(return_type)
    );
}
