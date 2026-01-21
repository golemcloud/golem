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

use crate::bridge_gen::typescript::TypeScriptBridgeGenerator;
use crate::bridge_gen::BridgeGenerator;
use crate::model::agent::test::{
    code_first_snippets_agent_type, multi_agent_wrapper_2_types, single_agent_wrapper_types,
};
use camino::{Utf8Path, Utf8PathBuf};
use golem_client::model::ValueAndType;
use golem_common::model::agent::{
    AgentConstructor, AgentMethod, AgentMode, AgentType, AgentTypeName, BinaryReference,
    BinaryReferenceValue, BinarySource, BinaryType, ComponentModelElementSchema, DataSchema,
    ElementSchema, JsonComponentModelValue, NamedElementSchema, NamedElementSchemas, TextReference,
    TextReferenceValue, TextSource, UntypedJsonDataValue, UntypedJsonElementValue,
    UntypedJsonElementValues, UntypedJsonNamedElementValue, UntypedJsonNamedElementValues,
};
use golem_templates::model::GuestLanguage;
use golem_wasm::analysis::analysed_type::{
    bool, case, f64, field, list, option, r#enum, record, s32, str, tuple, unit_case, variant,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{IntoValueAndType, Value};
use heck::ToUpperCamelCase;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::io::Write;
use std::process::Stdio;
use tempfile::TempDir;
use test_r::{test, test_dep};

struct GeneratedPackage {
    pub dir: TempDir,
}

impl GeneratedPackage {
    pub fn new(agent_type: AgentType) -> Self {
        let dir = TempDir::new().unwrap();
        let target_dir = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::remove_dir_all(target_dir).ok();
        generate_and_compile(agent_type, target_dir);
        GeneratedPackage { dir }
    }
}

#[allow(dead_code)]
struct TestTypes {
    object_type: AnalysedType,
    union_type: AnalysedType,
    list_complex_type: AnalysedType,
    tuple_type: AnalysedType,
    tuple_complex_type: AnalysedType,
    map_type: AnalysedType,
    simple_interface_type: AnalysedType,
    object_complex_type: AnalysedType,
    union_complex_type: AnalysedType,
    tagged_union: AnalysedType,
    union_with_literals: AnalysedType,
    union_with_only_literals: AnalysedType,
    anonymous_union_with_only_literals: AnalysedType,
    object_with_union_undefined: AnalysedType,
}

impl Default for TestTypes {
    fn default() -> Self {
        let object_type = record(vec![
            field("a", str()),
            field("b", s32()),
            field("c", bool()),
        ]);

        let union_type = variant(vec![
            case("case1", s32()),
            case("case2", str()),
            case("case3", bool()),
            case("case4", object_type.clone()),
        ]);

        let list_complex_type = list(object_type.clone());

        let tuple_type = tuple(vec![str(), s32(), bool()]);

        let tuple_complex_type = tuple(vec![str(), s32(), object_type.clone()]);

        let map_type = list(tuple(vec![str(), s32()]));

        let simple_interface_type = record(vec![field("n", s32())]);

        let object_complex_type = record(vec![
            field("a", str()),
            field("b", s32()),
            field("c", bool()),
            field("d", object_type.clone()),
            field("e", union_type.clone()),
            field("f", list(str())),
            field("g", list(object_type.clone())),
            field("h", tuple_type.clone()),
            field("i", tuple_complex_type.clone()),
            field("j", map_type.clone()),
            field("k", simple_interface_type.clone()),
        ]);

        let union_complex_type = variant(vec![
            case("case1", s32()),
            case("case2", str()),
            case("case3", bool()),
            case("case4", object_complex_type.clone()),
            case("case5", union_type.clone()),
            case("case6", tuple_type.clone()),
            case("case7", tuple_complex_type.clone()),
            case("case8", simple_interface_type.clone()),
            case("case9", list(str())),
        ]);

        let tagged_union = variant(vec![
            case("a", str()),
            case("b", s32()),
            case("c", bool()),
            case("d", union_type.clone()),
            case("e", object_type.clone()),
            case("f", list(str())),
            case("g", tuple(vec![str(), s32(), bool()])),
            case("h", simple_interface_type.clone()),
            unit_case("i"),
            unit_case("j"),
        ]);

        let union_with_literals = variant(vec![
            unit_case("lit1"),
            unit_case("lit2"),
            unit_case("lit3"),
            case("union-with-literals1", bool()),
        ])
        .named("UnionWithLiterals");

        let union_with_only_literals =
            r#enum(&["foo", "bar", "baz"]).named("UnionWithOnlyLiterals");

        let anonymous_union_with_only_literals = r#enum(&["foo", "bar", "baz"]);

        let object_with_union_undefined = record(vec![field("a", option(str()))]);

        TestTypes {
            object_type,
            union_type,
            list_complex_type,
            tuple_type,
            tuple_complex_type,
            map_type,
            simple_interface_type,
            object_complex_type,
            union_complex_type,
            tagged_union,
            union_with_literals,
            union_with_only_literals,
            anonymous_union_with_only_literals,
            object_with_union_undefined,
        }
    }
}

#[test_dep(tagged_as = "single_agent_wrapper_types_1")]
fn ts_single_agent_wrapper_1() -> GeneratedPackage {
    GeneratedPackage::new(single_agent_wrapper_types()[0].clone())
}

#[test_dep(tagged_as = "multi_agent_wrapper_2_types_1")]
fn ts_multi_agent_wrapper_2_types_1() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone())
}

#[test_dep(tagged_as = "multi_agent_wrapper_2_types_2")]
fn ts_multi_agent_wrapper_2_types_2() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[1].clone())
}

#[test_dep(tagged_as = "counter_agent")]
fn ts_counter_agent() -> GeneratedPackage {
    let agent_type = AgentType {
        type_name: AgentTypeName("CounterAgent".to_string()),
        description: "Constructs the agent CounterAgent".to_string(),
        constructor: AgentConstructor {
            name: Some("CounterAgent".to_string()),
            description: "Constructs the agent CounterAgent".to_string(),
            prompt_hint: Some("Enter the following parameters: name".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "name".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: str(),
                    }),
                }],
            }),
        },
        methods: vec![AgentMethod {
            name: "increment".to_string(),
            description: "Increases the count by one and returns the new value".to_string(),
            prompt_hint: Some("Increase the count by one".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "return-value".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: f64(),
                    }),
                }],
            }),
            http_endpoint: vec![],
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    };

    GeneratedPackage::new(agent_type)
}

#[test_dep(tagged_as = "ts_code_first_snippets_foo_agent")]
fn ts_code_first_snippets_foo_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::TypeScript,
        "FooAgent",
    ))
}

#[test_dep(tagged_as = "ts_code_first_snippets_bar_agent")]
fn ts_code_first_snippets_bar_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::TypeScript,
        "BarAgent",
    ))
}

#[test_dep(tagged_as = "rust_code_first_snippets_foo_agent")]
fn rust_code_first_snippets_foo_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::Rust,
        "FooAgent",
    ))
}

#[test_dep(tagged_as = "rust_code_first_snippets_bar_agent")]
fn rust_code_first_snippets_bar_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::Rust,
        "BarAgent",
    ))
}

#[test]
fn single_agent_wrapper_1_compiles(
    #[tagged_as("single_agent_wrapper_types_1")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn multi_agent_wrapper_2_types_1_compiles(
    #[tagged_as("multi_agent_wrapper_2_types_1")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn multi_agent_wrapper_2_types_2_compiles(
    #[tagged_as("multi_agent_wrapper_2_types_2")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn counter_agent_compiles(#[tagged_as("counter_agent")] _pkg: &GeneratedPackage) {}

#[test]
fn code_first_snippets_ts_foo_agent_compiles(
    #[tagged_as("ts_code_first_snippets_foo_agent")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn code_first_snippets_ts_bar_agent_compiles(
    #[tagged_as("ts_code_first_snippets_bar_agent")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn code_first_snippets_rust_foo_agent_compiles(
    #[tagged_as("rust_code_first_snippets_foo_agent")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn code_first_snippets_rust_bar_agent_compiles(
    #[tagged_as("ts_code_first_snippets_bar_agent")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn bridge_tests_optional_q_mark(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunOptionalQMark",
        json! {
            [
                "value1",
                10,
                null
            ]
        },
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: "value1".into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some(10i32).into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: None::<i32>.into_value_and_type().to_json_value().unwrap(),
                }),
            ],
        }),
    );
}

#[test]
fn bridge_tests_optional(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunOptional",
        // [string | undefined, ObjectWithUnionWithUndefined1, ObjectWithUnionWithUndefined2, ObjectWithUnionWithUndefined3, ObjectWithUnionWithUndefined4, string | undefined, UnionType | undefined]
        json!(["value", {"a": "nested"}, {"a": "nested"}, {"a": "nested"}, {"a": "nested"}, "optional", null]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some("value".to_string())
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        types.object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        types.object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        types.object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        types.object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some("optional".to_string())
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(Value::Option(None), option(types.union_type.clone()))
                        .to_json_value()
                        .unwrap(),
                }),
            ],
        }),
    );
}

#[test]
fn bridge_tests_number(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunNumber",
        json!([42]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: 42i32.into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_string(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunString",
        json!(["hello"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "hello".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_boolean(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunBoolean",
        json!([true]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: true.into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_void_return(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunVoidReturn",
        json!(["test"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_tuple_complex_type_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_output_decoding(
        target_dir,
        "FunTupleComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json! {
                        [
                            "hello".into_value_and_type().to_json_value().unwrap(),
                            100i32.into_value_and_type().to_json_value().unwrap(),
                            ValueAndType::new(
                                Value::Record(vec![
                                    Value::String("x".to_string()),
                                    Value::S32(200),
                                    Value::Bool(true),
                                ]),
                                record(vec![
                                    field("a", str()),
                                    field("b", s32()),
                                    field("c", bool()),
                                ]),
                            )
                            .to_json_value()
                            .unwrap()
                        ]
                    },
                },
            )],
        }),
        json!(["hello", 100, { "a": "x", "b": 200, "c": true }]),
    );
}

#[test]
fn bridge_tests_optionalqmark(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunOptionalQMark",
        json! {
            [
                "value1",
                10,
                null
            ]
        },
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: "value1".into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some(10i32).into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: None::<i32>.into_value_and_type().to_json_value().unwrap(),
                }),
            ],
        }),
    );
}

#[test]
fn bridge_tests_objectcomplextype(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunObjectComplexType",
        json!([{
            "a": "test",
            "b": 1,
            "c": true,
            "d": {
                "a": "nested",
                "b": 2,
                "c": false
            },
            "e": { "tag": "case1", "val": 5 },
            "f": ["item"],
            "g": [{"a": "obj", "b": 1, "c": true}],
            "h": ["str", 2, false],
            "i": ["str", 2, {"a": "obj", "b": 1, "c": true}],
            "j": [],
            "k": {"n": 1}}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("test".to_string()),
                            Value::S32(1),
                            Value::Bool(true),
                            Value::Record(vec![
                                Value::String("nested".to_string()),
                                Value::S32(2),
                                Value::Bool(false),
                            ]),
                            Value::Variant {
                                case_idx: 0,
                                case_value: Some(Box::new(Value::S32(5))),
                            },
                            Value::List(vec![Value::String("item".to_string())]),
                            Value::List(vec![Value::Record(vec![
                                Value::String("obj".to_string()),
                                Value::S32(1),
                                Value::Bool(true),
                            ])]),
                            Value::Tuple(vec![
                                Value::String("str".to_string()),
                                Value::S32(2),
                                Value::Bool(false),
                            ]),
                            Value::Tuple(vec![
                                Value::String("str".to_string()),
                                Value::S32(2),
                                Value::Record(vec![
                                    Value::String("obj".to_string()),
                                    Value::S32(1),
                                    Value::Bool(true),
                                ]),
                            ]),
                            Value::List(vec![]),
                            Value::Record(vec![Value::S32(1)]),
                        ]),
                        types.object_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_uniontype(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunUnionType",
        json!([{ "tag": "case3", "val": true }]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 2,
                            case_value: Some(Box::new(Value::Bool(true))),
                        },
                        types.union_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_unioncomplextype(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunUnionComplexType",
        json!([{ "tag": "case8", "val": { "n": 1 } }]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 7,
                            case_value: Some(Box::new(Value::Record(vec![Value::S32(1)]))),
                        },
                        types.union_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_map(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunMap",
        json!([[["key1", 10]]]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: vec![("key1".to_string(), 10i32)]
                        .into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>()
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_taggedunion(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunTaggedUnion",
        json!([{"tag": "e", "val": {"a": "x", "b": 200, "c": true }}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 4,
                            case_value: Some(Box::new(Value::Record(vec![
                                Value::String("x".to_string()),
                                Value::S32(200),
                                Value::Bool(true),
                            ]))),
                        },
                        types.tagged_union.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_tuplecomplextype(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunTupleComplexType",
        json!([["hello", 100, {"a": "x", "b": 200, "c": true}]]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Tuple(vec![
                            Value::String("hello".to_string()),
                            Value::S32(100),
                            Value::Record(vec![
                                Value::String("x".to_string()),
                                Value::S32(200),
                                Value::Bool(true),
                            ]),
                        ]),
                        types.tuple_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_tupletype(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunTupleType",
        json!([["item", 42, false]]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ("item".to_string(), 42, false)
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_listcomplextype(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunListComplexType",
        json!([[{"a": "item1", "b": 1, "c": true}, {"a": "item2", "b": 2, "c": false}]]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::List(vec![
                            Value::Record(vec![
                                Value::String("item1".to_string()),
                                Value::S32(1),
                                Value::Bool(true),
                            ]),
                            Value::Record(vec![
                                Value::String("item2".to_string()),
                                Value::S32(2),
                                Value::Bool(false),
                            ]),
                        ]),
                        types.list_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_objecttype(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunObjectType",
        json!([{"a": "test", "b": 123, "c": true}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("test".to_string()),
                            Value::S32(123),
                            Value::Bool(true),
                        ]),
                        types.object_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_unionwithliterals(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunUnionWithLiterals",
        json!([{"tag": "lit1"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 0,
                            case_value: None,
                        },
                        types.union_with_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_unionwithliterals_with_value(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunUnionWithLiterals",
        json!([{"tag": "union-with-literals1", "val": true}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 3,
                            case_value: Some(Box::new(Value::Bool(true))),
                        },
                        types.union_with_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_unionwithonlyliterals(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunUnionWithOnlyLiterals",
        json!(["bar"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Enum(1),
                        types.union_with_only_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_anyonymousunionwithonlyliterals(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_input_encoding(
        target_dir,
        "FunAnonymousUnionWithOnlyLiterals",
        json!(["bar"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Enum(1),
                        types.anonymous_union_with_only_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_voidreturn(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunVoidReturn",
        json!(["test"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_nullreturn(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunNullReturn",
        json!(["test"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_undefinedreturn(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunUndefinedReturn",
        json!(["test"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_unstructuredtext(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunUnstructuredText",
        json!([{ "tag": "inline", "val": "plain text"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::UnstructuredText(
                TextReferenceValue {
                    value: TextReference::Inline(TextSource {
                        data: "plain text".to_string(),
                        text_type: None,
                    }),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_unstructuredbinary(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunUnstructuredBinary",
        json!([{ "tag": "inline", "mimeType": "application/json", "val": [0,1,2,3]}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::UnstructuredBinary(
                BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        binary_type: BinaryType {
                            mime_type: "application/json".to_string(),
                        },
                        data: vec![0, 1, 2, 3],
                    }),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_multimodal(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunMultimodal",
        json!([[{"type": "text", "value": {"tag": "inline", "val": "hello"}}]]),
        UntypedJsonDataValue::Multimodal(UntypedJsonNamedElementValues {
            elements: vec![UntypedJsonNamedElementValue {
                name: "text".to_string(),
                value: UntypedJsonElementValue::UnstructuredText(TextReferenceValue {
                    value: TextReference::Inline(TextSource {
                        data: "hello".to_string(),
                        text_type: None,
                    }),
                }),
            }],
        }),
    );
}

#[test]
fn bridge_tests_multimodaladvanced(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunMultimodalAdvanced",
        json!([[{"type": "text", "value": "input"}]]),
        UntypedJsonDataValue::Multimodal(UntypedJsonNamedElementValues {
            elements: vec![UntypedJsonNamedElementValue {
                name: "text".to_string(),
                value: UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: "input".into_value_and_type().to_json_value().unwrap(),
                }),
            }],
        }),
    );
}

#[test]
fn bridge_tests_eitheroptional(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunEitherOptional",
        json!([{"ok": "value"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"ok": "value", "err": null}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_resultexact(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunResultExact",
        json!([{"tag": "ok", "val": "value"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"ok": "value"}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_resultlike(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunResultLike",
        json!([{"tag": "okay", "val": "value"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"okay": "value"}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_resultlikewithvoid(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunResultLikeWithVoid",
        json!([{"tag": "ok"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"ok": null}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultvs(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunBuiltinResultVS",
        json!(["hello"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "hello".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultsv(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunBuiltinResultSV",
        json!(["hello"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "hello".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultsn(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunBuiltinResultSN",
        json!(["hello"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "hello".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_noreturn(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunNoReturn",
        json!(["test"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_arrowsync(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_input_encoding(
        target_dir,
        "FunArrowSync",
        json!(["test"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_tuplecomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_output_decoding(
        target_dir,
        "FunTupleComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                // single return value containing a tuple
                JsonComponentModelValue {
                    value: json! {
                        [
                            "hello".into_value_and_type().to_json_value().unwrap(),
                            100i32.into_value_and_type().to_json_value().unwrap(),
                            ValueAndType::new(
                                Value::Record(vec![
                                    Value::String("x".to_string()),
                                    Value::S32(200),
                                    Value::Bool(true),
                                ]),
                                record(vec![
                                    field("a", str()),
                                    field("b", s32()),
                                    field("c", bool()),
                                ]),
                            )
                            .to_json_value()
                            .unwrap()
                        ]
                    },
                },
            )],
        }),
        json!(["hello", 100, { "a": "x", "b": 200, "c": true }]),
    );
}

#[test]
fn bridge_tests_tupletype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let bool_val: bool = false;

    assert_function_output_decoding(
        target_dir,
        "FunTupleType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json! {
                        [
                            "item".into_value_and_type().to_json_value().unwrap(),
                            42i32.into_value_and_type().to_json_value().unwrap(),
                            bool_val.into_value_and_type().to_json_value().unwrap(),
                        ]
                    },
                },
            )],
        }),
        json!(["item", 42, false]),
    );
}

#[test]
fn bridge_tests_listcomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunListComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::List(vec![
                            Value::Record(vec![
                                Value::String("item1".to_string()),
                                Value::S32(1),
                                Value::Bool(true),
                            ]),
                            Value::Record(vec![
                                Value::String("item2".to_string()),
                                Value::S32(2),
                                Value::Bool(false),
                            ]),
                        ]),
                        types.list_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!([{"a": "item1", "b": 1, "c": true}, {"a": "item2", "b": 2, "c": false}]),
    );
}

#[test]
fn bridge_tests_objecttype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunObjectType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("test".to_string()),
                            Value::S32(123),
                            Value::Bool(true),
                        ]),
                        types.object_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"a": "test", "b": 123, "c": true}),
    );
}

#[test]
fn bridge_tests_objectcomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunObjectComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("foo".to_string()),
                            Value::S32(42),
                            Value::Bool(false),
                            Value::Record(vec![
                                Value::String("nested".to_string()),
                                Value::S32(10),
                                Value::Bool(true),
                            ]),
                            Value::Variant {
                                case_idx: 0,
                                case_value: Some(Box::new(Value::S32(99))),
                            },
                            Value::List(vec![
                                Value::String("str1".to_string()),
                                Value::String("str2".to_string()),
                            ]),
                            Value::List(vec![Value::Record(vec![
                                Value::String("item1".to_string()),
                                Value::S32(1),
                                Value::Bool(true),
                            ])]),
                            Value::Tuple(vec![
                                Value::String("t1".to_string()),
                                Value::S32(100),
                                Value::Bool(false),
                            ]),
                            Value::Tuple(vec![
                                Value::String("t2".to_string()),
                                Value::S32(200),
                                Value::Record(vec![
                                    Value::String("t_nested".to_string()),
                                    Value::S32(20),
                                    Value::Bool(true),
                                ]),
                            ]),
                            Value::List(vec![Value::Tuple(vec![
                                Value::String("k1".to_string()),
                                Value::S32(1),
                            ])]),
                            Value::Record(vec![Value::S32(5)]),
                        ]),
                        types.object_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({
            "a": "foo",
            "b": 42,
            "c": false,
            "d": {"a": "nested", "b": 10, "c": true},
            "e": {"tag": "case1", "val": 99},
            "f": ["str1", "str2"],
            "g": [{"a": "item1", "b": 1, "c": true}],
            "h": ["t1", 100, false],
            "i": ["t2", 200, {"a": "t_nested", "b": 20, "c": true}],
            "j": [["k1", 1]],
            "k": {"n": 5}
        }),
    );
}

#[test]
fn bridge_tests_uniontype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunUnionType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 2,
                            case_value: Some(Box::new(Value::Bool(true))),
                        },
                        types.union_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"tag": "case3", "val": true}),
    );
}

#[test]
fn bridge_tests_unioncomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunUnionComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 4,
                            case_value: Some(Box::new(Value::Variant {
                                case_idx: 1,
                                case_value: Some(Box::new(Value::String("hello".to_string()))),
                            })),
                        },
                        types.union_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"tag": "case5", "val": {"tag": "case2", "val": "hello"}}),
    );
}

#[test]
fn bridge_tests_taggedunion_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunTaggedUnion",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 4,
                            case_value: Some(Box::new(Value::Record(vec![
                                Value::String("x".to_string()),
                                Value::S32(200),
                                Value::Bool(true),
                            ]))),
                        },
                        types.tagged_union.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"tag": "e", "val": {"a": "x", "b": 200, "c": true}}),
    );
}

#[test]
fn bridge_tests_unionwithliterals_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunUnionWithLiterals",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 0,
                            case_value: None,
                        },
                        types.union_with_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"tag": "lit1"}),
    );
}

#[test]
fn bridge_tests_unionwithonlyliterals_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunUnionWithOnlyLiterals",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Enum(1),
                        types.union_with_only_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!("bar"),
    );
}

#[test]
fn bridge_tests_anyonmousunionwithonlyliterals_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();
    let types = TestTypes::default();

    assert_function_output_decoding(
        target_dir,
        "FunAnonymousUnionWithOnlyLiterals",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Enum(1),
                        types.anonymous_union_with_only_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!("bar"),
    );
}

#[test]
fn bridge_tests_number_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_output_decoding(
        target_dir,
        "FunNumber",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: 42i32.into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
        json!(42),
    );
}

#[test]
fn bridge_tests_string_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_output_decoding(
        target_dir,
        "FunString",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "hello world"
                        .to_string()
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                },
            )],
        }),
        json!("hello world"),
    );
}

#[test]
fn bridge_tests_boolean_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_output_decoding(
        target_dir,
        "FunBoolean",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: true.into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
        json!(true),
    );
}

#[test]
fn bridge_tests_map_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let target_dir = Utf8Path::from_path(pkg.dir.path()).unwrap();

    assert_function_output_decoding(
        target_dir,
        "FunMap",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: vec![("key1", 10i32)]
                        .into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>()
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                },
            )],
        }),
        json!([["key1", 10]]),
    );
}

fn generate_and_compile(agent_type: AgentType, target_dir: &Utf8Path) {
    println!(
        "Generating TS bridge SDK for {} ({}) into: {}",
        agent_type.type_name, agent_type.description, target_dir
    );

    let mut gen = TypeScriptBridgeGenerator::new(agent_type, target_dir, true);
    gen.generate().unwrap();

    let status = std::process::Command::new("npm")
        .arg("install")
        .current_dir(target_dir.as_std_path())
        .status()
        .expect("failed to run `npm install`");
    assert!(status.success(), "`npm install` failed: {:?}", status);

    let status = std::process::Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(target_dir.as_std_path())
        .status()
        .expect("failed to run `npm run build`");
    assert!(status.success(), "`npm run build` failed: {:?}", status);

    let generated = collect_js_and_d_ts(target_dir.as_std_path());
    assert!(
        !generated.is_empty(),
        "no .js or .d.ts files generated in `{}`",
        target_dir
    );
}

fn collect_js_and_d_ts(dir: &std::path::Path) -> Vec<Utf8PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut result = vec![];
    while let Some(p) = stack.pop() {
        if let Ok(md) = std::fs::metadata(&p) {
            if md.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    for e in rd.flatten() {
                        stack.push(e.path());
                    }
                }
            } else if md.is_file() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if name.ends_with(".js") || name.ends_with(".d.ts") {
                        result.push(Utf8PathBuf::from_path_buf(p.to_path_buf()).unwrap());
                    }
                }
            }
        }
    }
    result
}

fn assert_function_input_encoding(
    target_dir: &Utf8Path,
    function_name: &str,
    input_json: serde_json::Value,
    expected: UntypedJsonDataValue,
) {
    // In this test we pass a JSON representing an array of function parameters as it is passed to our client method,
    // and we expect the encoding to be an UntypedDataValue matching the DataSchema of the function's input.

    let mut child = std::process::Command::new("npm")
        .arg("run")
        .arg("test")
        .arg(format!(
            "encode{}Input",
            function_name.to_upper_camel_case()
        ))
        .current_dir(target_dir.as_std_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to run encode input function");

    if let Some(ref mut stdin) = child.stdin {
        let input_str = serde_json::to_string(&input_json).expect("Failed to serialize input JSON");
        stdin
            .write_all(input_str.as_bytes())
            .expect("Failed to write to stdin");
    }

    let output = child.wait_with_output().expect("Failed to wait on npx");

    assert!(
        output.status.success(),
        "encode input function failed: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    let result_str = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with("> "))
        .collect::<Vec<_>>()
        .join("\n");
    let result: serde_json::Value = serde_json::from_str(&result_str).unwrap_or_else(|_| {
        panic!("Failed to parse JSON output from encode function:\n{result_str}")
    });

    let result_data_value: UntypedJsonDataValue =
        serde_json::from_value(result).unwrap_or_else(|_| {
            panic!("Failed to deserialize output to UntypedDataValue:\n{result_str}")
        });

    // Verify the output structure
    assert_eq!(
        result_data_value, expected,
        "Encoded data value does not match expected:\nInput:\n{input_json}\nOutput:\n{result_str}"
    );
}

fn assert_function_output_decoding(
    target_dir: &Utf8Path,
    function_name: &str,
    output: UntypedJsonDataValue,
    expected: serde_json::Value,
) {
    // In this test we pass a JSON representing an UntypedDataValue as it is returned from our REST API,
    // and we expect the output to be a JSON value representing the method's return value

    let mut child = std::process::Command::new("npm")
        .arg("run")
        .arg("test")
        .arg(format!(
            "decode{}Output",
            function_name.to_upper_camel_case()
        ))
        .current_dir(target_dir.as_std_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to run decode output function");

    if let Some(ref mut stdin) = child.stdin {
        let input_str = serde_json::to_string(&output).expect("Failed to serialize input JSON");
        stdin
            .write_all(input_str.as_bytes())
            .expect("Failed to write to stdin");
    }

    let output = child.wait_with_output().expect("Failed to wait on npx");

    assert!(
        output.status.success(),
        "decode output function failed: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    let result_str = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with("> "))
        .collect::<Vec<_>>()
        .join("\n");
    let result: serde_json::Value = serde_json::from_str(&result_str).unwrap_or_else(|_| {
        panic!("Failed to parse JSON output from decode function:\n{result_str}")
    });

    // Verify the output structure
    assert_eq!(
        result, expected,
        "Encoded decoded JSON value does not match expected:\nData:\n{output:?}\nDecoded:\n{result_str}"
    );
}
