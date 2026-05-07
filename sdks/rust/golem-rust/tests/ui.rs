// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

test_r::enable!();

#[cfg(feature = "export_golem_agentic")]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use test_r::test;

    #[test]
    fn agent_implementation_annotation_ui_tests() {
        let t = trybuild::TestCases::new();
        t.compile_fail("tests/ui/fail/*.rs");
        t.pass("tests/ui/pass/*.rs");
    }

    #[test]
    fn agent_definition_implementation_and_client_can_live_in_separate_crates() {
        let workspace = create_cross_crate_workspace();
        let target_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("golem-rust crate should have an SDK workspace parent")
            .join("target");

        let output = Command::new("cargo")
            .arg("check")
            .arg("--workspace")
            .arg("--quiet")
            .env("CARGO_TARGET_DIR", target_dir)
            .current_dir(&workspace)
            .output()
            .expect("failed to run cargo check for cross-crate agent workspace");

        fs::remove_dir_all(&workspace).unwrap_or_else(|error| {
            panic!(
                "failed to remove temporary workspace {}: {error}",
                workspace.display()
            )
        });

        if !output.status.success() {
            panic!(
                "cross-crate agent workspace failed to compile\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    fn create_cross_crate_workspace() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "golem-rust-agent-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos()
        ));

        fs::create_dir_all(root.join("agent-api/src")).unwrap();
        fs::create_dir_all(root.join("agent-impl/src")).unwrap();
        fs::create_dir_all(root.join("agent-client/src")).unwrap();

        let golem_rust_path = Path::new(env!("CARGO_MANIFEST_DIR"));

        fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
resolver = "2"
members = ["agent-api", "agent-impl", "agent-client"]
"#,
        )
        .unwrap();

        write_crate_manifest(
            &root,
            "agent-api",
            &format!(
                r#"
[dependencies]
golem-rust = {{ path = {}, features = ["export_golem_agentic"] }}
"#,
                toml_string(golem_rust_path)
            ),
        );
        fs::write(
            root.join("agent-api/src/lib.rs"),
            r#"
use golem_rust::agent_definition;

#[agent_definition]
pub trait CrossCrateAgent {
    fn new(id: String) -> Self;
    fn ping(&self) -> String;
}
"#,
        )
        .unwrap();

        write_crate_manifest(
            &root,
            "agent-impl",
            &format!(
                r#"
[dependencies]
agent-api = {{ path = "../agent-api" }}
golem-rust = {{ path = {}, features = ["export_golem_agentic"] }}
"#,
                toml_string(golem_rust_path)
            ),
        );
        fs::write(
            root.join("agent-impl/src/lib.rs"),
            r#"
use agent_api::CrossCrateAgent;
use golem_rust::agent_implementation;

pub struct CrossCrateAgentImpl {
    id: String,
}

#[agent_implementation]
impl CrossCrateAgent for CrossCrateAgentImpl {
    fn new(id: String) -> Self {
        Self { id }
    }

    fn ping(&self) -> String {
        self.id.clone()
    }
}
"#,
        )
        .unwrap();

        write_crate_manifest(
            &root,
            "agent-client",
            r#"
[dependencies]
agent-api = { path = "../agent-api" }
"#,
        );
        fs::write(
            root.join("agent-client/src/lib.rs"),
            r#"
use agent_api::CrossCrateAgentClient;

pub fn generated_client_type_is_available() -> usize {
    std::mem::size_of::<CrossCrateAgentClient>()
}
"#,
        )
        .unwrap();

        root
    }

    fn write_crate_manifest(root: &Path, name: &str, dependencies: &str) {
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                r#"
[package]
name = "{name}"
version = "0.0.0"
edition = "2024"

{dependencies}
"#
            ),
        )
        .unwrap();
    }

    fn toml_string(path: &Path) -> String {
        format!("{:?}", path.display().to_string())
    }
}
