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

use golem_cli::{package, versions};
use serde_json::Value;
use std::fs;
use std::io::Cursor;
use std::net::TcpListener;
use std::path::Path;
use test_r::{test, timeout};
use tokio::process::Command;

async fn run_cli(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_golem-cli"))
        .kill_on_drop(true)
        .current_dir(dir)
        .arg("--config-dir")
        .arg(dir.join("isolated-config"))
        .args(["--format", "json"])
        .args(args)
        .output()
        .await
        .unwrap()
}

async fn cli(dir: &Path, args: &[&str]) -> Value {
    let result = run_cli(dir, args).await;
    assert!(
        result.status.success(),
        "{args:?}: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
#[timeout("2m")]
async fn package_cli_is_reproducible_and_publish_dry_run_is_offline() {
    let endpoint = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    endpoint.set_nonblocking(true).unwrap();
    let manifest = format!(
        r#"
manifestVersion: {}
app: packaged-tool
includes:
- settings/golem.yaml
environments:
  local:
    server:
      url: http://{}
      auth:
        staticToken: "00000000-0000-0000-0000-000000000000"
      allowInsecure: true
components:
  app:agent:
    componentWasm: agent.wasm
  app:tool:
    componentWasm: tool.wasm
tools:
  permission-probe: {{}}
  validation-probe: {{}}
  chain-probe:
    component: app:tool
    files:
    - sourcePath: ./hello world.txt
      targetPath: /hello.txt
    config:
      example: ./not-an-input.txt
"#,
        versions::sdk::MANIFEST,
        endpoint.local_addr().unwrap()
    );
    let source = crate::workspace_path()
        .join("test-components/golem_it_tool_runtime_bypass_provider_release.wasm");
    let mut results = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("golem.yaml"), &manifest).unwrap();
        let settings = dir.path().join("settings");
        fs::create_dir(&settings).unwrap();
        let agent_manifest = format!(
            "manifestVersion: {}\nagents:\n  PackageFileAgent:\n    config:\n      file: ./sample.txt\n      text: ./sample.txt\n",
            versions::sdk::MANIFEST
        );
        fs::write(settings.join("golem.yaml"), &agent_manifest).unwrap();
        fs::write(settings.join("sample.txt"), "from the declaring manifest\n").unwrap();
        fs::write(dir.path().join("sample.txt"), "from the app root\n").unwrap();
        fs::write(dir.path().join("hello world.txt"), "portable file\n").unwrap();
        fs::copy(
            crate::workspace_path().join("test-components/it_agent_counters_release.wasm"),
            dir.path().join("agent.wasm"),
        )
        .expect("Build the agent-counters fixture first");
        fs::copy(&source, dir.path().join("tool.wasm")).unwrap_or_else(|e| {
            panic!(
                "Build the tool-runtime-bypass provider fixture first ({}): {e}",
                source.display()
            )
        });
        let dry_run = cli(dir.path(), &["publish", "--dry-run"]).await;
        assert!(!dir.path().join("golem-package.tar").exists());
        let packed = cli(dir.path(), &["package", "pack"]).await;
        assert_eq!(packed, dry_run);
        let bytes = fs::read(dir.path().join("golem-package.tar")).unwrap();
        let package = package::read(Cursor::new(&bytes)).unwrap();
        assert_eq!(packed["digest"], package.digest());
        let components = &package.config().components;
        assert_eq!(components.len(), 2);
        let find = |name| {
            components
                .iter()
                .find(|(key, _)| key.as_str() == name)
                .unwrap()
                .1
        };
        assert!(
            find("app:agent")
                .metadata
                .agent_types
                .iter()
                .any(|agent| agent.type_name.0 == "Counter")
        );
        let component = find("app:tool");
        let file_agent = find("app:agent");
        let declaration = file_agent
            .metadata
            .agent_types
            .iter()
            .find(|agent| agent.type_name.0 == "PackageFileAgent")
            .unwrap();
        for (field, expected) in [("file", true), ("text", false)] {
            let field = declaration
                .config
                .iter()
                .find(|item| item.path == [field])
                .unwrap();
            let ty = declaration.schema.resolve_ref(&field.value_type).unwrap();
            assert_eq!(golem_schema::schema::File::is_declared_type(ty), expected);
        }
        let defaults = &file_agent.agents[&declaration.type_name];
        assert_eq!(defaults.config["text"], "./sample.txt");
        let [file] = defaults.file_defaults.as_slice() else {
            panic!("Only the declared File should be captured")
        };
        assert_eq!(file.path, ["file"]);
        assert_eq!(
            package.blob(&file.digest),
            Some(&b"from the declaring manifest\n"[..])
        );
        assert!(
            component
                .metadata
                .tools
                .iter()
                .any(|tool| tool.name() == Some("chain-probe"))
        );
        let provision = component.tools.values().next().unwrap();
        assert_eq!(
            provision
                .files
                .as_ref()
                .unwrap()
                .entries
                .values()
                .next()
                .unwrap()
                .target
                .as_abs_str(),
            "/hello.txt"
        );
        assert_eq!(provision.config["example"], "./not-an-input.txt");
        let file_digest = &provision.files.as_ref().unwrap().layer;
        let mut layer = tar::Archive::new(package.blob(file_digest).unwrap());
        let mut entries = layer.entries().unwrap();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entries.next().unwrap().unwrap(), &mut content).unwrap();
        assert_eq!(content, "portable file\n");
        if results.is_empty() {
            fs::write(
                settings.join("golem.yaml"),
                agent_manifest.replacen("./sample.txt", "..", 1),
            )
            .unwrap();
            let refused = run_cli(dir.path(), &["publish", "--dry-run"]).await;
            assert!(!refused.status.success());
            assert!(
                String::from_utf8_lossy(&refused.stderr)
                    .contains("cannot capture the application root")
                    || String::from_utf8_lossy(&refused.stdout)
                        .contains("cannot capture the application root")
            );

            let outside = tempfile::NamedTempFile::new_in(dir.path().parent().unwrap()).unwrap();
            let outside_name = outside.path().file_name().unwrap().to_str().unwrap();
            fs::write(
                settings.join("golem.yaml"),
                agent_manifest.replacen("./sample.txt", &format!("../../{outside_name}"), 1),
            )
            .unwrap();
            let refused = run_cli(dir.path(), &["publish", "--dry-run"]).await;
            assert!(!refused.status.success());
            assert!(
                String::from_utf8_lossy(&refused.stderr).contains("leaves the application root")
                    || String::from_utf8_lossy(&refused.stdout)
                        .contains("leaves the application root")
            );
        }
        results.push((packed, bytes));
    }
    assert!(matches!(endpoint.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    assert!(
        results[0] == results[1],
        "different directories produced different package bytes"
    );
}
