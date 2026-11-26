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
use testcontainers::{
    ContainerAsync, GenericImage,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::process::Command;

async fn run_cli(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_golem-cli"))
        .kill_on_drop(true)
        .current_dir(dir)
        .env("DOCKER_CONFIG", dir.join("isolated-docker-config"))
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("NO_PROXY", "")
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
async fn install_oci_refuses_authentication_over_loopback_http() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::write(
        root.join("golem.yaml"),
        format!(
            "manifestVersion: {}\napp: installer\n",
            versions::sdk::MANIFEST
        ),
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let host = listener.local_addr()?;
    let docker_config = root.join("isolated-docker-config");
    fs::create_dir(&docker_config)?;
    fs::write(
        docker_config.join("config.json"),
        format!(r#"{{"auths":{{"{host}":{{"auth":"dXNlcjpwYXNz"}}}}}}"#),
    )?;
    let (requests, mut seen) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let app = axum::Router::new().fallback(move |headers: axum::http::HeaderMap| async move {
            let _ = requests.send(headers.contains_key(axum::http::header::AUTHORIZATION));
            (
                axum::http::StatusCode::UNAUTHORIZED,
                [(
                    axum::http::header::WWW_AUTHENTICATE,
                    "Basic realm=\"registry\"",
                )],
            )
        });
        axum::serve(listener, app).await.unwrap();
    });
    let source = format!("oci://{host}/packages/remote:1.0");
    assert_cli_error(root, &["install", &source], "must allow anonymous pulls").await;
    server.abort();
    assert_eq!(seen.try_recv(), Ok(false));
    assert!(seen.try_recv().is_err(), "unexpected authentication retry");
    assert!(!root.join("golem.lock").exists());
    Ok(())
}

#[test]
#[timeout("45s")]
async fn install_url_pins_digest_and_cold_build_rejects_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(
        root.join("golem.yaml"),
        format!(
            "manifestVersion: {}\napp: installer\nenvironments:\n  local: {{ server: local }}\n",
            versions::sdk::MANIFEST
        ),
    )
    .unwrap();
    let archive = root.join("package.tar");
    let digest = write_install_fixture(&archive, "remote", wat::parse_str("(component)").unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/package.tar", listener.local_addr().unwrap());
    assert_cli_error(
        root,
        &["install", &format!("{url}?token=not-a-real-secret")],
        "no credentials, query string, or fragment",
    )
    .await;
    assert!(!root.join("golem.lock").exists());
    let server = tokio::spawn({
        let serve = axum::Router::new().route(
            "/package.tar",
            axum::routing::get(move || {
                let archive = archive.clone();
                async move { fs::read(archive).unwrap() }
            }),
        );
        async move { axum::serve(listener, serve).await.unwrap() }
    });
    let result = run_cli(root, &["install", &url]).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let lock = fs::read(root.join("golem.lock")).unwrap();
    let pins: serde_yaml::Value = serde_yaml::from_slice(&lock).unwrap();
    assert_eq!(
        pins["packages"]["remote"]["source"].as_str(),
        Some(url.as_str())
    );
    assert_eq!(
        pins["packages"]["remote"]["digest"].as_str(),
        Some(digest.as_str())
    );
    assert_eq!(cli(root, &["build"]).await["built"], true);

    let replacement = write_install_fixture(
        &root.join("package.tar"),
        "remote",
        wat::parse_str("(component (core module))").unwrap(),
    );
    assert_ne!(digest, replacement);
    assert_eq!(cli(root, &["build"]).await["built"], true);
    let cache = root.join("isolated-config/package-cache");
    fs::remove_dir_all(&cache).unwrap();
    assert_cli_error(root, &["build"], "Package digest mismatch").await;
    assert_eq!(fs::read(root.join("golem.lock")).unwrap(), lock);
    assert!(!cache.exists());
    server.abort();
}

#[test]
#[timeout("2m")]
async fn install_oci_cold_build_fetches_the_pinned_manifest_after_tag_moves() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::write(
        root.join("golem.yaml"),
        format!(
            "manifestVersion: {}\napp: installer\nenvironments:\n  local: {{ server: local }}\n",
            versions::sdk::MANIFEST
        ),
    )?;
    let (registry, host) = start_registry().await?;
    let reference = format!("{host}/packages/remote:1.0");
    let source = format!("oci://{reference}");
    let archive = root.join("package.tar");
    let digest = write_install_fixture(&archive, "remote", wat::parse_str("(component)")?);
    push_package(&reference, &fs::read(&archive)?).await?;
    let result = run_cli(root, &["install", &source]).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let lock_path = root.join("golem.lock");
    let lock = fs::read(&lock_path)?;
    let pins: serde_yaml::Value = serde_yaml::from_slice(&lock)?;
    assert_eq!(
        pins["packages"]["remote"]["source"].as_str(),
        Some(source.as_str())
    );
    assert_eq!(
        pins["packages"]["remote"]["digest"].as_str(),
        Some(digest.as_str())
    );
    assert_eq!(cli(root, &["build"]).await["built"], true);

    let replacement = write_install_fixture(
        &archive,
        "remote",
        wat::parse_str("(component (core module))")?,
    );
    assert_ne!(replacement, digest);
    push_package(&reference, &fs::read(&archive)?).await?;
    let tag = reqwest::Client::new()
        .get(format!("http://{host}/v2/packages/remote/manifests/1.0"))
        .header(reqwest::header::ACCEPT, package::MANIFEST)
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(tag.headers()["docker-content-digest"], replacement.as_str());
    fs::remove_file(archive)?;
    let cache = root.join("isolated-config/package-cache");
    fs::remove_dir_all(&cache)?;
    let before = registry.stderr_to_vec().await?;
    assert_eq!(cli(root, &["build"]).await["built"], true);
    let after = registry.stderr_to_vec().await?;
    let requests = String::from_utf8_lossy(after.strip_prefix(before.as_slice()).unwrap());
    assert!(
        requests.contains(&format!("/manifests/{digest}")),
        "{requests}"
    );
    assert!(!requests.contains("/manifests/1.0"), "{requests}");
    let downloaded = package::read(fs::File::open(
        cache.join(digest.strip_prefix("sha256:").unwrap()),
    )?)?;
    assert_eq!(downloaded.digest(), digest);
    assert_eq!(fs::read(&lock_path)?, lock);

    registry.stop().await?;
    fs::remove_dir_all(cache)?;
    let failed = run_cli(root, &["build"]).await;
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&failed.stderr),
        String::from_utf8_lossy(&failed.stdout)
    );
    assert!(
        !failed.status.success()
            && output.contains(&source)
            && output.contains(&format!("Docker credential key {host}")),
        "{output}"
    );
    assert_eq!(fs::read(lock_path)?, lock);
    Ok(())
}

pub(crate) async fn start_registry() -> anyhow::Result<(ContainerAsync<GenericImage>, String)> {
    let registry = GenericImage::new(
        "registry",
        "3.1.2@sha256:82f6ff4a7f982960c8aa51617fea3aabc485df6e55cce80d22deb1de36f1bc3c",
    )
    .with_exposed_port(5000.tcp())
    .with_wait_for(WaitFor::message_on_stderr("listening on"))
    .start()
    .await?;
    let host = format!("127.0.0.1:{}", registry.get_host_port_ipv4(5000).await?);
    Ok((registry, host))
}

pub(crate) async fn push_package(reference: &str, archive: &[u8]) -> anyhow::Result<()> {
    use oci_client::{
        Client, Reference,
        client::{ClientConfig, ClientProtocol},
        secrets::RegistryAuth,
    };
    use std::io::Read;

    let reference: Reference = reference.parse()?;
    let client = Client::try_from(ClientConfig {
        protocol: ClientProtocol::Http,
        ..Default::default()
    })?;
    client
        .store_auth_if_needed(reference.resolve_registry(), &RegistryAuth::Anonymous)
        .await;
    let digest = package::read(archive)?.digest().to_owned();
    let mut manifest = None;
    for entry in tar::Archive::new(archive).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(hex) = path
            .to_str()
            .and_then(|path| path.strip_prefix("blobs/sha256/"))
        else {
            continue;
        };
        let blob_digest = format!("sha256:{hex}");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if blob_digest == digest {
            manifest = Some(bytes);
        } else {
            client.push_blob(&reference, bytes, &blob_digest).await?;
        }
    }
    client
        .push_manifest_raw(&reference, manifest.unwrap(), package::MANIFEST.parse()?)
        .await?;
    Ok(())
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
