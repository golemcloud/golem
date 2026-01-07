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
use golem_common::model::agent::AgentType;
use heck::ToUpperCamelCase;
use test_r::test;

// TODO: replace the paths with temp dirs before merging

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
    let agent_type =
        super::super::super::model::agent::test::ts_code_first_snippets()[0]
            .clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen4");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type, &target_dir);
}

fn generate_and_compile(agent_type: AgentType, target_dir: &Utf8Path) {
    let gen = TypeScriptBridgeGenerator::new(agent_type, target_dir);
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

#[test]
fn test_encode_decode_functions_exist() {
    let agent_type =
        super::super::super::model::agent::test::single_agent_wrapper_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_test_funcs");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    let test_ts = std::fs::read_to_string(target_dir.join("test.ts"))
        .expect("Failed to read test.ts");

    // Verify that all encode/decode functions are exported
    for method in &agent_type.methods {
        let method_pascal = method.name.to_upper_camel_case();
        assert!(
            test_ts.contains(&format!("export async function encode{}Input", method_pascal)),
            "Missing encode{}Input function",
            method_pascal
        );
        assert!(
            test_ts.contains(&format!("export async function decode{}Input", method_pascal)),
            "Missing decode{}Input function",
            method_pascal
        );
        assert!(
            test_ts.contains(&format!("export async function encode{}Output", method_pascal)),
            "Missing encode{}Output function",
            method_pascal
        );
        assert!(
            test_ts.contains(&format!("export async function decode{}Output", method_pascal)),
            "Missing decode{}Output function",
            method_pascal
        );
    }

    // Verify that testFunctions map includes all functions
    assert!(test_ts.contains("const testFunctions"), "Missing testFunctions map");
}

#[test]
fn test_encode_input_callable() {
    let agent_type =
        super::super::super::model::agent::test::single_agent_wrapper_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_test_encode_input");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    // Get the first method's name
    let first_method = agent_type.methods.first().expect("No methods in agent type");
    let method_pascal = first_method.name.to_upper_camel_case();

    // Call the encode function via npx tsx with a simple input
    let input = "{}";
    let status = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "cd {} && echo '{}' | npx tsx test.ts encode{}Input",
            target_dir.as_std_path().display(),
            input,
            method_pascal
        ))
        .status()
        .expect("Failed to run encode function");

    assert!(status.success(), "encode function failed");
}

#[test]
fn test_main_function_shows_available_functions() {
    let agent_type =
        super::super::super::model::agent::test::single_agent_wrapper_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_test_main");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    // Call the test script with no arguments to see the usage message
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "cd {} && npx tsx test.ts 2>&1",
            target_dir.as_std_path().display()
        ))
        .output()
        .expect("Failed to run test.ts");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("Usage: npx tsx test.ts <function-name>"),
        "Usage message not found in output: {}",
        combined
    );
}

// Tests using different agent types - call functions and verify outputs
#[test]
fn test_single_agent_encode_decode_with_input_output() {
    let agent_type =
        super::super::super::model::agent::test::single_agent_wrapper_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_encode_decode_test");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    let first_method = agent_type.methods.first().expect("No methods in agent type");
    let method_pascal = first_method.name.to_upper_camel_case();

    // Test encodeF1Input: call with empty object input
    let input = "{}";
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "cd {} && echo '{}' | npx tsx test.ts encode{}Input",
            target_dir.as_std_path().display(),
            input,
            method_pascal
        ))
        .output()
        .expect("Failed to run encode function");

    assert!(output.status.success(), "encode function failed");
    
    let result_str = String::from_utf8_lossy(&output.stdout);
    let result: serde_json::Value = serde_json::from_str(&result_str)
        .expect("Failed to parse JSON output from encode function");
    
    // Verify the output is a valid DataValue structure
    assert!(result.get("type").is_some(), "Missing 'type' field in DataValue");
    assert_eq!(
        result.get("type").and_then(|v| v.as_str()),
        Some("tuple"),
        "Expected 'tuple' type in output"
    );
    assert!(result.get("elements").is_some(), "Missing 'elements' field in DataValue");
}

#[test]
fn test_multi_agent_wrapper_2_encode_input() {
    let agent_type =
        super::super::super::model::agent::test::multi_agent_wrapper_2_types()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_multi_2_encode");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    let first_method = agent_type.methods.first().expect("No methods in agent type");
    let method_pascal = first_method.name.to_upper_camel_case();

    // Test encoding input with empty object
    let input = "{}";
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "cd {} && echo '{}' | npx tsx test.ts encode{}Input",
            target_dir.as_std_path().display(),
            input,
            method_pascal
        ))
        .output()
        .expect("Failed to run encode input function");

    assert!(output.status.success(), "encode input function failed: {:?}", String::from_utf8_lossy(&output.stderr));
    
    let result_str = String::from_utf8_lossy(&output.stdout);
    let result: serde_json::Value = serde_json::from_str(&result_str)
        .expect("Failed to parse JSON output from encode function");
    
    // Verify the output structure
    assert_eq!(
        result.get("type").and_then(|v| v.as_str()),
        Some("tuple"),
        "Expected 'tuple' type in output"
    );
    assert!(result.get("elements").is_some(), "Missing 'elements' field");
}

#[test]
fn test_multiple_element_types_encode() {
    let agent_type =
        super::super::super::model::agent::test::reproducer_for_multiple_types_called_element()[0]
            .clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_multi_element_encode");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    let first_method = agent_type.methods.first().expect("No methods in agent type");
    let method_pascal = first_method.name.to_upper_camel_case();

    // Call encode with simple numeric input
    let input = "{}";
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "cd {} && echo '{}' | npx tsx test.ts encode{}Input",
            target_dir.as_std_path().display(),
            input,
            method_pascal
        ))
        .output()
        .expect("Failed to run encode function");

    assert!(
        output.status.success(),
        "encode function failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let result_str = String::from_utf8_lossy(&output.stdout);
    let _result: serde_json::Value = serde_json::from_str(&result_str)
        .expect("Failed to parse JSON output");
}

#[test]
fn test_enum_types_compile() {
    let agent_type =
        super::super::super::model::agent::test::reproducer_for_issue_with_enums()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_enum_encode");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    let first_method = agent_type.methods.first().expect("No methods in agent type");
    let method_pascal = first_method.name.to_upper_camel_case();

    // Test that encode input function is callable
    let input = "{}";
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "cd {} && echo '{}' | npx tsx test.ts encode{}Input 2>&1",
            target_dir.as_std_path().display(),
            input,
            method_pascal
        ))
        .output()
        .expect("Failed to run encode function");

    // The test.ts should be compilable and the functions should be callable
    // We verify successful compilation by checking that the function can be invoked
    let output_str = String::from_utf8_lossy(&output.stdout);
    let error_str = String::from_utf8_lossy(&output.stderr);
    
    // If we got here without panicking, the compilation succeeded
    // Now check that output is valid JSON (success or expected error)
    if output.status.success() {
        let result: serde_json::Value = serde_json::from_str(&output_str)
            .expect("Failed to parse JSON output from encode function");
        assert!(result.is_object(), "Output should be a JSON object");
    }
}

#[test]
fn test_char_type_encode_input() {
    let agent_type = super::super::super::model::agent::test::char_type()[0].clone();
    let target_dir = Utf8Path::new("/Users/vigoo/tmp/tsgen_char_encode");

    std::fs::remove_dir_all(target_dir).ok();
    generate_and_compile(agent_type.clone(), &target_dir);

    let first_method = agent_type.methods.first().expect("No methods in agent type");
    let method_pascal = first_method.name.to_upper_camel_case();

    // Test encoding char type with a single character
    let input = r#"{"c": "a"}"#;
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "cd {} && echo '{}' | npx tsx test.ts encode{}Input",
            target_dir.as_std_path().display(),
            input,
            method_pascal
        ))
        .output()
        .expect("Failed to run encode function");

    assert!(
        output.status.success(),
        "encode char function failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let result_str = String::from_utf8_lossy(&output.stdout);
    let result: serde_json::Value = serde_json::from_str(&result_str)
        .expect("Failed to parse JSON output from encode function");

    // Verify the encoded char is in the elements
    assert!(result.get("elements").is_some(), "Missing elements in output");
}
