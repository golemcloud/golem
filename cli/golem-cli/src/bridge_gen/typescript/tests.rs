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

use crate::bridge_gen::type_naming::tests::test_type_naming;
use crate::bridge_gen::typescript::type_name::TypeScriptTypeName;
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
use golem_wasm::analysis::analysed_type::{bool, f64, field, record, s32, str};
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
    pub agent_type: AgentType,
}

impl GeneratedPackage {
    pub fn new(agent_type: AgentType) -> Self {
        let dir = TempDir::new().unwrap();
        let target_dir = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::remove_dir_all(target_dir).ok();
        generate_and_compile(agent_type.clone(), target_dir);
        GeneratedPackage { dir, agent_type }
    }

    pub fn target_dir(&self) -> &Utf8Path {
        Utf8Path::from_path(self.dir.path()).unwrap()
    }

    pub fn input_element_type_by_name(&self, method_name: &str, name: &str) -> AnalysedType {
        self.element_type_by_name_in_schema(
            method_name,
            "input",
            name,
            &self.method_by_name(method_name).input_schema,
        )
        .clone()
    }

    pub fn output_element_type_by_name(&self, method_name: &str, name: &str) -> AnalysedType {
        self.element_type_by_name_in_schema(
            method_name,
            "output",
            name,
            &self.method_by_name(method_name).output_schema,
        )
        .clone()
    }

    fn method_by_name(&self, method_name: &str) -> &AgentMethod {
        self.agent_type
            .methods
            .iter()
            .find(|m| m.name == method_name)
            .unwrap_or_else(|| {
                panic!(
                    "Method {} not found in agent {}",
                    self.agent_type.type_name, method_name
                )
            })
    }

    fn element_type_by_name_in_schema<'a>(
        &self,
        method_name: &str,
        kind: &str,
        name: &str,
        schema: &'a DataSchema,
    ) -> &'a AnalysedType {
        match schema {
            DataSchema::Tuple(tuple) => {
                let element = tuple
                    .elements
                    .iter()
                    .find(|e| e.name == name)
                    .map(|e| &e.schema)
                    .unwrap_or_else(|| {
                        panic!(
                            "Input {} not found in method {} of agent {}",
                            name,
                            method_name,
                            self.agent_type.type_name.as_str()
                        )
                    });

                match element {
                    ElementSchema::ComponentModel(component_model) => &component_model.element_type,
                    ElementSchema::UnstructuredText(_) => {
                        panic!(
                            "Expected component model {} schema while searching for {}.{}.{}",
                            kind,
                            self.agent_type.type_name.as_str(),
                            method_name,
                            name
                        )
                    }
                    ElementSchema::UnstructuredBinary(_) => {
                        panic!(
                            "Expected component model {} schema while searching for {}.{}.{}",
                            kind,
                            self.agent_type.type_name.as_str(),
                            method_name,
                            name
                        )
                    }
                }
            }
            DataSchema::Multimodal(_) => {
                panic!(
                    "Expected tuple {} schema while searching for {}.{}.{}",
                    kind,
                    self.agent_type.type_name.as_str(),
                    method_name,
                    name
                )
            }
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunOptional",
        json!([
            { "tag": "case1", "val": "value" },
            {"a": "nested"},
            {"a": { "tag": "case1", "val": "value" } },
            {"a": { "tag": "case1", "val": "value" } },
            {"a": "nested"},
            "optional",
            null
        ]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Option(Some(Box::new(Value::Variant {
                            case_idx: 0,
                            case_value: Some(Box::new(Value::String("value".to_string()))),
                        }))),
                        pkg.input_element_type_by_name("funOptional", "param1"),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        pkg.input_element_type_by_name("funOptional", "param2"),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::Variant {
                            case_idx: 0,
                            case_value: Some(Box::new(Value::String("value".to_string()))),
                        })))]),
                        pkg.input_element_type_by_name("funOptional", "param3"),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::Variant {
                            case_idx: 0,
                            case_value: Some(Box::new(Value::String("value".to_string()))),
                        })))]),
                        pkg.input_element_type_by_name("funOptional", "param4"),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::Option(Some(Box::new(Value::String(
                            "nested".to_string(),
                        ))))]),
                        pkg.input_element_type_by_name("funOptional", "param5"),
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
                    value: ValueAndType::new(
                        Value::Option(None),
                        pkg.input_element_type_by_name("funOptional", "param7"),
                    )
                    .to_json_value()
                    .unwrap(),
                }),
            ],
        }),
    );
}

#[test]
fn bridge_tests_number(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
fn bridge_tests_tuple_complex_type_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunObjectComplexType",
        json!([{
            "a": "test",
            "b": 1.1,
            "c": true,
            "d": {
                "a": "nested",
                "b": 2.2,
                "c": false
            },
            "e": { "tag": "union-type2", "val": 5.5 },
            "f": ["item"],
            "g": [{"a": "obj", "b": 1.1, "c": true}],
            "h": ["str", 2.2, false],
            "i": ["str", 2.2, {"a": "obj", "b": 1.1, "c": true}],
            "j": [],
            "k": {"n": 1.1}}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("test".to_string()),
                            Value::F64(1.1),
                            Value::Bool(true),
                            Value::Record(vec![
                                Value::String("nested".to_string()),
                                Value::F64(2.2),
                                Value::Bool(false),
                            ]),
                            Value::Variant {
                                case_idx: 1,
                                case_value: Some(Box::new(Value::F64(5.5))),
                            },
                            Value::List(vec![Value::String("item".to_string())]),
                            Value::List(vec![Value::Record(vec![
                                Value::String("obj".to_string()),
                                Value::F64(1.1),
                                Value::Bool(true),
                            ])]),
                            Value::Tuple(vec![
                                Value::String("str".to_string()),
                                Value::F64(2.2),
                                Value::Bool(false),
                            ]),
                            Value::Tuple(vec![
                                Value::String("str".to_string()),
                                Value::F64(2.2),
                                Value::Record(vec![
                                    Value::String("obj".to_string()),
                                    Value::F64(1.1),
                                    Value::Bool(true),
                                ]),
                            ]),
                            Value::List(vec![]),
                            Value::Record(vec![Value::F64(1.1)]),
                        ]),
                        pkg.input_element_type_by_name("funObjectComplexType", "text"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunUnionType",
        json!([{ "tag": "union-type4", "val": true }]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 3,
                            case_value: Some(Box::new(Value::Bool(true))),
                        },
                        pkg.input_element_type_by_name("funUnionType", "unionType"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunUnionComplexType",
        json!([{ "tag": "union-complex-type10", "val": { "n": 1.2 } }]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 9,
                            case_value: Some(Box::new(Value::Record(vec![Value::F64(1.2)]))),
                        },
                        pkg.input_element_type_by_name("funUnionComplexType", "unionComplexType"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunTaggedUnion",
        json!([{"tag": "e", "val": {"a": "x", "b": 200.1, "c": true }}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 4,
                            case_value: Some(Box::new(Value::Record(vec![
                                Value::String("x".to_string()),
                                Value::F64(200.1),
                                Value::Bool(true),
                            ]))),
                        },
                        pkg.input_element_type_by_name("funTaggedUnion", "taggedUnionType"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunTupleComplexType",
        json!([["hello", 100.1, {"a": "x", "b": 200.2, "c": true}]]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Tuple(vec![
                            Value::String("hello".to_string()),
                            Value::F64(100.1),
                            Value::Record(vec![
                                Value::String("x".to_string()),
                                Value::F64(200.2),
                                Value::Bool(true),
                            ]),
                        ]),
                        pkg.input_element_type_by_name("funTupleComplexType", "complexType"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunListComplexType",
        json!([[{"a": "item1", "b": 1.1, "c": true}, {"a": "item2", "b": 2.2, "c": false}]]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::List(vec![
                            Value::Record(vec![
                                Value::String("item1".to_string()),
                                Value::F64(1.1),
                                Value::Bool(true),
                            ]),
                            Value::Record(vec![
                                Value::String("item2".to_string()),
                                Value::F64(2.2),
                                Value::Bool(false),
                            ]),
                        ]),
                        pkg.input_element_type_by_name("funListComplexType", "listComplexType"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunObjectType",
        json!([{"a": "test", "b": 123.4, "c": true}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("test".to_string()),
                            Value::F64(123.4),
                            Value::Bool(true),
                        ]),
                        pkg.input_element_type_by_name("funObjectType", "objectType"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
                        pkg.input_element_type_by_name("funUnionWithLiterals", "unionWithLiterals"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
                        pkg.input_element_type_by_name("funUnionWithLiterals", "unionWithLiterals"),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunUnionWithOnlyLiterals",
        json!(["bar"]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Enum(1),
                        pkg.input_element_type_by_name(
                            "funUnionWithOnlyLiterals",
                            "unionWithLiterals",
                        ),
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
    assert_function_output_decoding_void(pkg.target_dir(), "FunVoidReturn");
}

#[test]
fn bridge_tests_nullreturn(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding_void(pkg.target_dir(), "FunNullReturn");
}

#[test]
fn bridge_tests_undefinedreturn(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding_void(pkg.target_dir(), "FunUndefinedReturn");
}

#[test]
fn bridge_tests_unstructuredtext(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunResultExact",
        json!([{"ok": "value"}]),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunResultLikeWithVoid",
        json!([{}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue { value: json!({}) },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultvs_ok(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunBuiltinResultVS",
        json!([{}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue { value: json!({}) },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultvs_err(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunBuiltinResultVS",
        json!([{"err": "hello"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"err": "hello"}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultsv_ok(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunBuiltinResultSV",
        json!([{"ok": "hello"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"ok": "hello"}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultsv_err(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunBuiltinResultSV",
        json!([{}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue { value: json!({}) },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultsn_ok(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunBuiltinResultSN",
        json!([{"ok": "hello"}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"ok": "hello"}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_builtinresultsn_number(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunBuiltinResultSN",
        json!([{"err": 123.4}]),
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: json!({"err": 123.4}),
                },
            )],
        }),
    );
}

#[test]
fn bridge_tests_noreturn(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_input_encoding(
        pkg.target_dir(),
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
    assert_function_output_decoding(
        pkg.target_dir(),
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
    let bool_val: bool = false;

    assert_function_output_decoding(
        pkg.target_dir(),
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
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunListComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::List(vec![
                            Value::Record(vec![
                                Value::String("item1".to_string()),
                                Value::F64(1.1),
                                Value::Bool(true),
                            ]),
                            Value::Record(vec![
                                Value::String("item2".to_string()),
                                Value::F64(2.1),
                                Value::Bool(false),
                            ]),
                        ]),
                        pkg.output_element_type_by_name("funListComplexType", "return-value"),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!([{"a": "item1", "b": 1.1, "c": true}, {"a": "item2", "b": 2.1, "c": false}]),
    );
}

#[test]
fn bridge_tests_objecttype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunObjectType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("test".to_string()),
                            Value::F64(123.4),
                            Value::Bool(true),
                        ]),
                        pkg.output_element_type_by_name("funObjectType", "return-value"),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"a": "test", "b": 123.4, "c": true}),
    );
}

#[test]
fn bridge_tests_objectcomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunObjectComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Record(vec![
                            Value::String("foo".to_string()),
                            Value::F64(42.1),
                            Value::Bool(false),
                            Value::Record(vec![
                                Value::String("nested".to_string()),
                                Value::F64(10.1),
                                Value::Bool(true),
                            ]),
                            Value::Variant {
                                case_idx: 0,
                                case_value: Some(Box::new(Value::String("hello".to_string()))),
                            },
                            Value::List(vec![
                                Value::String("str1".to_string()),
                                Value::String("str2".to_string()),
                            ]),
                            Value::List(vec![Value::Record(vec![
                                Value::String("item1".to_string()),
                                Value::F64(1.1),
                                Value::Bool(true),
                            ])]),
                            Value::Tuple(vec![
                                Value::String("t1".to_string()),
                                Value::F64(100.1),
                                Value::Bool(false),
                            ]),
                            Value::Tuple(vec![
                                Value::String("t2".to_string()),
                                Value::F64(200.1),
                                Value::Record(vec![
                                    Value::String("t_nested".to_string()),
                                    Value::F64(20.1),
                                    Value::Bool(true),
                                ]),
                            ]),
                            Value::List(vec![Value::Tuple(vec![
                                Value::String("k1".to_string()),
                                Value::F64(1.1),
                            ])]),
                            Value::Record(vec![Value::F64(5.1)]),
                        ]),
                        pkg.output_element_type_by_name("funObjectComplexType", "return-value"),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({
            "a": "foo",
            "b": 42.1,
            "c": false,
            "d": {"a": "nested", "b": 10.1, "c": true},
            "e": {"tag": "union-type1", "val": "hello"},
            "f": ["str1", "str2"],
            "g": [{"a": "item1", "b": 1.1, "c": true}],
            "h": ["t1", 100.1, false],
            "i": ["t2", 200.1, {"a": "t_nested", "b": 20.1, "c": true}],
            "j": [["k1", 1.1]],
            "k": {"n": 5.1}
        }),
    );
}

#[test]
fn bridge_tests_uniontype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunUnionType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 3,
                            case_value: Some(Box::new(Value::Bool(true))),
                        },
                        pkg.output_element_type_by_name("funUnionType", "return-value"),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"tag": "union-type4", "val": true}),
    );
}

#[test]
fn bridge_tests_unioncomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunUnionComplexType",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 3,
                            case_value: Some(Box::new(Value::Record(vec![
                                Value::String("hello".to_string()),
                                Value::F64(123.4),
                                Value::Bool(true),
                            ]))),
                        },
                        pkg.output_element_type_by_name("funUnionComplexType", "return-value"),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"tag": "union-complex-type4", "val": {"a": "hello", "b": 123.4, "c": true}}),
    );
}

#[test]
fn bridge_tests_taggedunion_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunTaggedUnion",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 4,
                            case_value: Some(Box::new(Value::Record(vec![
                                Value::String("x".to_string()),
                                Value::F64(200.2),
                                Value::Bool(true),
                            ]))),
                        },
                        pkg.output_element_type_by_name("funTaggedUnion", "return-value"),
                    )
                    .to_json_value()
                    .unwrap(),
                },
            )],
        }),
        json!({"tag": "e", "val": {"a": "x", "b": 200.2, "c": true}}),
    );
}

#[test]
fn bridge_tests_unionwithliterals_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunUnionWithLiterals",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Variant {
                            case_idx: 0,
                            case_value: None,
                        },
                        pkg.output_element_type_by_name("funUnionWithLiterals", "return-value"),
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
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunUnionWithOnlyLiterals",
        UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
            elements: vec![UntypedJsonElementValue::ComponentModel(
                JsonComponentModelValue {
                    value: ValueAndType::new(
                        Value::Enum(1),
                        pkg.output_element_type_by_name("funUnionWithOnlyLiterals", "return-value"),
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
    assert_function_output_decoding(
        pkg.target_dir(),
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
    assert_function_output_decoding(
        pkg.target_dir(),
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
    assert_function_output_decoding(
        pkg.target_dir(),
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
    assert_function_output_decoding(
        pkg.target_dir(),
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

    let mut gen = TypeScriptBridgeGenerator::new(agent_type, target_dir, true).unwrap();
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

    let input_str = {
        let stdin = child.stdin.as_mut().expect("Failed to get stdin");
        let input_str = serde_json::to_string(&input_json).expect("Failed to serialize input JSON");
        stdin
            .write_all(input_str.as_bytes())
            .expect("Failed to write to stdin");
        input_str
    };

    let output = child.wait_with_output().expect("Failed to wait on npx");

    assert!(
        output.status.success(),
        "encode input function failed for: {}",
        input_str,
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

    let input_str = {
        let stdin = child.stdin.as_mut().expect("Failed to get stdin");
        let input_str = serde_json::to_string(&output).expect("Failed to serialize input JSON");
        stdin
            .write_all(input_str.as_bytes())
            .expect("Failed to write to stdin");
        input_str
    };

    let output = child.wait_with_output().expect("Failed to wait on npx");

    assert!(
        output.status.success(),
        "decode output function failed for: {}",
        input_str,
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

fn assert_function_output_decoding_void(target_dir: &Utf8Path, function_name: &str) {
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

    {
        let stdin = child.stdin.as_mut().expect("Failed to get stdin");
        stdin
            .write_all(
                "THIS IS NOT A JSON AND WE EXPECT THAT IT IS NOT READ AT ALL, OTHERWISE (╯°□°)╯︵ ┻━┻"
                    .as_bytes(),
            )
            .expect("Failed to write to stdin");
    };

    let output = child.wait_with_output().expect("Failed to wait on npx");

    assert!(
        output.status.success(),
        "decode output function failed for void",
    );

    let result_str = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with("> "))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(result_str, "void");
}

#[test]
fn test_ts_type_naming_rust_foo() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::Rust, "FooAgent");
}

#[test]
fn test_ts_type_naming_rust_bar() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::Rust, "BarAgent");
}

#[test]
fn test_ts_type_naming_ts_foo() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::TypeScript, "FooAgent");
}

#[test]
fn test_ts_type_naming_ts_bar() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::TypeScript, "BarAgent");
}
