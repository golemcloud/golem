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
    agent, code_first_snippets_agent_type, field, method, multi_agent_wrapper_2_types,
    single_agent_wrapper_types,
};
use crate::bridge_gen::type_naming::test_type_naming;
use camino::Utf8Path;
use golem_cli::bridge_gen::rust::{RustBridgeGenerator, RustTypeName};
use golem_cli::bridge_gen::{BridgeGenerator, BridgeMode, bridge_client_directory_name};
use golem_cli::model::GuestLanguage;
use golem_common::model::agent::AgentMode;
use golem_common::schema::{AgentTypeSchema, SchemaType};
use tempfile::TempDir;
use test_r::{test, test_dep};

struct GeneratedPackage {
    #[allow(dead_code)]
    pub dir: TempDir,
}

impl GeneratedPackage {
    pub fn new(agent_type: AgentTypeSchema) -> Self {
        let dir = TempDir::new().unwrap();
        let target_dir = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::remove_dir_all(target_dir).ok();
        generate_and_compile(agent_type, target_dir);
        GeneratedPackage { dir }
    }
}

#[test_dep(scope = PerWorker, tagged_as = "single_agent_wrapper_types_1")]
fn rust_single_agent_wrapper_1() -> GeneratedPackage {
    GeneratedPackage::new(single_agent_wrapper_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_wrapper_2_types_1")]
fn rust_multi_agent_wrapper_2_types_1() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[0].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "multi_agent_wrapper_2_types_2")]
fn rust_multi_agent_wrapper_2_types_2() -> GeneratedPackage {
    GeneratedPackage::new(multi_agent_wrapper_2_types()[1].clone())
}

#[test_dep(scope = PerWorker, tagged_as = "rust_code_first_snippets_foo_agent")]
fn rust_code_first_snippets_foo_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(
        GuestLanguage::Rust,
        "FooAgent",
    ))
}

#[test_dep(scope = PerWorker, tagged_as = "counter_agent")]
fn rust_counter_agent() -> GeneratedPackage {
    GeneratedPackage::new(agent(
        "CounterAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![method("increment", vec![], Some(SchemaType::f64()))],
        vec![],
        AgentMode::Durable,
    ))
}

#[test]
fn bridge_rust_compiles_single_agent_wrapper(
    #[tagged_as("single_agent_wrapper_types_1")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn bridge_rust_compiles_multi_agent_1(
    #[tagged_as("multi_agent_wrapper_2_types_1")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn bridge_rust_compiles_multi_agent_2(
    #[tagged_as("multi_agent_wrapper_2_types_2")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn bridge_rust_compiles_counter_agent(#[tagged_as("counter_agent")] _pkg: &GeneratedPackage) {}

#[test]
fn bridge_rust_compiles_rust_code_first_snippets_foo_agent(
    #[tagged_as("rust_code_first_snippets_foo_agent")] _pkg: &GeneratedPackage,
) {
}

#[test]
fn bridge_rust_ephemeral_agent_skips_non_phantom_constructors() {
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    let mut agent_type = agent(
        "EphemeralConfigAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![method("get", vec![], Some(SchemaType::string()))],
        vec![],
        AgentMode::Ephemeral,
    );
    agent_type.constructor.name = Some("EphemeralConfigAgent".to_string());
    let package_dir = target_dir.join(bridge_client_directory_name(
        &agent_type.type_name,
        BridgeMode::External,
    ));
    let mut generator = RustBridgeGenerator::new(agent_type, &package_dir, true).unwrap();
    generator.generate().unwrap();

    let lib_rs = std::fs::read_to_string(package_dir.join("src/lib.rs")).unwrap();
    assert!(lib_rs.contains("pub struct EphemeralConfigAgent"));
    assert!(!lib_rs.contains("pub async fn new("));
}

#[test]
fn bridge_rust_external_consumer_can_configure_with_only_generated_dependency() {
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    let agent_type = agent(
        "CounterAgent",
        "rust",
        vec![field("name", SchemaType::string())],
        vec![],
        vec![],
        AgentMode::Durable,
    );
    let package_dir = target_dir.join(bridge_client_directory_name(
        &agent_type.type_name,
        BridgeMode::External,
    ));
    let mut generator = RustBridgeGenerator::new(agent_type, &package_dir, true).unwrap();
    generator.generate().unwrap();

    let consumer_dir = target_dir.join("consumer");
    std::fs::create_dir_all(consumer_dir.join("src")).unwrap();
    std::fs::write(
        consumer_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "generated-bridge-consumer"
version = "0.0.1"
edition = "2021"

[workspace]

[dependencies]
counter-agent-client = {{ path = {package_dir:?} }}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        consumer_dir.join("src/main.rs"),
        r#"fn main() {
    counter_agent_client::configure(counter_agent_client::GolemServer::Local, "app", "local");
}
"#,
    )
    .unwrap();

    let shared_target_dir = crate::workspace_path().join("target/shared_bridge_tests");
    let output = std::process::Command::new("cargo")
        .arg("check")
        .arg("--target-dir")
        .arg(&shared_target_dir)
        .current_dir(&consumer_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated bridge consumer failed to compile with only the generated crate dependency\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bridge_rust_agent_named_golem_server_still_compiles() {
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    let agent_type = agent(
        "GolemServer",
        "rust",
        vec![],
        vec![],
        vec![],
        AgentMode::Durable,
    );

    generate_and_compile(agent_type, target_dir);
}

#[test]
fn bridge_rust_agent_named_bridge_still_compiles() {
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    let agent_type = agent("bridge", "", vec![], vec![], vec![], AgentMode::Durable);

    generate_and_compile(agent_type, target_dir);
}

#[test]
fn test_type_naming_rust_foo_agent() {
    test_type_naming::<RustTypeName>(GuestLanguage::Rust, "FooAgent");
}

#[test]
fn test_type_naming_ts_foo_agent_for_rust_bridge() {
    test_type_naming::<RustTypeName>(GuestLanguage::TypeScript, "FooAgent");
}

fn generate_and_compile(agent_type: AgentTypeSchema, target_dir: &Utf8Path) {
    let package_dir = target_dir.join(bridge_client_directory_name(
        &agent_type.type_name,
        BridgeMode::External,
    ));
    let mut generator = RustBridgeGenerator::new(agent_type, &package_dir, true).unwrap();
    generator.generate().unwrap();

    // Share a single cargo target directory across all bridge compile tests so
    // the generated client crates' dependencies are built once and reused,
    // instead of recompiling them from scratch in each per-test temp dir.
    let shared_target_dir = crate::workspace_path().join("target/shared_bridge_tests");

    assert!(
        std::process::Command::new("cargo")
            .arg("check")
            .arg("--target-dir")
            .arg(&shared_target_dir)
            .current_dir(&package_dir)
            .status()
            .unwrap()
            .success()
    );
}
