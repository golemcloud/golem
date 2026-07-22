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
    agent, code_first_snippets_agent_type, def, field, local_config, method,
    multi_agent_wrapper_2_types, ref_to, single_agent_wrapper_types,
};
use crate::bridge_gen::type_naming::test_type_naming;
use camino::{Utf8Path, Utf8PathBuf};
use golem_cli::bridge_gen::rust::{RustBridgeGenerator, RustBridgeMode, RustTypeName};
use golem_cli::bridge_gen::{BridgeGenerator, BridgeMode, bridge_client_directory_name};
use golem_cli::model::GuestLanguage;
use golem_common::model::Empty;
use golem_common::model::agent::{AgentConfigSource, AgentMode, AgentTypeName, Snapshotting};
use golem_common::schema::agent::{
    AgentConfigDeclarationSchema, AgentConstructorSchema, AgentMethodSchema, InputSchema,
    OutputSchema,
};
use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
use golem_common::schema::metadata::TypeId;
use golem_common::schema::schema_type::{BinaryRestrictions, TextRestrictions};
use golem_common::schema::tool::{
    BoolFlagShape, CommandBody, CommandIndex, CommandNode, CommandTree, Doc, ErrorCase, ErrorKind,
    FlagShape, FlagSpec, Globals, OptionShape, OptionSpec, Positional, Positionals,
    ResultSpec as ToolResultSpec, StreamSpec, TailPositional, Tool,
};
use golem_common::schema::unstructured::{
    unstructured_binary_schema_type, unstructured_text_schema_type,
};
use golem_common::schema::{
    AgentTypeSchema, MetadataEnvelope, NamedField, NamedFieldType, SchemaType,
};
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
    assert!(lib_rs.contains("pub async fn new_phantom("));
    assert!(!lib_rs.contains("pub async fn get_phantom("));
    assert!(!lib_rs.contains("Uuid::new_v4"));
    assert!(lib_rs.contains(
        "return Ok(Self {\n            constructor_parameters,\n            phantom_id: None,"
    ));
}

#[test]
fn bridge_rust_ephemeral_metadata_wrapper_does_not_collide_with_schema_type() {
    let dir = TempDir::new().unwrap();
    let target_dir = Utf8Path::from_path(dir.path()).unwrap();
    let agent_type = agent(
        "AlphaAgent",
        "typescript",
        vec![],
        vec![method(
            "run",
            vec![],
            Some(ref_to("AlphaAgentInvocationResult")),
        )],
        vec![def(
            "AlphaAgentInvocationResult",
            SchemaType::record(vec![]),
        )],
        AgentMode::Ephemeral,
    );

    generate_and_compile(agent_type, target_dir);
}

#[test]
fn guest_generation_uses_logical_ephemeral_proxies_and_invocation_metadata() {
    let dir = TempDir::new().unwrap();
    let target_path = Utf8Path::from_path(dir.path()).unwrap();
    let mut agent_type = agent(
        "AlphaAgent",
        "rust",
        vec![],
        vec![method(
            "run",
            vec![field("value", SchemaType::s32())],
            Some(SchemaType::string()),
        )],
        vec![],
        AgentMode::Ephemeral,
    );
    agent_type.config = vec![local_config(vec!["api-key"], SchemaType::string())];
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
    for ephemeral_shape in [
        "pub use golem_rust::agentic::EphemeralInvocationResult",
        "pub fn new_phantom(",
        "pub fn new_phantom_with_config(",
        "async_invoke_and_await",
        ".invoke(",
        "schedule_invocation",
        "schedule_cancelable_invocation",
    ] {
        assert!(
            lib_rs.contains(ephemeral_shape),
            "missing ephemeral guest wasm-rpc API shape {ephemeral_shape}:\n{lib_rs}"
        );
    }
    assert!(!lib_rs.contains("pub struct AlphaAgentInvocationResult"));
    assert!(!lib_rs.contains("_with_metadata"));
    assert!(!lib_rs.contains("pub fn get("));
    assert!(!lib_rs.contains("pub fn get_phantom("));
    assert!(!lib_rs.contains("pub fn get_with_config("));
    assert!(!lib_rs.contains("pub fn get_phantom_with_config("));
    assert!(!lib_rs.contains("Uuid::new_v4"));

    let shared_target_dir = crate::workspace_path().join("target/shared_bridge_tests");
    let output = std::process::Command::new("cargo")
        .arg("check")
        .arg("--target-dir")
        .arg(shared_target_dir)
        .current_dir(target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated ephemeral guest crate failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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

// Compiler-backed generator checks live in the integration target.
#[test]
fn guest_runtime_prelude_compiles_with_generated_golem_rust_dependency_flags() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("media-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("MediaAgent");
    agent_type.mode = AgentMode::Durable;
    agent_type.methods.push(AgentMethodSchema {
        name: "convert".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
            "input",
            unstructured_binary_schema_type(BinaryRestrictions {
                mime_types: Some(vec!["application/json".to_string()]),
                ..Default::default()
            }),
        )]),
        output_schema: OutputSchema::Single(Box::new(unstructured_text_schema_type(
            TextRestrictions::default(),
        ))),
        http_endpoint: vec![],
        read_only: None,
    });
    agent_type.methods.push(AgentMethodSchema {
        name: "upload".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
            "input",
            unstructured_binary_schema_type(BinaryRestrictions::default()),
        )]),
        output_schema: OutputSchema::Unit,
        http_endpoint: vec![],
        read_only: None,
    });
    agent_type.methods.push(AgentMethodSchema {
        name: "submit".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
            "input",
            unstructured_text_schema_type(TextRestrictions::default()),
        )]),
        output_schema: OutputSchema::Unit,
        http_endpoint: vec![],
        read_only: None,
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
    assert!(
        lib_rs.contains("impl crate::__golem_bridge_runtime::agentic::AllowedMimeTypes"),
        "the relocated prelude test must still exercise the generated MIME restriction API"
    );
    assert!(
        lib_rs.contains("UnstructuredBinary<String>"),
        "the relocated prelude test no longer exercises unrestricted binary values"
    );
    assert!(
        lib_rs.contains("::__golem_bridge_runtime::agentic::UnstructuredText as crate")
            && lib_rs.contains("::to_schema_value(input)"),
        "the relocated prelude test no longer exercises unrestricted text encoding:\n{lib_rs}"
    );

    cargo_check(&target_path);
}

#[test]
fn guest_generation_emits_wasm_rpc_cargo_dependencies_and_api_shape() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    agent_type.methods.push(AgentMethodSchema {
        name: "run".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
            "value",
            SchemaType::s32(),
        )]),
        output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
        http_endpoint: vec![],
        read_only: None,
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let cargo_toml = std::fs::read_to_string(target_path.join("Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("name = \"alpha-agent-guest-client\""));
    assert!(cargo_toml.contains("golem-rust"));
    assert!(cargo_toml.contains("export_golem_agentic"));
    assert!(!cargo_toml.contains("golem-client"));
    assert!(!cargo_toml.contains("reqwest"));

    let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
    for guest_shape in [
        "wasm_rpc: golem_rust::golem_agentic::golem::agent::host::WasmRpc",
        "pub fn get(",
        "pub fn get_phantom(",
        "pub fn new_phantom(",
        "golem_rust::golem_agentic::golem::agent::host::WasmRpc::new",
        "pub async fn run(\n        &self,\n        value: i32,",
        "pub fn trigger_run(\n        &self,\n        value: i32,",
        "pub fn schedule_run(\n        &self,\n        value: i32,\n        golem_bridge_scheduled_time: golem_rust::wasip2::clocks::wall_clock::Datetime,",
        "pub fn schedule_cancelable_run(\n        &self,\n        value: i32,\n        golem_bridge_scheduled_time: golem_rust::wasip2::clocks::wall_clock::Datetime,",
        "async_invoke_and_await",
        "await_invoke_schema_value_result",
        ".invoke(",
        "schedule_invocation",
        "schedule_cancelable_invocation",
        "MissingResult",
    ] {
        assert!(
            lib_rs.contains(guest_shape),
            "missing guest wasm-rpc API shape {guest_shape}:\n{lib_rs}"
        );
    }
    assert!(!lib_rs.contains("pub fn __golem_bridge_trigger_run"));
    assert!(!lib_rs.contains("pub fn __golem_bridge_schedule_run"));
    assert!(!lib_rs.contains("golem_client"));
    assert!(!lib_rs.contains("reqwest"));
    assert!(!lib_rs.contains("constructor_parameters"));
    assert!(!lib_rs.contains("agent_id: String"));
    assert!(!lib_rs.contains("phantom_id: Option<golem_rust::Uuid>,\n                wasm_rpc"));
    assert!(!lib_rs.contains("golem_rust::golem_agentic::golem::agent::host::make_agent_id"));

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate cargo check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_with_non_copy_constructor_parameters() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    agent_type.constructor.input_schema = InputSchema::parameters(vec![NamedField::user_supplied(
        "name",
        SchemaType::string(),
    )]);
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with non-Copy constructor parameter failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_constructor_parameter_matches_internal_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    agent_type.constructor.input_schema = InputSchema::parameters(vec![NamedField::user_supplied(
        "phantom_id",
        SchemaType::s32(),
    )]);
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with constructor parameter matching an internal name failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_agent_type_name_matches_guest_client_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("client-error-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("ClientError");
    agent_type.mode = AgentMode::Durable;
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with agent type named ClientError failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_agent_type_name_matches_guest_runtime_module() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("runtime-module-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("__golem_bridge_runtime");
    agent_type.source_language = "rust".to_string();
    agent_type.mode = AgentMode::Durable;
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with agent type named __golem_bridge_runtime failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_method_parameter_is_named_when() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.methods.push(AgentMethodSchema {
        name: "run".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
            "when",
            SchemaType::s32(),
        )]),
        output_schema: OutputSchema::Unit,
        http_endpoint: vec![],
        read_only: None,
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with method parameter named when failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_method_name_matches_phantom_id_helper() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.methods.push(AgentMethodSchema {
        name: "phantom_id".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![]),
        output_schema: OutputSchema::Unit,
        http_endpoint: vec![],
        read_only: None,
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with method named phantom_id failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_method_name_matches_removed_agent_id_field() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.methods.push(AgentMethodSchema {
        name: "agent_id".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![]),
        output_schema: OutputSchema::Unit,
        http_endpoint: vec![],
        read_only: None,
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with method named agent_id failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_method_name_matches_guest_get_helper() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    agent_type.methods.push(AgentMethodSchema {
        name: "get".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![]),
        output_schema: OutputSchema::Unit,
        http_endpoint: vec![],
        read_only: None,
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with method named get failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_get_helper_deconflicted_name_matches_user_method() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    for method_name in ["get", "get_1"] {
        agent_type.methods.push(AgentMethodSchema {
            name: method_name.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
    }
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with methods named get and get_1 failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_method_name_matches_trigger_wrapper_for_another_method() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    for method_name in ["run", "trigger_run"] {
        agent_type.methods.push(AgentMethodSchema {
            name: method_name.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
    }
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with method named trigger_run next to run failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_reserved_trigger_wrapper_suffix_matches_another_wrapper() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.source_language = "rust".to_string();
    for method_name in ["run", "__golem_bridge_trigger_run", "run_1"] {
        agent_type.methods.push(AgentMethodSchema {
            name: method_name.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
    }
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with a reserved trigger wrapper suffix matching another wrapper failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_rust_method_name_matches_reserved_trigger_wrapper() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.source_language = "rust".to_string();
    for method_name in ["run", "__golem_bridge_trigger_run"] {
        agent_type.methods.push(AgentMethodSchema {
            name: method_name.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        });
    }
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with Rust method named __golem_bridge_trigger_run next to run failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_constructor_parameter_matches_config_parameter_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    agent_type.constructor.input_schema = InputSchema::parameters(vec![NamedField::user_supplied(
        "config_foo",
        SchemaType::s32(),
    )]);
    agent_type.config.push(AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["foo".to_string()],
        value_type: SchemaType::string(),
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with constructor parameter matching local config parameter failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_emits_self_contained_typed_config_schema_values() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    let config_type_id = TypeId::new("config-shared");
    agent_type.schema = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: config_type_id.clone(),
            name: Some("ConfigShared".to_string()),
            body: SchemaType::record(vec![NamedFieldType {
                name: "label".to_string(),
                body: SchemaType::string(),
                metadata: MetadataEnvelope::default(),
            }]),
        }],
        root: SchemaType::record(vec![]),
    };
    agent_type.config.push(AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["shared".to_string()],
        value_type: SchemaType::ref_to(config_type_id),
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
    assert!(
        lib_rs.contains("config-shared"),
        "generated typed config schema graph must include referenced definitions:\n{lib_rs}"
    );
    assert!(
        lib_rs.contains("TypedSchemaValue::new"),
        "generated typed config encoding must build a typed value:\n{lib_rs}"
    );
    assert!(
        lib_rs.contains("golem_rust::encode_typed_schema_value"),
        "generated typed config encoding must use guest golem-rust wire encoding:\n{lib_rs}"
    );

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with referenced local config type failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_constructor_parameter_is_named_agent_config() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.mode = AgentMode::Durable;
    agent_type.constructor.input_schema = InputSchema::parameters(vec![NamedField::user_supplied(
        "agent_config",
        SchemaType::s32(),
    )]);
    agent_type.config.push(AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["foo".to_string()],
        value_type: SchemaType::string(),
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with constructor parameter named agent_config failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_rust_constructor_parameter_matches_reserved_phantom_id_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.source_language = "rust".to_string();
    agent_type.mode = AgentMode::Durable;
    agent_type.constructor.input_schema = InputSchema::parameters(vec![NamedField::user_supplied(
        "__golem_bridge_phantom_id",
        SchemaType::s32(),
    )]);
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with Rust constructor parameter matching reserved phantom id name failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guest_generation_compiles_when_method_parameter_names_sanitize_to_same_ident() {
    let dir = tempfile::TempDir::new().unwrap();
    let target_path =
        Utf8PathBuf::from_path_buf(dir.path().join("alpha-agent-guest-client")).unwrap();
    let mut agent_type = minimal_agent_type("AlphaAgent");
    agent_type.methods.push(AgentMethodSchema {
        name: "run".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(vec![
            NamedField::user_supplied("foo-bar", SchemaType::s32()),
            NamedField::user_supplied("foo_bar", SchemaType::s32()),
        ]),
        output_schema: OutputSchema::Unit,
        http_endpoint: vec![],
        read_only: None,
    });
    let mut generator = RustBridgeGenerator::new_with_mode(
        agent_type,
        &target_path,
        true,
        RustBridgeMode::GuestWasmRpc,
    )
    .unwrap();

    generator.generate().unwrap();

    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(&target_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "generated guest crate with method parameters that sanitize to the same Rust ident failed cargo check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn generate_tool(tool: golem_common::schema::tool::Tool, dir_name: &str) -> (TempDir, Utf8PathBuf) {
    use golem_cli::bridge_gen::rust::tool::RustToolBridgeGenerator;
    let dir = TempDir::new().unwrap();
    let target_path = Utf8Path::from_path(dir.path()).unwrap().join(dir_name);
    let mut generator = RustToolBridgeGenerator::new(tool, &target_path, true).unwrap();
    generator.generate().unwrap();
    (dir, target_path)
}

fn cargo_check(target_path: &Utf8Path) {
    let output = std::process::Command::new("cargo")
        .arg("check")
        .current_dir(target_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tool_generation_compiles() {
    let (_dir, target_path) = generate_tool(grep_tool(), "grep-tool-guest-client");
    cargo_check(&target_path);
}
#[test]
fn colliding_names_tool_generation_compiles() {
    let (_dir, target_path) = generate_tool(colliding_names_tool(), "new-tool-guest-client");
    let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
    for shape in [
        "pub fn new() -> Self",
        "pub async fn new1(",
        "pub async fn new2(",
    ] {
        assert!(lib_rs.contains(shape), "missing {shape}:\n{lib_rs}");
    }
    cargo_check(&target_path);
}
#[test]
fn subtree_tool_generation_emits_and_compiles() {
    let (_dir, target_path) = generate_tool(git_tool(), "git-tool-guest-client");
    let lib_rs = std::fs::read_to_string(target_path.join("src/lib.rs")).unwrap();
    for shape in [
        "pub struct GitStashClient",
        "pub fn stash(",
        "pub async fn pop(",
    ] {
        assert!(lib_rs.contains(shape), "missing {shape}:\n{lib_rs}");
    }
    cargo_check(&target_path);
}

fn minimal_agent_type(type_name: &str) -> AgentTypeSchema {
    AgentTypeSchema {
        type_name: AgentTypeName(type_name.to_string()),
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
        },
        methods: vec![],
        dependencies: vec![],
        mode: AgentMode::Ephemeral,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    }
}

fn doc(summary: &str) -> Doc {
    Doc {
        summary: summary.to_string(),
        description: String::new(),
        examples: vec![],
    }
}

fn body() -> CommandBody {
    CommandBody {
        positionals: Positionals::default(),
        options: vec![],
        flags: vec![],
        constraints: vec![],
        stdin: None,
        stdout: None,
        result: None,
        errors: vec![],
        annotations: None,
    }
}

fn tool_node(name: &str) -> CommandNode {
    CommandNode {
        name: name.to_string(),
        aliases: vec![],
        doc: doc(name),
        globals: Globals::default(),
        subcommands: vec![],
        body: None,
    }
}

fn positional(name: &str, type_: SchemaType) -> Positional {
    Positional {
        name: name.to_string(),
        doc: doc(name),
        value_name: None,
        type_,
        default: None,
        required: true,
        accepts_stdio: false,
    }
}

fn option(long: &str, shape: OptionShape) -> OptionSpec {
    OptionSpec {
        long: long.to_string(),
        short: None,
        aliases: vec![],
        doc: doc(long),
        value_name: None,
        shape,
        default: None,
        required: false,
        env_var: None,
    }
}

fn flag(long: &str, shape: FlagShape) -> FlagSpec {
    FlagSpec {
        long: long.to_string(),
        short: None,
        aliases: vec![],
        doc: doc(long),
        shape,
        env_var: None,
    }
}

fn grep_tool() -> Tool {
    let color_type = TypeId::from("color-mode");
    let color_ref = SchemaType::ref_to(color_type.clone());
    let mut root = tool_node("grep");
    root.globals = Globals {
        options: vec![option("color", OptionShape::Scalar(color_ref))],
        flags: vec![flag(
            "case-sensitive",
            FlagShape::BoolFlag(BoolFlagShape {
                default: false,
                negatable: false,
            }),
        )],
    };
    root.subcommands = vec![CommandIndex(1)];
    root.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![positional("pattern", SchemaType::string())],
            tail: Some(TailPositional {
                name: "files".to_string(),
                doc: doc("files"),
                value_name: None,
                item_type: SchemaType::string(),
                min: 0,
                max: None,
                separator: None,
                verbatim: false,
                accepts_stdio: false,
            }),
        },
        options: vec![option(
            "max-count",
            OptionShape::OptionalScalar(SchemaType::u32()),
        )],
        flags: vec![flag("verbosity", FlagShape::CountFlag(None))],
        result: Some(ToolResultSpec {
            type_: SchemaType::list(SchemaType::string()),
            doc: doc("matches"),
            formatters: vec![],
            default_formatter: String::new(),
        }),
        errors: vec![
            ErrorCase {
                name: "bad-pattern".to_string(),
                doc: doc("bad pattern"),
                kind: ErrorKind::UsageError,
                exit_code: 2,
                payload: Some(SchemaType::string()),
            },
            ErrorCase {
                name: "io".to_string(),
                doc: doc("io"),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: None,
            },
        ],
        ..body()
    });
    let mut replace = tool_node("replace");
    replace.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![
                positional("pattern", SchemaType::string()),
                positional("replacement", SchemaType::string()),
            ],
            tail: None,
        },
        stdout: Some(StreamSpec {
            doc: doc("stdout"),
            mime: vec![],
            required: true,
        }),
        ..body()
    });
    Tool {
        version: "1".to_string(),
        commands: CommandTree {
            nodes: vec![root, replace],
        },
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: color_type,
                name: None,
                body: SchemaType::r#enum(vec![
                    "never".to_string(),
                    "always".to_string(),
                    "auto".to_string(),
                ]),
            }],
            root: SchemaType::record(vec![]),
        },
    }
}

fn git_tool() -> Tool {
    let mut root = tool_node("git");
    root.globals.flags = vec![flag("verbose", FlagShape::CountFlag(None))];
    root.subcommands = vec![CommandIndex(1)];
    let mut stash = tool_node("stash");
    stash.globals.options = vec![option("git-dir", OptionShape::Scalar(SchemaType::string()))];
    stash.subcommands = vec![CommandIndex(2)];
    let mut pop = tool_node("pop");
    pop.body = Some(CommandBody {
        options: vec![option(
            "name",
            OptionShape::OptionalScalar(SchemaType::string()),
        )],
        ..body()
    });
    Tool {
        version: "1".to_string(),
        commands: CommandTree {
            nodes: vec![root, stash, pop],
        },
        schema: SchemaGraph::empty(),
    }
}

fn colliding_names_tool() -> Tool {
    let mut root = tool_node("new");
    root.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![positional("value", SchemaType::string())],
            tail: None,
        },
        errors: vec![
            ErrorCase {
                name: "self".to_string(),
                doc: doc("self"),
                kind: ErrorKind::UsageError,
                exit_code: 2,
                payload: None,
            },
            ErrorCase {
                name: "foo-1".to_string(),
                doc: doc("foo-1"),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: Some(SchemaType::string()),
            },
            ErrorCase {
                name: "foo1".to_string(),
                doc: doc("foo1"),
                kind: ErrorKind::RuntimeError,
                exit_code: 1,
                payload: None,
            },
        ],
        ..body()
    });
    root.subcommands = vec![CommandIndex(1)];
    let mut sub = tool_node("new");
    sub.body = Some(CommandBody {
        positionals: Positionals {
            fixed: vec![positional("value", SchemaType::string())],
            tail: None,
        },
        ..body()
    });
    Tool {
        version: "1".to_string(),
        commands: CommandTree {
            nodes: vec![root, sub],
        },
        schema: SchemaGraph::empty(),
    }
}
