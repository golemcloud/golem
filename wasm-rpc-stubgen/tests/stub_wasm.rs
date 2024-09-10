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

use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunctionParameter, AnalysedInstance, AnalysedResourceId,
    AnalysedResourceMode, AnalysedType, AnalysisContext, NameOptionTypePair, NameTypePair,
    TypeBool, TypeChr, TypeEnum, TypeF32, TypeF64, TypeFlags, TypeHandle, TypeList, TypeOption,
    TypeRecord, TypeResult, TypeS16, TypeS32, TypeS64, TypeS8, TypeStr, TypeTuple, TypeU16,
    TypeU32, TypeU64, TypeU8, TypeVariant,
};
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use golem_wasm_rpc_stubgen::cargo::generate_cargo_toml;
use golem_wasm_rpc_stubgen::compilation::compile;
use golem_wasm_rpc_stubgen::rust::generate_stub_source;
use golem_wasm_rpc_stubgen::stub::StubDefinition;
use golem_wasm_rpc_stubgen::wit::{copy_wit_files, generate_stub_wit};
use golem_wasm_rpc_stubgen::WasmRpcOverride;
use heck::ToSnakeCase;
use std::path::Path;
use tempfile::tempdir;

///! Tests in this module are verifying the STUB WASM created by the stub generator
///! regardless of how the actual wasm generator is implemented. (Currently generates Rust code and compiles it)

#[tokio::test]
async fn all_wit_types() {
    // TODO: extract some of to the main `build` module
    let source_wit_root = Path::new("test-data/all-wit-types");
    let target_root = tempdir().unwrap();
    let canonical_target_root = target_root.path().canonicalize().unwrap();

    let def = StubDefinition::new(
        source_wit_root,
        target_root.path(),
        &None,
        "1.0.0",
        &WasmRpcOverride {
            wasm_rpc_version_override: None,
            wasm_rpc_path_override: Some(
                std::env::current_dir()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("wasm-rpc")
                    .to_string_lossy()
                    .to_string(),
            ),
        },
        false,
    )
    .unwrap();
    generate_stub_wit(&def).unwrap();
    copy_wit_files(&def).unwrap();
    let _ = def.verify_target_wits().unwrap();

    generate_cargo_toml(&def).unwrap();
    generate_stub_source(&def).unwrap();
    compile(&canonical_target_root).await.unwrap();

    let wasm_path = canonical_target_root
        .join("target")
        .join("wasm32-wasi")
        .join("release")
        .join(format!(
            "{}.wasm",
            def.target_crate_name().unwrap().to_snake_case()
        ));

    let stub_bytes = std::fs::read(wasm_path).unwrap();
    let stub_component = Component::<IgnoreAllButMetadata>::from_bytes(&stub_bytes).unwrap();

    let state = AnalysisContext::new(stub_component);
    let stub_exports = state.get_top_level_exports().unwrap();

    assert_eq!(stub_exports.len(), 1);
    let AnalysedExport::Instance(exported_interface) = &stub_exports[0] else {
        panic!("unexpected export type")
    };

    assert_eq!(exported_interface.name, "test:main-stub/stub-api");

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

// TODO: move these helpers to golem-wasm-ast

fn field(name: &str, typ: AnalysedType) -> NameTypePair {
    NameTypePair {
        name: name.to_string(),
        typ,
    }
}

fn case(name: &str, typ: AnalysedType) -> NameOptionTypePair {
    NameOptionTypePair {
        name: name.to_string(),
        typ: Some(typ),
    }
}

fn unit_case(name: &str) -> NameOptionTypePair {
    NameOptionTypePair {
        name: name.to_string(),
        typ: None,
    }
}

fn bool() -> AnalysedType {
    AnalysedType::Bool(TypeBool)
}

fn s8() -> AnalysedType {
    AnalysedType::S8(TypeS8)
}

fn s16() -> AnalysedType {
    AnalysedType::S16(TypeS16)
}

fn s32() -> AnalysedType {
    AnalysedType::S32(TypeS32)
}

fn s64() -> AnalysedType {
    AnalysedType::S64(TypeS64)
}

fn u8() -> AnalysedType {
    AnalysedType::U8(TypeU8)
}

fn u16() -> AnalysedType {
    AnalysedType::U16(TypeU16)
}

fn u32() -> AnalysedType {
    AnalysedType::U32(TypeU32)
}

fn u64() -> AnalysedType {
    AnalysedType::U64(TypeU64)
}

fn f32() -> AnalysedType {
    AnalysedType::F32(TypeF32)
}

fn f64() -> AnalysedType {
    AnalysedType::F64(TypeF64)
}

fn chr() -> AnalysedType {
    AnalysedType::Chr(TypeChr)
}

fn str() -> AnalysedType {
    AnalysedType::Str(TypeStr)
}

fn list(inner: AnalysedType) -> AnalysedType {
    AnalysedType::List(TypeList {
        inner: Box::new(inner),
    })
}

fn option(inner: AnalysedType) -> AnalysedType {
    AnalysedType::Option(TypeOption {
        inner: Box::new(inner),
    })
}

fn flags(names: &[&str]) -> AnalysedType {
    AnalysedType::Flags(TypeFlags {
        names: names.iter().map(|n| n.to_string()).collect(),
    })
}

fn r#enum(cases: &[&str]) -> AnalysedType {
    AnalysedType::Enum(TypeEnum {
        cases: cases.iter().map(|n| n.to_string()).collect(),
    })
}

fn tuple(items: Vec<AnalysedType>) -> AnalysedType {
    AnalysedType::Tuple(TypeTuple { items })
}

fn result(ok: AnalysedType, err: AnalysedType) -> AnalysedType {
    AnalysedType::Result(TypeResult {
        ok: Some(Box::new(ok)),
        err: Some(Box::new(err)),
    })
}

fn result_ok(ok: AnalysedType) -> AnalysedType {
    AnalysedType::Result(TypeResult {
        ok: Some(Box::new(ok)),
        err: None,
    })
}

fn result_err(err: AnalysedType) -> AnalysedType {
    AnalysedType::Result(TypeResult {
        ok: None,
        err: Some(Box::new(err)),
    })
}

fn record(fields: Vec<NameTypePair>) -> AnalysedType {
    AnalysedType::Record(TypeRecord { fields })
}

fn variant(cases: Vec<NameOptionTypePair>) -> AnalysedType {
    AnalysedType::Variant(TypeVariant { cases })
}

fn assert_has_rpc_resource_constructor(exported_interface: &AnalysedInstance, name: &str) {
    let fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == format!("[constructor]{name}"))
        .expect(format!("missing constructor for {name}").as_str());

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
        .expect(format!("missing constructor for {resource_name}").as_str());

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
        .expect(format!("missing async function {async_fun_name}").as_str());
    let blocking_fun = exported_interface
        .functions
        .iter()
        .find(|f| f.name == blocking_fun_name)
        .expect(format!("missing blocking function {blocking_fun_name}").as_str());

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

    let parameters_with_self = vec![
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

        assert_valid_polling_resource(&exported_interface, async_result_resource_id, return_type);
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
        .filter(|r| r.is_method() && r.parameters.len() >= 1)
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
