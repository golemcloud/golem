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
#[timeout("45s")]
async fn install_file_package_builds_without_instantiating_and_keeps_lock_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let manifest = format!(
        "# keep me\nmanifestVersion: {}\napp: installer # keep me too\nenvironments:\n  local: {{ server: local }}\n",
        versions::sdk::MANIFEST
    );
    fs::write(root.join("golem.yaml"), &manifest).unwrap();
    for (name, instruction) in [("traps", "unreachable"), ("loops", "(loop br 0)")] {
        let wasm = wat::parse_str(format!(
            "(component (core module $m (func $start {instruction}) (start $start)) (core instance (instantiate $m)))"
        )).unwrap();
        let tar = format!("{name}.tar");
        let digest = write_install_fixture(&root.join(&tar), name, wasm);
        let result = run_cli(root, &["install", &format!("./{tar}")]).await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let edited = fs::read_to_string(root.join("golem.yaml")).unwrap();
        assert!(edited.contains("# keep me\n") && edited.contains("# keep me too\n"));
        assert!(edited.contains("packages:") && edited.contains(&digest));
    }
    let lock = fs::read(root.join("golem.lock")).unwrap();
    assert_eq!(cli(root, &["build"]).await["built"], true);
    assert_eq!(fs::read(root.join("golem.lock")).unwrap(), lock);

    let other = tempfile::tempdir().unwrap();
    write_install_fixture(
        &root.join("traps.tar"),
        "traps",
        b"different bytes".to_vec(),
    );
    let manifest_with_packages = fs::read_to_string(root.join("golem.yaml")).unwrap();
    for file in ["golem.yaml", "golem.lock", "loops.tar", "traps.tar"] {
        fs::copy(root.join(file), other.path().join(file)).unwrap();
    }
    assert_cli_error(other.path(), &["build"], "digest").await;

    fs::write(
        root.join("golem.yaml"),
        manifest_with_packages.replacen("1.2.3", "2.0.0", 1),
    )
    .unwrap();
    for command in ["build", "deploy"] {
        assert_cli_error(root, &[command], "golem install").await;
        assert_eq!(fs::read(root.join("golem.lock")).unwrap(), lock);
    }
}

async fn assert_cli_error(dir: &Path, args: &[&str], expected: &str) {
    let result = run_cli(dir, args).await;
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(
        !result.status.success() && output.contains(expected),
        "{args:?}: {output}"
    );
}

#[test]
#[timeout("45s")]
async fn development_package_build_reads_its_current_component_without_a_lock() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let dev = root.join("dev");
    fs::create_dir(&dev).unwrap();
    let version = versions::sdk::MANIFEST;
    fs::write(
        root.join("golem.yaml"),
        format!("manifestVersion: {version}\napp: consumer\nenvironments:\n  local: {{server: local}}\npackages:\n  editable: {{path: ./dev}}\n"),
    ).unwrap();
    let copy = if cfg!(windows) {
        "cmd /C copy /Y source.wasm guest.wasm"
    } else {
        "cp source.wasm guest.wasm"
    };
    fs::write(
        dev.join("golem.yaml"),
        format!("manifestVersion: {version}\napp: producer\npackage: {{name: editable, version: 1.0.0}}\ncomponents:\n  editable:guest:\n    componentWasm: guest.wasm\n    build:\n      - command: {copy}\n        sources: [source.wasm]\n        targets: [guest.wasm]\n"),
    ).unwrap();
    for source in ["(component)", "(component (core module))"] {
        let wasm = wat::parse_str(source).unwrap();
        fs::write(dev.join("source.wasm"), &wasm).unwrap();
        assert_eq!(cli(root, &["build"]).await["built"], true);
        assert_eq!(fs::read(dev.join("guest.wasm")).unwrap(), wasm);
        assert!(!root.join("golem.lock").exists());
    }
    fs::write(
        root.join("golem.lock"),
        format!("packages:\n  editable:\n    source: old.tar\n    version: 1.0.0\n    digest: sha256:{}\n", "a".repeat(64)),
    ).unwrap();
    assert_cli_error(root, &["build"], "remove packages.editable from golem.lock").await;

    fs::remove_file(root.join("golem.lock")).unwrap();
    write_install_fixture(
        &root.join("other.tar"),
        "other",
        wat::parse_str("(component)").unwrap(),
    );
    let result = run_cli(root, &["install", "./other.tar"]).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let manifest = dev.join("golem.yaml");
    fs::write(
        &manifest,
        format!(
            "{}\ntools:\n  borrowed: {{package: other, tool: echo}}\n",
            fs::read_to_string(&manifest).unwrap()
        ),
    )
    .unwrap();
    assert_cli_error(
        root,
        &["build"],
        "Development package tool borrowed cannot select a file package",
    )
    .await;
}

#[test]
#[timeout("120s")]
async fn development_package_build_uses_its_rust_workspace_target() {
    let dir = tempfile::tempdir().unwrap();
    let consumer = dir.path().join("consumer");
    let producer = dir.path().join("producer");
    let guest = producer.join("guest");
    // This fixture needs the Rust template, not the guest SDK implementation.
    let sdk = dir.path().join("sdk");
    fs::create_dir_all(sdk.join("src")).unwrap();
    fs::write(
        sdk.join("Cargo.toml"),
        "[package]\nname = 'golem-rust'\nversion = '0.0.0'\nedition = '2021'\n",
    )
    .unwrap();
    fs::write(sdk.join("src/lib.rs"), "").unwrap();
    fs::create_dir(&consumer).unwrap();
    fs::create_dir_all(guest.join("src")).unwrap();
    fs::write(consumer.join("Cargo.toml"), "[workspace]\nresolver = '2'\n").unwrap();
    fs::write(
        producer.join("Cargo.toml"),
        "[workspace]\nresolver = '2'\nmembers = ['guest']\n",
    )
    .unwrap();
    let guest_manifest = "[package]\nname = 'editable_guest'\nversion = '0.1.0'\nedition = '2021'\n[lib]\ncrate-type = ['cdylib']\n[dependencies]\ngolem-rust = { path = '../../sdk' }\n";
    fs::write(guest.join("Cargo.toml"), guest_manifest).unwrap();
    for workspace in [&consumer, &producer] {
        fs::create_dir(workspace.join(".cargo")).unwrap();
        fs::write(
            workspace.join(".cargo/config.toml"),
            "[build]\ntarget-dir = 'target'\n",
        )
        .unwrap();
    }
    let version = versions::sdk::MANIFEST;
    fs::write(
        consumer.join("golem.yaml"),
        format!("manifestVersion: {version}\napp: consumer\nenvironments:\n  local: {{server: local}}\npackages:\n  editable: {{path: ../producer}}\n"),
    ).unwrap();
    fs::write(
        producer.join("golem.yaml"),
        format!("manifestVersion: {version}\napp: producer\npackage: {{name: editable, version: 1.0.0}}\ncomponents:\n  editable:guest:\n    dir: guest\n    templates: rust\n"),
    ).unwrap();
    let wasm = producer.join("target/wasm32-wasip2/debug/editable_guest.wasm");
    let mut previous = Vec::new();
    for value in [17, 42] {
        fs::write(
            guest.join("src/lib.rs"),
            format!("#[no_mangle]\npub extern \"C\" fn version() -> u32 {{ {value} }}\n"),
        )
        .unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
            .kill_on_drop(true)
            .current_dir(&consumer)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .env("GOLEM_RUST_PATH", &sdk)
            .arg("--config-dir")
            .arg(consumer.join("isolated-config"))
            .arg("build")
            .arg("--yes")
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = fs::read(&wasm).unwrap();
        assert_ne!(bytes, previous);
        previous = bytes;
        assert_eq!(
            fs::read_to_string(guest.join("Cargo.toml")).unwrap(),
            guest_manifest
        );
    }
}

// Assemble a package without a publisher-side build, which cannot extract these components.
fn write_install_fixture(path: &Path, name: &str, wasm: Vec<u8>) -> String {
    use sha2::{Digest, Sha256};
    let mut blobs = std::collections::BTreeMap::new();
    let mut add = |media_type: &str, bytes: Vec<u8>| {
        let hex = format!("{:x}", Sha256::digest(&bytes));
        let descriptor = serde_json::json!({"mediaType": media_type, "digest": format!("sha256:{hex}"), "size": bytes.len()});
        blobs.insert(format!("blobs/sha256/{hex}"), bytes);
        descriptor
    };
    let wasm = add("application/wasm", wasm);
    let config = add("application/vnd.golem.package.config.v1+json", serde_json::to_vec(&serde_json::json!({
        "application": name, "package": {"name": name, "version": "1.2.3"},
        "components": {format!("pkg:{name}"): {
            "wasm": wasm["digest"], "metadata": {"agentTypes": [], "tools": [], "toolMiddlewares": []},
            "configSchema": {},
            "provision": {"config": null, "fileDefaults": [], "env": {}, "initialCard": null, "plugins": [], "files": null},
            "agents": {}, "tools": {}, "toolMiddlewares": {}
        }}
    })).unwrap());
    let media = "application/vnd.oci.image.manifest.v1+json";
    let manifest = add(media, serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2, "mediaType": media, "artifactType": "application/vnd.golem.package.v1",
        "config": config, "layers": [wasm]
    })).unwrap());
    let digest = manifest["digest"].as_str().unwrap().to_owned();
    blobs.insert(
        "oci-layout".into(),
        br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
    );
    blobs.insert("index.json".into(), serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json", "manifests": [manifest]
    })).unwrap());
    let mut archive = tar::Builder::new(fs::File::create(path).unwrap());
    for (path, content) in blobs {
        let mut header = tar::Header::new_ustar();
        header.set_mode(0o644);
        header.set_size(content.len() as u64);
        header.set_cksum();
        archive
            .append_data(&mut header, path, content.as_slice())
            .unwrap();
    }
    archive.finish().unwrap();
    digest
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
package: {{ name: packaged-tool, version: 1.2.3 }}
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
        let default = package.blob(&file.digest).unwrap();
        assert_eq!(default, b"from the declaring manifest\n");
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
