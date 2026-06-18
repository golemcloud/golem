// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::bridge_gen::fixtures::{
    code_first_snippets_agent_type, multi_agent_wrapper_2_types, single_agent_wrapper_types,
};
use crate::bridge_gen::type_naming::test_type_naming;
use camino::{Utf8Path, Utf8PathBuf};
use golem_cli::bridge_gen::BridgeGenerator;
use golem_cli::bridge_gen::typescript::{TypeScriptBridgeGenerator, TypeScriptTypeName};
use golem_cli::model::GuestLanguage;
use golem_common::model::Empty;
use golem_common::model::agent::{
    AgentConfigDeclaration, AgentConfigSource, AgentConstructor, AgentMethod, AgentMode, AgentType,
    AgentTypeName, ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
    NamedElementSchemas, Snapshotting,
};
use golem_wasm::analysis::analysed_type::{f64, field, list, option, record, s32, str};
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

    pub fn target_dir(&self) -> &Utf8Path {
        Utf8Path::from_path(self.dir.path()).unwrap()
    }
}

#[test_dep(scope = PerWorker, tagged_as = "single_agent_wrapper_types_1")]
fn ts_single_agent_wrapper_1() -> GeneratedPackage {
    GeneratedPackage::new(single_agent_wrapper_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_wrapper_2_types_1")]
fn ts_multi_agent_wrapper_2_types_1() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_wrapper_2_types_2")]
fn ts_multi_agent_wrapper_2_types_2() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[1].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "counter_agent")]
fn ts_counter_agent() -> GeneratedPackage {
    let agent_type = AgentType {
        type_name: AgentTypeName("CounterAgent".to_string()),
        description: "Constructs the agent CounterAgent".to_string(),
        source_language: "typescript".to_string(),
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
                    name: "returnValue".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: f64(),
                    }),
                }],
            }),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: Vec::new(),
    };

    GeneratedPackage::new(agent_type)
}

#[test_dep(scope = PerWorker, tagged_as = "ts_collision_parameter_names_agent")]
fn ts_collision_parameter_names_agent() -> GeneratedPackage {
    let agent_type = AgentType {
        type_name: AgentTypeName("CollisionParameterNamesAgent".to_string()),
        description: "Constructs the agent CollisionParameterNamesAgent".to_string(),
        source_language: "typescript".to_string(),
        constructor: AgentConstructor {
            name: Some("CollisionParameterNamesAgent".to_string()),
            description: "Constructs the agent CollisionParameterNamesAgent".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
        },
        methods: vec![AgentMethod {
            name: "collide".to_string(),
            description: "Method with argument names colliding with generated internals"
                .to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![
                    NamedElementSchema {
                        name: "args".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    },
                    NamedElementSchema {
                        name: "methodParameters".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    },
                    NamedElementSchema {
                        name: "result".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    },
                    NamedElementSchema {
                        name: "multimodalInput".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    },
                ],
            }),
            output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: Vec::new(),
    };

    GeneratedPackage::new(agent_type)
}

#[test_dep(scope = PerWorker, tagged_as = "ts_config_nested_local_types_agent")]
fn ts_config_nested_local_types_agent() -> GeneratedPackage {
    GeneratedPackage::new(ts_config_nested_local_types_agent_type())
}

fn ts_config_nested_local_types_agent_type() -> AgentType {
    let config_record_type = record(vec![field("x", s32()), field("y", str())]);

    AgentType {
        type_name: AgentTypeName("ConfigNestedLocalTypesAgent".to_string()),
        description: "Agent with nested local config types".to_string(),
        source_language: "typescript".to_string(),
        constructor: AgentConstructor {
            name: Some("ConfigNestedLocalTypesAgent".to_string()),
            description: "Constructs the agent ConfigNestedLocalTypesAgent".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
        },
        methods: vec![AgentMethod {
            name: "ping".to_string(),
            description: "Simple method for bridge generation".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "returnValue".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: str(),
                    }),
                }],
            }),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![
            AgentConfigDeclaration {
                source: AgentConfigSource::Local,
                path: vec!["xyz".to_string()],
                value_type: option(config_record_type.clone()),
            },
            AgentConfigDeclaration {
                source: AgentConfigSource::Local,
                path: vec!["records".to_string()],
                value_type: list(config_record_type),
            },
        ],
    }
}

#[test_dep(scope = PerWorker, tagged_as = "ts_code_first_snippets_foo_agent")]
fn ts_code_first_snippets_foo_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::TypeScript,
        "FooAgent",
    ))
}

#[test_dep(scope = PerWorker, tagged_as = "ts_code_first_snippets_bar_agent")]
fn ts_code_first_snippets_bar_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::TypeScript,
        "BarAgent",
    ))
}

#[test_dep(scope = PerWorker, tagged_as = "rust_code_first_snippets_foo_agent")]
fn rust_code_first_snippets_foo_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::Rust,
        "FooAgent",
    ))
}

#[test_dep(scope = PerWorker, tagged_as = "rust_code_first_snippets_bar_agent")]
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
fn collision_parameter_names_agent_compiles(
    #[tagged_as("ts_collision_parameter_names_agent")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn config_nested_local_types_agent_bridge_compiles(
    #[tagged_as("ts_config_nested_local_types_agent")] _pkg: &GeneratedPackage,
) {
}

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
        json!({"kind": "record", "value": {"fields": [{"kind": "string", "value": "value1"}, {"kind": "option", "value": {"inner": {"kind": "f64", "value": 10}}}, {"kind": "option", "value": {}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "option", "value": {"inner": {"kind": "variant", "value": {"case": 0, "payload": {"kind": "string", "value": "value"}}}}}, {"kind": "record", "value": {"fields": [{"kind": "option", "value": {"inner": {"kind": "string", "value": "nested"}}}]}}, {"kind": "record", "value": {"fields": [{"kind": "option", "value": {"inner": {"kind": "variant", "value": {"case": 0, "payload": {"kind": "string", "value": "value"}}}}}]}}, {"kind": "record", "value": {"fields": [{"kind": "option", "value": {"inner": {"kind": "variant", "value": {"case": 0, "payload": {"kind": "string", "value": "value"}}}}}]}}, {"kind": "record", "value": {"fields": [{"kind": "option", "value": {"inner": {"kind": "string", "value": "nested"}}}]}}, {"kind": "option", "value": {"inner": {"kind": "string", "value": "optional"}}}, {"kind": "option", "value": {}}]}}),
    );
}

#[test]
fn bridge_tests_number(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunNumber",
        json!([42]),
        json!({"kind": "record", "value": {"fields": [{"kind": "f64", "value": 42}]}}),
    );
}

#[test]
fn bridge_tests_string(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunString",
        json!(["hello"]),
        json!({"kind": "record", "value": {"fields": [{"kind": "string", "value": "hello"}]}}),
    );
}

#[test]
fn bridge_tests_boolean(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunBoolean",
        json!([true]),
        json!({"kind": "record", "value": {"fields": [{"kind": "bool", "value": true}]}}),
    );
}

#[test]
fn bridge_tests_tuple_complex_type_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunTupleComplexType",
        json!({"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "hello"}, {"kind": "f64", "value": 100}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "x"}, {"kind": "f64", "value": 200}, {"kind": "bool", "value": true}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "string", "value": "value1"}, {"kind": "option", "value": {"inner": {"kind": "f64", "value": 10}}}, {"kind": "option", "value": {}}]}}),
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
            "e": { "tag": "UnionType1", "val": 5.5 },
            "f": ["item"],
            "g": [{"a": "obj", "b": 1.1, "c": true}],
            "h": ["str", 2.2, false],
            "i": ["str", 2.2, {"a": "obj", "b": 1.1, "c": true}],
            "j": [],
            "k": {"n": 1.1}}]),
        json!({"kind": "record", "value": {"fields": [{"kind": "record", "value": {"fields": [{"kind": "string", "value": "test"}, {"kind": "f64", "value": 1.1}, {"kind": "bool", "value": true}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "nested"}, {"kind": "f64", "value": 2.2}, {"kind": "bool", "value": false}]}}, {"kind": "variant", "value": {"case": 0, "payload": {"kind": "f64", "value": 5.5}}}, {"kind": "list", "value": {"elements": [{"kind": "string", "value": "item"}]}}, {"kind": "list", "value": {"elements": [{"kind": "record", "value": {"fields": [{"kind": "string", "value": "obj"}, {"kind": "f64", "value": 1.1}, {"kind": "bool", "value": true}]}}]}}, {"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "str"}, {"kind": "f64", "value": 2.2}, {"kind": "bool", "value": false}]}}, {"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "str"}, {"kind": "f64", "value": 2.2}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "obj"}, {"kind": "f64", "value": 1.1}, {"kind": "bool", "value": true}]}}]}}, {"kind": "list", "value": {"elements": []}}, {"kind": "record", "value": {"fields": [{"kind": "f64", "value": 1.1}]}}]}}]}}),
    );
}

#[test]
fn bridge_tests_uniontype(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunUnionType",
        json!([{ "tag": "UnionType3", "val": true }]),
        json!({"kind": "record", "value": {"fields": [{"kind": "variant", "value": {"case": 2, "payload": {"kind": "bool", "value": true}}}]}}),
    );
}

#[test]
fn bridge_tests_unioncomplextype(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunUnionComplexType",
        json!([{ "tag": "UnionComplexType8", "val": { "n": 1.2 } }]),
        json!({"kind": "record", "value": {"fields": [{"kind": "variant", "value": {"case": 7, "payload": {"kind": "record", "value": {"fields": [{"kind": "f64", "value": 1.2}]}}}}]}}),
    );
}

#[test]
fn bridge_tests_map(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunMap",
        json!([[["key1", 10]]]),
        json!({"kind": "record", "value": {"fields": [{"kind": "list", "value": {"elements": [{"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "key1"}, {"kind": "f64", "value": 10}]}}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "variant", "value": {"case": 4, "payload": {"kind": "record", "value": {"fields": [{"kind": "string", "value": "x"}, {"kind": "f64", "value": 200.1}, {"kind": "bool", "value": true}]}}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "hello"}, {"kind": "f64", "value": 100.1}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "x"}, {"kind": "f64", "value": 200.2}, {"kind": "bool", "value": true}]}}]}}]}}),
    );
}

#[test]
fn bridge_tests_tupletype(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunTupleType",
        json!([["item", 42, false]]),
        json!({"kind": "record", "value": {"fields": [{"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "item"}, {"kind": "f64", "value": 42}, {"kind": "bool", "value": false}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "list", "value": {"elements": [{"kind": "record", "value": {"fields": [{"kind": "string", "value": "item1"}, {"kind": "f64", "value": 1.1}, {"kind": "bool", "value": true}]}}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "item2"}, {"kind": "f64", "value": 2.2}, {"kind": "bool", "value": false}]}}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "record", "value": {"fields": [{"kind": "string", "value": "test"}, {"kind": "f64", "value": 123.4}, {"kind": "bool", "value": true}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "variant", "value": {"case": 0}}]}}),
    );
}

#[test]
fn bridge_tests_unionwithliterals_with_value(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunUnionWithLiterals",
        json!([{"tag": "UnionWithLiterals1", "val": true}]),
        json!({"kind": "record", "value": {"fields": [{"kind": "variant", "value": {"case": 3, "payload": {"kind": "bool", "value": true}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "enum", "value": {"case": 1}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "text", "value": {"text": "plain text"}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "binary", "value": {"bytes": [0, 1, 2, 3], "mime_type": "application/json"}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "list", "value": {"elements": [{"kind": "variant", "value": {"case": 0, "payload": {"kind": "text", "value": {"text": "hello"}}}}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "list", "value": {"elements": [{"kind": "variant", "value": {"case": 1, "payload": {"kind": "string", "value": "input"}}}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "record", "value": {"fields": [{"kind": "option", "value": {"inner": {"kind": "string", "value": "value"}}}, {"kind": "option", "value": {}}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "ok", "value": {"kind": "string", "value": "value"}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "variant", "value": {"case": 0, "payload": {"kind": "string", "value": "value"}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "err"}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "err", "value": {"kind": "string"}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "err", "value": {"kind": "string", "value": "hello"}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "ok", "value": {"kind": "string", "value": "hello"}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "err"}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "ok", "value": {"kind": "string", "value": "hello"}}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "result", "value": {"tag": "err", "value": {"kind": "f64", "value": 123.4}}}]}}),
    );
}

#[test]
fn bridge_tests_noreturn(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunNoReturn",
        json!(["test"]),
        json!({"kind": "record", "value": {"fields": [{"kind": "string", "value": "test"}]}}),
    );
}

#[test]
fn bridge_tests_arrowsync(#[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage) {
    assert_function_input_encoding(
        pkg.target_dir(),
        "FunArrowSync",
        json!(["test"]),
        json!({"kind": "record", "value": {"fields": [{"kind": "string", "value": "test"}]}}),
    );
}

#[test]
fn bridge_tests_tuplecomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunTupleComplexType",
        json!({"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "hello"}, {"kind": "f64", "value": 100}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "x"}, {"kind": "f64", "value": 200}, {"kind": "bool", "value": true}]}}]}}),
        json!(["hello", 100, { "a": "x", "b": 200, "c": true }]),
    );
}

#[test]
fn bridge_tests_tupletype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunTupleType",
        json!({"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "item"}, {"kind": "f64", "value": 42}, {"kind": "bool", "value": false}]}}),
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
        json!({"kind": "list", "value": {"elements": [{"kind": "record", "value": {"fields": [{"kind": "string", "value": "item1"}, {"kind": "f64", "value": 1.1}, {"kind": "bool", "value": true}]}}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "item2"}, {"kind": "f64", "value": 2.1}, {"kind": "bool", "value": false}]}}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "string", "value": "test"}, {"kind": "f64", "value": 123.4}, {"kind": "bool", "value": true}]}}),
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
        json!({"kind": "record", "value": {"fields": [{"kind": "string", "value": "foo"}, {"kind": "f64", "value": 42.1}, {"kind": "bool", "value": false}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "nested"}, {"kind": "f64", "value": 10.1}, {"kind": "bool", "value": true}]}}, {"kind": "variant", "value": {"case": 1, "payload": {"kind": "string", "value": "hello"}}}, {"kind": "list", "value": {"elements": [{"kind": "string", "value": "str1"}, {"kind": "string", "value": "str2"}]}}, {"kind": "list", "value": {"elements": [{"kind": "record", "value": {"fields": [{"kind": "string", "value": "item1"}, {"kind": "f64", "value": 1.1}, {"kind": "bool", "value": true}]}}]}}, {"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "t1"}, {"kind": "f64", "value": 100.1}, {"kind": "bool", "value": false}]}}, {"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "t2"}, {"kind": "f64", "value": 200.1}, {"kind": "record", "value": {"fields": [{"kind": "string", "value": "t_nested"}, {"kind": "f64", "value": 20.1}, {"kind": "bool", "value": true}]}}]}}, {"kind": "list", "value": {"elements": [{"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "k1"}, {"kind": "f64", "value": 1.1}]}}]}}, {"kind": "record", "value": {"fields": [{"kind": "f64", "value": 5.1}]}}]}}),
        json!({
            "a": "foo",
            "b": 42.1,
            "c": false,
            "d": {"a": "nested", "b": 10.1, "c": true},
            "e": {"tag": "UnionType2", "val": "hello"},
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
        json!({"kind": "variant", "value": {"case": 2, "payload": {"kind": "bool", "value": true}}}),
        json!({"tag": "UnionType3", "val": true}),
    );
}

#[test]
fn bridge_tests_unioncomplextype_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunUnionComplexType",
        json!({"kind": "variant", "value": {"case": 0, "payload": {"kind": "f64", "value": 123.4}}}),
        json!({"tag": "UnionComplexType1", "val": 123.4}),
    );
}

#[test]
fn bridge_tests_taggedunion_output(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    assert_function_output_decoding(
        pkg.target_dir(),
        "FunTaggedUnion",
        json!({"kind": "variant", "value": {"case": 4, "payload": {"kind": "record", "value": {"fields": [{"kind": "string", "value": "x"}, {"kind": "f64", "value": 200.2}, {"kind": "bool", "value": true}]}}}}),
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
        json!({"kind": "variant", "value": {"case": 0}}),
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
        json!({"kind": "enum", "value": {"case": 1}}),
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
        json!({"kind": "f64", "value": 42}),
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
        json!({"kind": "string", "value": "hello world"}),
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
        json!({"kind": "bool", "value": true}),
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
        json!({"kind": "list", "value": {"elements": [{"kind": "tuple", "value": {"elements": [{"kind": "string", "value": "key1"}, {"kind": "f64", "value": 10}]}}]}}),
        json!([["key1", 10]]),
    );
}

fn generate_and_compile(agent_type: AgentType, target_dir: &Utf8Path) {
    println!(
        "Generating TS bridge SDK for {} ({}) into: {}",
        agent_type.type_name, agent_type.description, target_dir
    );

    let mut generator = TypeScriptBridgeGenerator::new(agent_type, target_dir, true).unwrap();
    generator.generate().unwrap();

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
            } else if md.is_file()
                && let Some(name) = p.file_name().and_then(|s| s.to_str())
                && (name.ends_with(".js") || name.ends_with(".d.ts"))
            {
                result.push(Utf8PathBuf::from_path_buf(p.to_path_buf()).unwrap());
            }
        }
    }
    result
}

fn assert_function_input_encoding(
    target_dir: &Utf8Path,
    function_name: &str,
    input_json: serde_json::Value,
    expected: serde_json::Value,
) {
    // In this test we pass a JSON representing an array of function parameters as it is passed to our client method,
    // and we expect the encoding to be a schema-native SchemaValue JSON matching the DataSchema of the function's input.

    let mut child = std::process::Command::new("npm")
        .arg("run")
        .arg("test")
        .arg(format!("encode{function_name}Input"))
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

    // Verify the output structure
    assert_eq!(
        result, expected,
        "Encoded data value does not match expected:\nInput:\n{input_json}\nOutput:\n{result_str}"
    );
}

fn assert_function_output_decoding(
    target_dir: &Utf8Path,
    function_name: &str,
    output: serde_json::Value,
    expected: serde_json::Value,
) {
    // In this test we pass a JSON representing a schema-native SchemaValue as it is returned from our REST API,
    // and we expect the output to be a JSON value representing the method's return value

    let mut child = std::process::Command::new("npm")
        .arg("run")
        .arg("test")
        .arg(format!("decode{function_name}Output"))
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
        .arg(format!("decode{function_name}Output"))
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
