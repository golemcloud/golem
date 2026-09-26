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

use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use std::path::PathBuf;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

/// The Go component directory `golem new` created: the one holding go.mod.
fn go_component_dir(ctx: &TestContext) -> PathBuf {
    std::fs::read_dir(ctx.cwd_path())
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| path.join("go.mod").exists())
        .unwrap_or_else(|| ctx.cwd_path().to_path_buf())
}

/// Deploys Go routers and an agent exposing its files, then drives them over
/// HTTP: a net/http mux with a streamed echo, a raw handler, static files,
/// config read from the request context, an OpenAPI fragment and live files.
#[test]
#[timeout("15 minutes")]
async fn test_go_http_router_deployed() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "http-routers", flag::TEMPLATE, "go"])
            .await
            .success_or_dump()
    );
    ctx.cd("http-routers");

    let component = go_component_dir(&ctx);
    let module = fs::read_to_string(component.join("go.mod"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the component's go.mod names its module");
    fs::write_str(
        component.join("routers/routers.go"),
        include_str!("go_http_router.go"),
    )
    .unwrap();
    fs::write_str(
        component.join("main.go"),
        format!("package main\n\nimport _ \"{module}/routers\"\n\nfunc main() {{}}\n"),
    )
    .unwrap();
    fs::remove(component.join("agents")).unwrap();
    fs::write_str(ctx.cwd_path_join("asset.txt"), "immutable asset").unwrap();

    let component_dir = component
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|_| component != ctx.cwd_path())
        .unwrap_or(".")
        .to_string();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: http-routers
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          http-routers:main:
            dir: {component_dir}
            templates: go
        agents:
          StaticRouter:
            files:
              - sourcePath: ./asset.txt
                targetPath: /assets/value.txt
                permissions: read-only
          ConfiguredRouter:
            config:
              greeting: "hello from config"
        httpApi:
          deployments:
            local:
              - domain: localhost:9006
                scheme: http
                openapiEndpoint: /
                agents:
                  WebRouter: {{}}
                  RawRouter: {{}}
                  StaticRouter: {{}}
                  ConfiguredRouter: {{}}
                  Files: {{}}
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let base = format!("http://localhost:{}", ctx.custom_request_port());
    let client = reqwest::Client::new();

    // Static files come from the component, without calling the router.
    let asset = client
        .get(format!("{base}/static/value.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);
    assert_eq!(asset.text().await.unwrap(), "immutable asset");

    // Live files come from the agent's filesystem as it changes.
    let initial = client
        .get(format!("{base}/files/alice/value"))
        .send()
        .await
        .unwrap();
    assert_eq!(initial.status(), 200);
    assert_eq!(initial.text().await.unwrap(), "initial:alice");
    assert!(
        ctx.cli([cmd::AGENT, cmd::INVOKE, "Files(\"alice\")", "update"])
            .await
            .success_or_dump()
    );
    assert_eq!(
        client
            .get(format!("{base}/files/alice/value"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "updated"
    );

    // Config is read from the request context.
    let configured = client
        .get(format!("{base}/configured/anything"))
        .send()
        .await
        .unwrap();
    assert_eq!(configured.status(), 200);
    assert_eq!(configured.text().await.unwrap(), "hello from config");

    // The provider's fragment is merged under the mount.
    let document: serde_json::Value = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        document["paths"]["/web/echo"]["post"].is_object(),
        "{document}"
    );

    // The raw handler sees the envelope: an extension method, the full path and
    // a present-but-empty query.
    let response = client
        .request(
            reqwest::Method::from_bytes(b"CuStOm").unwrap(),
            format!("{base}/raw/x?"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let head: serde_json::Value = response.json().await.unwrap();
    assert_eq!(head["method"], "CuStOm");
    assert_eq!(head["path"], "/raw/x");
    assert_eq!(head["query"], "");

    // net/http: a HEAD carries no body, and a handler may answer without
    // reading its input.
    let head = client
        .head(format!("{base}/web/head"))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert!(head.bytes().await.unwrap().is_empty());
    let early = client
        .post(format!("{base}/web/early"))
        .body("unread input")
        .send()
        .await
        .unwrap();
    assert_eq!(early.status(), 202);
    assert_eq!(early.text().await.unwrap(), "early");

    // The echo streams both ways: the second input chunk is withheld until the
    // first output chunk has arrived.
    let (send, receive) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    send.send(Ok(b"first".to_vec())).await.unwrap();
    let input = futures_util::stream::unfold(receive, |mut receive| async {
        receive.recv().await.map(|chunk| (chunk, receive))
    });
    let mut echo = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client
            .post(format!("{base}/web/echo"))
            .body(reqwest::Body::wrap_stream(input))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(echo.status(), 200);
    assert_eq!(
        echo.headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["a=first", "a=second"]
    );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(30), echo.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "first"
    );
    send.send(Ok(b"second".to_vec())).await.unwrap();
    drop(send);
    assert_eq!(echo.bytes().await.unwrap(), "second");
}
