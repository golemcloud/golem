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
use camino::{Utf8Path, Utf8PathBuf};
use golem_cli::bridge_gen::{bridge_client_directory_name, BridgeGenerator};
use golem_cli::bridge_gen::typescript::{TypeScriptBridgeGenerator, TypeScriptTypeName};
use golem_cli::model::GuestLanguage;
use golem_common::model::agent::AgentMode;
use golem_common::schema::{AgentTypeSchema, SchemaType};
use tempfile::TempDir;
use test_r::{test, test_dep};

struct GeneratedPackage {
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
    GeneratedPackage::new(agent(
        "CounterAgent",
        "typescript",
        vec![field("name", SchemaType::string())],
        vec![method("increment", vec![], Some(SchemaType::f64()))],
        vec![],
        AgentMode::Durable,
    ))
}

#[test_dep(scope = PerWorker, tagged_as = "ts_code_first_snippets_foo_agent")]
fn ts_code_first_snippets_foo_agent() -> GeneratedPackage {
    GeneratedPackage::new(code_first_snippets_agent_type(GuestLanguage::TypeScript, "FooAgent"))
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
fn bridge_tests_schema_value_encoding(
    #[tagged_as("ts_code_first_snippets_foo_agent")] pkg: &GeneratedPackage,
) {
    let package_dir = generated_package_dir(pkg.target_dir(), "foo-agent");
    let index_ts = std::fs::read_to_string(package_dir.join("src/index.ts")).unwrap();
    assert!(index_ts.contains("kind"));
    assert!(index_ts.contains("SchemaValue"));
}

#[test]
fn test_type_naming_ts_foo_agent() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::TypeScript, "FooAgent");
}

#[test]
fn test_type_naming_rust_foo_agent_for_ts_bridge() {
    test_type_naming::<TypeScriptTypeName>(GuestLanguage::Rust, "FooAgent");
}

fn generate_and_compile(agent_type: AgentTypeSchema, target_dir: &Utf8Path) {
    let package_name = bridge_client_directory_name(&agent_type.type_name);
    let mut generator = TypeScriptBridgeGenerator::new(agent_type, target_dir, true).unwrap();
    generator.generate().unwrap();

    let package_dir = target_dir.join(package_name);
    assert!(std::process::Command::new("npm")
        .arg("install")
        .current_dir(&package_dir)
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(&package_dir)
        .status()
        .unwrap()
        .success());
}

fn generated_package_dir(target_dir: &Utf8Path, package_name: &str) -> Utf8PathBuf {
    target_dir.join(format!("{package_name}-client"))
}
