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
    AgentType, JsonComponentModelValue, UntypedDataValue, UntypedElementValue, UntypedElementValues,
};
use golem_wasm::analysis::analysed_type::{bool, field, record, s32, str};
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

    assert_function_output_decoding(
        target_dir,
        "FunTupleComplexType",
        UntypedDataValue::Tuple(UntypedElementValues {
            elements: vec![UntypedElementValue::ComponentModel( // single return value containing a tuple
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
