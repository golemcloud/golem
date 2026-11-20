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
#[timeout("90s")]
async fn install_file_package_builds_without_instantiating_and_keeps_lock_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let manifest = format!(
        "# keep me\nmanifestVersion: {}\napp: installer # keep me too\npackage: {{ name: installer, version: 1.2.3 }}\nenvironments:\n  local: {{ server: local }}\n",
        versions::sdk::MANIFEST
    );
    fs::write(root.join("golem.yaml"), &manifest).unwrap();
    fs::create_dir(root.join("packages")).unwrap();
    for (name, instruction) in [("traps", "unreachable"), ("loops", "(loop br 0)")] {
        let wasm = wat::parse_str(format!(
            "(component (core module $m (func $start {instruction}) (start $start)) (core instance (instantiate $m)))"
        )).unwrap();
        let tar = format!("packages/{name}.tar");
        let digest = write_install_fixture(&root.join(&tar), name, wasm);
        let result = run_cli(root, &["install", &format!("./{tar}")]).await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let installed: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(
            installed,
            serde_json::json!({"$type": "package.install", "name": name, "version": "1.2.3", "digest": digest})
        );
        let quiet = run_cli(root, &["--quiet", "install", &format!("./{tar}")]).await;
        assert!(quiet.status.success());
        assert!(
            quiet.stderr.is_empty(),
            "Quiet install diagnostics: {}",
            String::from_utf8_lossy(&quiet.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&quiet.stdout).unwrap(),
            installed
        );
        let edited = fs::read_to_string(root.join("golem.yaml")).unwrap();
        assert!(edited.contains("# keep me\n") && edited.contains("# keep me too\n"));
        assert!(edited.contains("packages:") && edited.contains(&digest));
        let pins: serde_yaml::Value = serde_yaml::from_str(&edited).unwrap();
        assert_eq!(pins["packages"][name]["source"].as_str().unwrap(), tar);
        if name == "traps" {
            let lock = fs::read(root.join("golem.lock")).unwrap();
            fs::write(root.join("golem.yaml"), &manifest).unwrap();
            assert_cli_error(
                root,
                &["--yes", "build", "--step", "check"],
                "no manifest entry",
            )
            .await;
            assert_eq!(
                fs::read_to_string(root.join("golem.yaml")).unwrap(),
                manifest
            );
            assert_eq!(fs::read(root.join("golem.lock")).unwrap(), lock);
            assert_eq!(cli(root, &["install", &tar]).await, installed);
        }
    }
    let lock = fs::read(root.join("golem.lock")).unwrap();
    let pinned_manifest = fs::read(root.join("golem.yaml")).unwrap();
    let unpack = root.join("golem-temp/package-unpack");
    assert!(
        !unpack.exists(),
        "Loading pins must not allocate package roots"
    );
    let mut package_operations = Vec::new();
    for args in [
        vec!["build", "--step", "check"],
        vec!["--yes", "build", "--step", "check"],
    ] {
        let checked = run_cli(root, &args).await;
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        assert!(!unpack.exists(), "Check must not materialize packages");
        assert_eq!(fs::read(root.join("golem.yaml")).unwrap(), pinned_manifest);
        assert_eq!(fs::read(root.join("golem.lock")).unwrap(), lock);
        let dump = format!(
            "{}{}",
            String::from_utf8_lossy(&checked.stderr),
            String::from_utf8_lossy(&checked.stdout)
        );
        let operation = dump
            .lines()
            .find(|line| line.contains("package inputs:"))
            .unwrap_or_else(|| {
                panic!("Separate package preparation inspection is missing: {dump}")
            });
        package_operations.push(operation.to_owned());
        assert!(dump.contains("no executable build plan was constructed"));
        assert!(!dump.contains("operation: PackageAssets"));
    }
    assert_eq!(
        package_operations[0], package_operations[1],
        "Unchanged package inputs must produce the same concrete inventory"
    );
    let dry_run = cli(root, &["publish", "--dry-run"]).await;
    assert!(!root.join("golem-package.tar").exists());
    assert_eq!(cli(root, &["package", "pack"]).await, dry_run);
    let repacked = package::read(Cursor::new(
        fs::read(root.join("golem-package.tar")).unwrap(),
    ))
    .unwrap();
    assert_eq!(repacked.config().components.len(), 2);
    assert_eq!(dry_run["digest"], repacked.digest());
    assert_eq!(fs::read(root.join("golem.yaml")).unwrap(), pinned_manifest);
    assert_eq!(fs::read(root.join("golem.lock")).unwrap(), lock);
    let prepared = fs::read_dir(&unpack)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b"root-")
        })
        .unwrap();
    let wasm = fs::read_dir(prepared.join("traps/wasm"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let original_wasm = fs::read(&wasm).unwrap();
    let modified = fs::metadata(&wasm).unwrap().modified().unwrap();
    for _ in 0..2 {
        let built = run_cli(root, &["build"]).await;
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&built.stdout).unwrap()["built"],
            true
        );
        assert!(String::from_utf8_lossy(&built.stderr).contains("Up-to-date"));
        assert_eq!(fs::metadata(&wasm).unwrap().modified().unwrap(), modified);
        assert_eq!(fs::read(&wasm).unwrap(), original_wasm);
    }
    assert_eq!(fs::read(root.join("golem.lock")).unwrap(), lock);
    assert!(
        prepared.exists(),
        "Prepared outputs persist beyond the final lease"
    );
    fs::write(&wasm, b"corrupt").unwrap();
    let check = run_cli(root, &["--yes", "build", "--step", "check"]).await;
    assert!(check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("no executable build plan was constructed")
    );
    assert_eq!(fs::read(&wasm).unwrap(), b"corrupt");
    assert_eq!(cli(root, &["build"]).await["built"], true);
    assert_eq!(fs::read(&wasm).unwrap(), original_wasm);
    for output in [
        wasm.clone(),
        prepared.join("lease"),
        prepared.join("package-manifest.json"),
    ] {
        fs::remove_file(&output).unwrap();
        let check = run_cli(root, &["--yes", "build", "--step", "check"]).await;
        assert!(check.status.success());
        assert!(
            String::from_utf8_lossy(&check.stderr)
                .contains("no executable build plan was constructed")
        );
        assert!(!output.exists(), "Check must not repair package outputs");
        assert_eq!(cli(root, &["build"]).await["built"], true);
        assert!(output.exists());
        assert_eq!(fs::read(&wasm).unwrap(), original_wasm);
    }
    let unexpected = prepared.join("unexpected.txt");
    fs::write(&unexpected, b"not part of the verified package").unwrap();
    let check = run_cli(root, &["--yes", "build", "--step", "check"]).await;
    assert!(check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("no executable build plan was constructed")
    );
    assert!(unexpected.exists(), "Check must not repair package outputs");
    assert_eq!(cli(root, &["build"]).await["built"], true);
    assert!(!unexpected.exists());

    let other = tempfile::tempdir().unwrap();
    let manifest_with_packages = fs::read_to_string(root.join("golem.yaml")).unwrap();
    fs::create_dir(other.path().join("packages")).unwrap();
    for file in [
        "golem.yaml",
        "golem.lock",
        "packages/loops.tar",
        "packages/traps.tar",
    ] {
        fs::copy(root.join(file), other.path().join(file)).unwrap();
    }
    assert_eq!(cli(other.path(), &["build"]).await["built"], true);
    assert_eq!(cli(other.path(), &["clean"]).await["cleaned"], true);
    fs::remove_dir_all(other.path().join("isolated-config/package-cache")).unwrap();
    write_install_fixture(
        &other.path().join("packages/traps.tar"),
        "traps",
        b"different bytes".to_vec(),
    );
    assert_cli_error(other.path(), &["build"], "digest").await;
    assert_eq!(cli(other.path(), &["clean"]).await["cleaned"], true);
    assert!(
        fs::read_dir(other.path().join("golem-temp/package-unpack"))
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == "registry.lock")
    );

    let mut changed: serde_yaml::Value = serde_yaml::from_str(&manifest_with_packages).unwrap();
    changed["packages"]["traps"]["version"] = serde_yaml::Value::String("2.0.0".into());
    fs::write(
        root.join("golem.yaml"),
        serde_yaml::to_string(&changed).unwrap(),
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
async fn overlapping_builds_retain_package_assets_and_refuse_clean_until_leases_exit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("golem.yaml"), format!(
        "manifestVersion: {}\napp: package-leases\nenvironments:\n  local: {{ server: local }}\ncomponents:\n  local:hold:\n    componentWasm: local.wasm\n    build:\n      - command: node hold.cjs\n", versions::sdk::MANIFEST)).unwrap();
    let wasm = wat::parse_str("(component)").unwrap();
    fs::write(root.join("local.wasm"), &wasm).unwrap();
    write_install_fixture(&root.join("installed.tar"), "installed", wasm);
    cli(root, &["install", "./installed.tar"]).await;
    fs::write(
        root.join("hold.cjs"),
        r#"
const fs = require('node:fs');
const path = require('node:path');
const slot = process.env.PACKAGE_TEST_SLOT;
const parent = 'golem-temp/package-unpack';
const roots = fs.readdirSync(parent).filter(name => name.startsWith('root-'));
if (roots.length !== 1) throw new Error('Expected exactly one shared package root');
const root = path.join(parent, roots[0]);
const wasmDir = path.join(root, 'installed/wasm');
const wasm = fs.readFileSync(path.join(wasmDir, fs.readdirSync(wasmDir)[0]));
if (wasm[0] !== 0 || wasm[1] !== 97) throw new Error('Missing package Wasm');
fs.writeFileSync(slot + '.ready', root);
const timer = setInterval(() => {
  if (fs.existsSync(slot + '.release')) {
    clearInterval(timer);
    clearTimeout(deadline);
  }
}, 10);
const deadline = setTimeout(() => { throw new Error('Timed out waiting for release'); }, 20000);
"#,
    )
    .unwrap();
    let mut children = Vec::new();
    for slot in ["first", "second"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
            .kill_on_drop(true)
            .current_dir(root)
            .env("PACKAGE_TEST_SLOT", slot)
            .arg("--config-dir")
            .arg(root.join("isolated-config"))
            .args(["--yes", "build", "--step", "build"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while !root.join(format!("{slot}.ready")).exists() {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("The build must read package assets before waiting");
        if !root.join(format!("{slot}.ready")).exists() {
            let output = child.wait_with_output().await.unwrap();
            panic!(
                "Build exited before reading package assets: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        children.push((slot, child));
    }
    let shared = fs::read_to_string(root.join("first.ready")).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("second.ready")).unwrap(),
        shared
    );
    assert_cli_error(root, &["clean"], "lease").await;
    assert!(root.join(&shared).exists());
    for (slot, child) in children {
        fs::write(root.join(format!("{slot}.release")), b"release").unwrap();
        let result = child.wait_with_output().await.unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(root.join(&shared).exists());
    }
    fs::remove_file(root.join("installed.tar")).unwrap();
    fs::remove_dir_all(root.join("isolated-config/package-cache")).unwrap();
    assert_eq!(cli(root, &["clean"]).await["cleaned"], true);
    assert!(!root.join(&shared).exists());
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
    componentPresets: debug
    server:
      url: http://{}
      auth:
        staticToken: "00000000-0000-0000-0000-000000000000"
      allowInsecure: true
components:
  app:agent:
    componentWasm: agent.wasm
    files:
    - sourcePath: ./component.txt
      targetPath: /component.txt
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
    for iteration in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("golem.yaml"), &manifest).unwrap();
        let settings = dir.path().join("settings");
        fs::create_dir(&settings).unwrap();
        let agent_manifest = format!(
            "manifestVersion: {}\nagents:\n  PackageFileAgent:\n    config:\n      file: ./sample.txt\n      text: ./sample.txt\n    files:\n    - sourcePath: ./agent.txt\n      targetPath: /agent.txt\n    presets:\n      app-env:local:\n        files:\n        - sourcePath: ./environment.txt\n          targetPath: /environment.txt\n      debug:\n        files:\n        - sourcePath: ./debug.txt\n          targetPath: /debug.txt\n",
            versions::sdk::MANIFEST
        );
        fs::write(settings.join("golem.yaml"), &agent_manifest).unwrap();
        fs::write(settings.join("sample.txt"), "from the declaring manifest\n").unwrap();
        fs::write(dir.path().join("sample.txt"), "from the app root\n").unwrap();
        fs::write(
            dir.path().join("component.txt"),
            "component initial bytes\n",
        )
        .unwrap();
        for (file, bytes) in [
            ("agent.txt", "agent initial bytes\n"),
            ("environment.txt", "environment initial bytes\n"),
            ("debug.txt", "preset initial bytes\n"),
        ] {
            fs::write(settings.join(file), bytes).unwrap();
            if iteration == 0 {
                fs::write(dir.path().join(file), "wrong component-directory bytes\n").unwrap();
            }
        }
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
        let files = defaults.files.as_ref().unwrap();
        let expected = std::collections::BTreeMap::from([
            ("/component.txt", "component initial bytes\n"),
            ("/agent.txt", "agent initial bytes\n"),
            ("/environment.txt", "environment initial bytes\n"),
            ("/debug.txt", "preset initial bytes\n"),
        ]);
        assert_eq!(files.entries.len(), expected.len());
        let mut archive = tar::Archive::new(package.blob(&files.layer).unwrap());
        let contents = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
                (path, bytes)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        for (path, file) in &files.entries {
            assert_eq!(
                contents[path],
                expected[file.target.as_abs_str()].as_bytes()
            );
        }
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
