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

use crate::bridge_gen::rust::type_name::RustTypeName;
use crate::bridge_gen::rust::RustBridgeGenerator;
use crate::bridge_gen::type_naming::tests::test_type_naming;
use crate::bridge_gen::BridgeGenerator;
use crate::model::agent::test::{
    code_first_snippets_agent_type, multi_agent_wrapper_2_types, single_agent_wrapper_types,
};
use crate::model::GuestLanguage;
use camino::Utf8Path;
use golem_common::model::agent::{
    AgentConstructor, AgentMethod, AgentMode, AgentType, AgentTypeName,
    ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
    NamedElementSchemas, Snapshotting,
};
use golem_common::model::Empty;
use golem_wasm::analysis::analysed_type::{f64, str};
use tempfile::TempDir;
use test_r::{test, test_dep};

struct GeneratedPackage {
    #[allow(dead_code)]
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

#[test_dep(tagged_as = "single_agent_wrapper_types_1")]
fn rust_single_agent_wrapper_1() -> GeneratedPackage {
    GeneratedPackage::new(single_agent_wrapper_types()[0].clone())
}

#[test_dep(tagged_as = "multi_agent_wrapper_2_types_1")]
fn rust_multi_agent_wrapper_2_types_1() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone())
}

#[test_dep(tagged_as = "multi_agent_wrapper_2_types_2")]
fn rust_multi_agent_wrapper_2_types_2() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[1].clone())
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

#[test_dep(tagged_as = "counter_agent")]
fn rust_counter_agent() -> GeneratedPackage {
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
        snapshotting: Snapshotting::Disabled(Empty {}),
    };

    GeneratedPackage::new(agent_type)
}

#[test]
fn bridge_rust_compiles_single_agent_wrapper(
    #[tagged_as("single_agent_wrapper_types_1")] _pkg: &GeneratedPackage,
) {
    // The test_dep ensures it was compiled successfully in generate_and_compile
}

#[test]
fn bridge_rust_compiles_multi_agent_1(
    #[tagged_as("multi_agent_wrapper_2_types_1")] _pkg: &GeneratedPackage,
) {
    //     The test_dep ensures it was compiled successfully in generate_and_compile
}

#[test]
fn bridge_rust_compiles_multi_agent_2(
    #[tagged_as("multi_agent_wrapper_2_types_2")] _pkg: &GeneratedPackage,
) {
    // The test_dep ensures it was compiled successfully in generate_and_compile
}

#[test]
fn bridge_rust_compiles_counter_agent(#[tagged_as("counter_agent")] _pkg: &GeneratedPackage) {
    // The test_dep ensures it was compiled successfully in generate_and_compile
}

#[test]
fn bridge_rust_compiles_ts_ode_first_snippets_foo_agent(
    #[tagged_as("ts_code_first_snippets_foo_agent")] _pkg: &GeneratedPackage,
) {
    // The test_dep ensures it was compiled successfully in generate_and_compile
}

#[test]
fn bridge_rust_compiles_ts_code_first_snippets_bar_agent(
    #[tagged_as("ts_code_first_snippets_bar_agent")] _pkg: &GeneratedPackage,
) {
    // The test_dep ensures it was compiled successfully in generate_and_compile
}

#[test]
fn bridge_rust_compiles_rust_code_first_snippets_foo_agent(
    #[tagged_as("rust_code_first_snippets_foo_agent")] _pkg: &GeneratedPackage,
) {
    // The test_dep ensures it was compiled successfully in generate_and_compile
}

#[test]
fn bridge_rust_compiles_rust_code_first_snippets_bar_agent(
    #[tagged_as("rust_code_first_snippets_bar_agent")] _pkg: &GeneratedPackage,
) {
    // The test_dep ensures it was compiled successfully in generate_and_compile
}

fn generate_and_compile(agent_type: AgentType, target_dir: &Utf8Path) {
    println!(
        "Generating Rust bridge SDK for {} ({}) into: {}",
        agent_type.type_name, agent_type.description, target_dir
    );

    let mut gen = RustBridgeGenerator::new(agent_type, target_dir, true).unwrap();
    gen.generate().expect("Failed to generate Rust bridge");

    let cwd = std::env::current_dir().expect("Failed to get current directory");
    let shared_target_dir = cwd.join("../../target/shared_bridge_tests");

    let status = std::process::Command::new("cargo")
        .arg("check")
        .arg("--manifest-path")
        .arg(target_dir.join("Cargo.toml").as_std_path())
        .arg("--target-dir")
        .arg(&shared_target_dir)
        .status()
        .expect("failed to run `cargo check`");
    assert!(status.success(), "`cargo check` failed: {:?}", status);

    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(target_dir.join("Cargo.toml").as_std_path())
        .arg("--target-dir")
        .arg(&shared_target_dir)
        .status()
        .expect("failed to run `cargo build`");
    assert!(status.success(), "`cargo build` failed: {:?}", status);
}

#[test]
fn test_rust_type_naming_rust_foo() {
    test_type_naming::<RustTypeName>(GuestLanguage::Rust, "FooAgent");
}

#[test]
fn test_rust_type_naming_rust_bar() {
    test_type_naming::<RustTypeName>(GuestLanguage::Rust, "BarAgent");
}

#[test]
fn test_rust_type_naming_ts_foo() {
    test_type_naming::<RustTypeName>(GuestLanguage::TypeScript, "FooAgent");
}

#[test]
fn test_rust_type_naming_ts_bar() {
    test_type_naming::<RustTypeName>(GuestLanguage::TypeScript, "BarAgent");
}
