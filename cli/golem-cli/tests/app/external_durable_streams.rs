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

use crate::app::{TestContext, cmd, flag};
use crate::workspace_path;
use golem_cli::model::invoke_result_view::InvokeResultView;
use golem_cli::{fs, versions};
use golem_common::schema::FromSchema;
use indoc::formatdoc;
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use test_r::{test, timeout};
use tokio::process::{Child, Command};

struct ReferenceServer {
    process: Child,
    url: String,
    client: reqwest::Client,
    _directory: TempDir,
}

impl ReferenceServer {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // This is the server release at durable-streams/durable-streams protocol
        // baseline 461b40267aabd644558f9b19dbb9507dd5f691cf. npm ci pins transitives.
        for (name, contents) in [
            (
                "package.json",
                include_str!("durable_streams_reference/package.json"),
            ),
            (
                "package-lock.json",
                include_str!("durable_streams_reference/package-lock.json"),
            ),
            (
                "server.mjs",
                include_str!("durable_streams_reference/server.mjs"),
            ),
        ] {
            fs::write_str(directory.path().join(name), contents).unwrap();
        }
        let install = tokio::time::timeout(
            Duration::from_secs(120),
            Command::new("npm")
                .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
                .current_dir(directory.path())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("reference server npm ci timed out")
        .expect("failed to run npm ci");
        assert!(
            install.status.success(),
            "reference server npm ci failed: {}",
            String::from_utf8_lossy(&install.stderr)
        );
        let ready = directory.path().join("ready.json");
        let mut process = Command::new("node")
            .arg("server.mjs")
            .arg(&ready)
            .current_dir(directory.path())
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to start reference server");
        let url = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    process.try_wait().unwrap().is_none(),
                    "reference server exited before readiness"
                );
                if let Ok(contents) = fs::read_to_string(&ready)
                    && let Ok(ready) = serde_json::from_str::<Value>(&contents)
                {
                    break ready["url"].as_str().unwrap().to_owned();
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("reference server startup timed out");
        Self {
            process,
            url,
            _directory: directory,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    async fn create(&self, path: &str, content_type: &str) -> String {
        let url = format!("{}{path}", self.url);
        let response = self
            .client
            .put(&url)
            .header("content-type", content_type)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        url
    }

    async fn requests(&self) -> Vec<Value> {
        self.client
            .get(format!("{}/__golem_test/requests", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn wait_for_live(&self, path: &str, transport: &str) {
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if self.requests().await.iter().any(|request| {
                    request["method"] == "GET" && request["path"] == path
                        && request["live"] == transport
                        // Exercise an actual empty/open long-poll response before data arrives.
                        && (transport != "long-poll" || request["status"] == 204)
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("guest never reached the selected live transport");
    }

    async fn stop(mut self) {
        drop(self.process.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(10), self.process.wait())
            .await
            .expect("reference server shutdown timed out")
            .unwrap();
        assert!(
            status.success(),
            "reference server shutdown failed: {status}"
        );
    }
}

async fn invoke<T: FromSchema>(
    ctx: &TestContext,
    agent: &str,
    method: &str,
    values: Vec<Value>,
) -> T {
    let mut args = vec![
        flag::YES.to_owned(),
        cmd::AGENT.to_owned(),
        cmd::INVOKE.to_owned(),
        format!("ExternalDurableStreams(\"{agent}\")"),
        method.to_owned(),
    ];
    args.extend(values.iter().map(Value::to_string));
    args.extend([
        "--no-stream".to_owned(),
        flag::FORMAT.to_owned(),
        "json".to_owned(),
    ]);
    let output = tokio::time::timeout(Duration::from_secs(60), ctx.cli(args))
        .await
        .expect("external durable stream invocation timed out");
    assert!(output.success_or_dump());
    let results: Vec<InvokeResultView> = output.stdout_json();
    assert_eq!(results.len(), 1, "expected one invocation result");
    let value = results[0]
        .result_json
        .as_ref()
        .expect("missing typed invocation result");
    T::from_value(value.value()).expect("unexpected guest result schema")
}

#[test]
#[timeout("10 minutes")]
async fn external_durable_streams_reference_server_e2e() {
    let fixture =
        workspace_path().join("test-components/golem_it_external_durable_streams_release.wasm");
    assert!(
        fixture.is_file(),
        "build test-components/external-durable-streams before running this test"
    );
    let mut ctx = TestContext::new();
    fs::copy(&fixture, ctx.cwd_path_join("external-durable-streams.wasm")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: external-durable-streams-reference
        environments:
          local:
            server: local
        components:
          golem-it:external-durable-streams:
            componentWasm: external-durable-streams.wasm
            outputWasm: external-durable-streams-final.wasm
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    ctx.start_server().await;
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let server = ReferenceServer::start().await;

    // The reference server JSON.parse/JSON.stringify path rounds integers above
    // Number.MAX_SAFE_INTEGER. Host codec tests cover unsafe numeric lexemes;
    // third-party roundtrips here assert exact safe-large values and nested arrays.
    let nested = "[9007199254740991,{\"left\":[1,2],\"right\":17}]";
    let final_json = "{\"tail\":9007199254740989}";
    let json_url = server.create("/json", "application/json").await;
    let append = vec![
        json!(json_url),
        json!("json-duplicate"),
        json!([nested]),
        json!(false),
    ];
    let first: Result<Vec<Option<String>>, String> =
        invoke(&ctx, "writer-json", "append_json", append.clone()).await;
    let first = first.unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].as_ref().is_some_and(|offset| !offset.is_empty()));
    let duplicate: Result<Vec<Option<String>>, String> =
        invoke(&ctx, "writer-json-retry", "append_json", append).await;
    assert_eq!(duplicate.unwrap(), [None]);
    let closed: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-json-final",
        "append_json",
        vec![
            json!(json_url),
            json!("json-final"),
            json!([final_json]),
            json!(true),
        ],
    )
    .await;
    assert_eq!(closed.unwrap().len(), 1);
    let read: Result<Vec<String>, String> = invoke(
        &ctx,
        "reader-json",
        "consume_json",
        vec![json!(json_url), json!("-1"), json!("catch-up"), json!(100)],
    )
    .await;
    let read = read
        .unwrap()
        .into_iter()
        .map(|raw| serde_json::from_str::<Value>(&raw).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        read,
        vec![
            json!([9007199254740991u64, {"left": [1, 2], "right": 17}]),
            json!({"tail": 9007199254740989u64})
        ]
    );
    let rejected: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-json-closed",
        "append_json",
        vec![
            json!(json_url),
            json!("after-close"),
            json!(["99"]),
            json!(false),
        ],
    )
    .await;
    assert!(rejected.is_err(), "a closed stream accepted new data");

    let bytes_url = server.create("/bytes", "application/octet-stream").await;
    let appended: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-bytes",
        "append_bytes",
        vec![
            json!(bytes_url),
            json!("bytes-final"),
            json!([[0, 255, 19], [4, 8]]),
            json!(true),
        ],
    )
    .await;
    assert_eq!(appended.unwrap().len(), 2);
    let read: Result<Vec<u8>, String> = invoke(
        &ctx,
        "reader-bytes",
        "consume_bytes",
        vec![json!(bytes_url), json!("-1"), json!("catch-up"), json!(100)],
    )
    .await;
    assert_eq!(read.unwrap(), [0, 255, 19, 4, 8]);

    let empty_url = server.create("/close-only", "application/json").await;
    let closed: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-close-only",
        "append_json",
        vec![
            json!(empty_url),
            json!("close-only"),
            json!([]),
            json!(true),
        ],
    )
    .await;
    let closed = closed.unwrap();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].as_ref().is_some_and(|offset| !offset.is_empty()));
    let read: Result<Vec<String>, String> = invoke(
        &ctx,
        "reader-close-only",
        "consume_json",
        vec![json!(empty_url), json!("-1"), json!("catch-up"), json!(100)],
    )
    .await;
    assert!(read.unwrap().is_empty());

    for transport in ["long-poll", "sse"] {
        for bytes in [false, true] {
            let mode = if bytes { "bytes" } else { "json" };
            let name = format!("{mode}-{transport}");
            let path = format!("/{name}");
            let url = server
                .create(
                    &path,
                    if bytes {
                        "application/octet-stream"
                    } else {
                        "application/json"
                    },
                )
                .await;
            let producer = format!("producer-{name}");
            let write = async {
                server.wait_for_live(&path, transport).await;
                let values = if bytes {
                    json!([[0, 255, 19], [4, 8]])
                } else {
                    json!([nested, final_json])
                };
                let result: Result<Vec<Option<String>>, String> = invoke(
                    &ctx,
                    &format!("writer-{name}"),
                    if bytes { "append_bytes" } else { "append_json" },
                    vec![json!(url), json!(producer), values, json!(true)],
                )
                .await;
                assert_eq!(result.unwrap().len(), 2);
            };
            let read = async {
                let args = vec![json!(url), json!("-1"), json!(transport), json!(100)];
                if bytes {
                    let result: Result<Vec<u8>, String> =
                        invoke(&ctx, &format!("reader-{name}"), "consume_bytes", args).await;
                    assert_eq!(result.unwrap(), [0, 255, 19, 4, 8]);
                } else {
                    let result: Result<Vec<String>, String> =
                        invoke(&ctx, &format!("reader-{name}"), "consume_json", args).await;
                    assert_eq!(result.unwrap(), [nested, final_json]);
                }
            };
            tokio::join!(read, write);
        }
    }

    let requests = server.requests().await;
    let duplicate_posts = requests
        .iter()
        .filter(|r| r["producerId"] == "json-duplicate")
        .collect::<Vec<_>>();
    assert_eq!(duplicate_posts.len(), 2);
    assert_eq!(duplicate_posts[0]["status"], 200);
    assert_eq!(duplicate_posts[1]["status"], 204);
    for request in duplicate_posts {
        assert_eq!(request["producerEpoch"], "0");
        assert_eq!(request["producerSeq"], "0");
    }
    let bytes_close = requests
        .iter()
        .find(|r| r["producerId"] == "bytes-final" && r["producerSeq"] == "1")
        .unwrap();
    assert_eq!(bytes_close["closed"], "true");
    let close_only = requests
        .iter()
        .find(|r| r["producerId"] == "close-only")
        .unwrap();
    assert_eq!(close_only["producerSeq"], "0");
    assert_eq!(close_only["closed"], "true");
    assert_eq!(close_only["status"], 204);
    server.stop().await;
}
