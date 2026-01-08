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
use camino::{Utf8Path, Utf8PathBuf};
use golem_client::model::ValueAndType;
use golem_common::model::agent::{
    AgentType, BinaryReference, BinaryReferenceValue, BinarySource, BinaryType,
    JsonComponentModelValue, TextReference, TextReferenceValue, TextSource, UntypedDataValue,
    UntypedElementValue, UntypedElementValues,
};
use golem_wasm::analysis::analysed_type::{
    bool, case, field, list, option, r#enum, record, s32, str, tuple, unit_case, variant,
};
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{IntoValueAndType, Value};
use heck::ToUpperCamelCase;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::io::Write;
use std::process::Stdio;
use test_r::test;
// TODO: replace the paths with temp dirs before merging
// TODO: generate doc comments based on descriptions

// Playground tests for manual inspection
#[test]
fn playground1() {
    let agent_type =
        super::super::super::model::agent::test::single_agent_wrapper_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type, &target_dir);
}

#[test]
fn playground2() {
    let agent_type =
        super::super::super::model::agent::test::multi_agent_wrapper_2_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen2");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type, &target_dir);
}

#[test]
fn playground3() {
    let agent_type =
        super::super::super::model::agent::test::multi_agent_wrapper_2_types()[1].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen3");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type, &target_dir);
}

#[test]
fn playground4() {
    let agent_type = super::super::super::model::agent::test::ts_code_first_snippets()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen4");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type, &target_dir);

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

    let union_with_only_literals = r#enum(&["foo", "bar", "baz"]).named("UnionWithOnlyLiterals");

    let object_with_union_undefined = record(vec![field("a", option(str()))]);

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
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: "value1".into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some(10i32).into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: None::<i32>.into_value_and_type().to_json_value().unwrap(),
                }),
            ],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunOptional",
        // [string | undefined, ObjectWithUnionWithUndefined1, ObjectWithUnionWithUndefined2, ObjectWithUnionWithUndefined3, ObjectWithUnionWithUndefined4, string | undefined, UnionType | undefined]
        json!(["value", {"a": "nested"}, {"a": "nested"}, {"a": "nested"}, {"a": "nested"}, "optional", null]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some("value".to_string())
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        object_with_union_undefined.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some("optional".to_string())
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(Value::Option(None), option(union_type.clone()))
                        .to_json_value()
                        .unwrap(),
                }),
            ],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunOptionalQMark",
        json!(["value1", 10, null]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: "value1".into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: Some(10i32).into_value_and_type().to_json_value().unwrap(),
                }),
                UntypedElementValue::ComponentModel(JsonComponentModelValue {
                    value: None::<i32>.into_value_and_type().to_json_value().unwrap(),
                }),
            ],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunNumber",
        json!([42]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: (42i32).into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunString",
        json!(["hello"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "hello".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunBoolean",
        json!([true]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: true.into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

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
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
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
                        object_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUnionType",
        json!([{ "tag": "case3", "val": true }]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 2,
                            case_value: Some(Box::new(Value::Bool(true))),
                        },
                        union_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUnionComplexType",
        json!([{ "tag": "case8", "val": { "n": 1 } }]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 7,
                            case_value: Some(Box::new(Value::Record(vec![Value::S32(1)]))),
                        },
                        union_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunNumber",
        json!([42]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: (42i32).into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunString",
        json!(["hello"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "hello".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunBoolean",
        json!([true]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: true.into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunMap",
        json!([[["key1", 10]]]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
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

    assert_function_input_encoding(
        target_dir,
        "FunTaggedUnion",
        json!([{"tag": "e", "val": {"a": "x", "b": 200, "c": true }}]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
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
                        tagged_union.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunTupleComplexType",
        json!([["hello", 100, {"a": "x", "b": 200, "c": true}]]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
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
                        tuple_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunTupleType",
        json!([["item", 42, false]]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ("item".to_string(), 42, false)
                        .into_value_and_type()
                        .to_json_value()
                        .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunListComplexType",
        json!([[{"a": "item1", "b": 1, "c": true}, {"a": "item2", "b": 2, "c": false}]]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
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
                        list_complex_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunObjectType",
        json!([{"a": "test", "b": 123, "c": true}]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("test".to_string()),
                            Value::S32(123),
                            Value::Bool(true),
                        ]),
                        object_type.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUnionWithLiterals",
        json!([{"tag": "lit1"}]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 0,
                            case_value: None,
                        },
                        union_with_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUnionWithLiterals",
        json!([{"tag": "union-with-literals1", "val": true}]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 3,
                            case_value: Some(Box::new(Value::Bool(true))),
                        },
                        union_with_literals.clone(),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUnionWithOnlyLiterals",
        json!(["bar"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(Value::Enum(1), union_with_only_literals.clone())
                        .to_json_value()
                        .unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunVoidReturn",
        json!(["test"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunNullReturn",
        json!(["test"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUndefinedReturn",
        json!(["test"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUnstructuredText",
        json!([{ "tag": "inline", "val": "plain text"}]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource {
                    data: "plain text".to_string(),
                    text_type: None,
                }),
            })],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunUnstructuredBinary",
        json!([{ "tag": "inline", "mimeType": "application/json", "val": [0,1,2,3]}]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::UnstructuredBinary(
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

    // TODO: FunMultimodal - multimodal encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunMultimodal",
    //     json!([{"mimeType": "text/plain", "data": "test"}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"mimeType": "text/plain", "data": "test"}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunMultimodalAdvanced - multimodal advanced encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunMultimodalAdvanced",
    //     json!([{"tag": "text", "val": "input"}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"tag": "text", "val": "input"}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunEitherOptional - either/optional encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunEitherOptional",
    //     json!([{"ok": "value"}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"ok": "value"}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunResultExact - result encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunResultExact",
    //     json!([{"ok": "value"}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"ok": "value"}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunResultLike - result encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunResultLike",
    //     json!([{"ok": "value"}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"ok": "value"}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunResultLikeWithVoid - result encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunResultLikeWithVoid",
    //     json!([{"ok": null}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"ok": null}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunBuiltinResultVS - result encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunBuiltinResultVS",
    //     json!([{"ok": null}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"ok": null}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunBuiltinResultSV - result encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunBuiltinResultSV",
    //     json!([{"ok": "value"}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"ok": "value"}),
    //             }),
    //         ],
    //     }),
    // );

    // TODO: FunBuiltinResultSN - result encoding format needs investigation
    // assert_function_input_encoding(
    //     target_dir,
    //     "FunBuiltinResultSN",
    //     json!([{"ok": "value"}]),
    //     UntypedDataValue::Tuple(UntypedElementValues {
    //         elements: vec![
    //             UntypedElementValue::ComponentModel(JsonComponentModelValue {
    //                 value: json!({"ok": "value"}),
    //             }),
    //         ],
    //     }),
    // );

    assert_function_input_encoding(
        target_dir,
        "FunNoReturn",
        json!(["test"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_input_encoding(
        target_dir,
        "FunArrowSync",
        json!(["test"]),
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: "test".into_value_and_type().to_json_value().unwrap(),
                },
            )],
        }),
    );

    assert_function_output_decoding(
        target_dir,
        "FunTupleComplexType",
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel(
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

fn generate_and_compile(agent_type: AgentType, target_dir: &Utf8Path) {
    let gen = TypeScriptBridgeGenerator::new(agent_type, target_dir, true);
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
    expected: UntypedDataValue,
) {
    // In this test we pass a JSON representing an array of function parameters as it is passed to our client method,
    // and we expect the encoding to be a UntypedDataValue matching the DataSchema of the function's input.

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
    let result: serde_json::Value = serde_json::from_str(&result_str).expect(&format!(
        "Failed to parse JSON output from encode function:\n{result_str}"
    ));

    let result_data_value: UntypedDataValue = serde_json::from_value(result).expect(&format!(
        "Failed to deserialize output to UntypedDataValue:\n{result_str}"
    ));

    // Verify the output structure
    assert_eq!(
        result_data_value, expected,
        "Encoded data value does not match expected:\nInput:\n{input_json}\nOutput:\n{result_str}"
    );
}

fn assert_function_output_decoding(
    target_dir: &Utf8Path,
    function_name: &str,
    output: UntypedDataValue,
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
    let result: serde_json::Value = serde_json::from_str(&result_str).expect(&format!(
        "Failed to parse JSON output from decode function:\n{result_str}"
    ));

    // Verify the output structure
    assert_eq!(
        result, expected,
        "Encoded decoded JSON value does not match expected:\nData:\n{output:?}\nDecoded:\n{result_str}"
    );
}
