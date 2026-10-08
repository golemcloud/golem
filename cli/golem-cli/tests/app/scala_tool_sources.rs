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

use super::{TestContext, cmd, flag};
use crate::workspace_path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};

const FIXTURE: &str = "scala-tool-sources";
const NATIVE_FIXTURE: &str = "scala-native-tool-source";

#[derive(Clone, Debug)]
struct McpCall {
    item_id: String,
    authorized: bool,
}

#[test]
#[timeout("20 minutes")]
async fn scala_generated_client_invokes_ambient_native_tool_through_golem() {
    let mut ctx = TestContext::new();
    fs_extra::dir::copy(
        ctx.test_data_path_join(NATIVE_FIXTURE),
        ctx.cwd_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();
    ctx.cd(NATIVE_FIXTURE);
    ctx.enable_native_conformance_tool();
    ctx.start_server().await;

    let built = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(built.success_or_dump());
    let generated_path = ctx.cwd_path_join(
        "golem-temp/bridge-sdk/scala/internal/native-conformance-tool-guest-client/src/main/scala/golem/bridge/client/native_conformance/NativeConformanceClient.scala",
    );
    let generated = std::fs::read_to_string(&generated_path).unwrap();
    assert!(generated.contains("final class NativeConformanceClient"));
    assert!(generated.contains("def structured("));
    assert!(generated.contains("def supportedError("));
    assert!(generated.contains("def finiteStream("));
    assert!(generated.contains("def middleware("));
    assert!(generated.contains("agentAuthorized: _root_.scala.Boolean"));
    assert!(
        !ctx.cwd_path_join("provider").exists(),
        "ambient native client must not select a local provider component"
    );

    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    let invoke = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "ScalaNativeToolSourceAgent(\"scala\")",
            "exercise",
        ])
        .await;
    assert!(invoke.success_or_dump());
    for expected in [
        "success:alpha:7:true",
        "error:Tool(Rejected(expected))",
        "stream:first:payload|second:2",
        "middleware:leaf(middleware(input))",
    ] {
        assert!(
            invoke.stdout_contains(expected),
            "missing independent native conformance evidence: {expected}"
        );
    }

    let denied = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "UnauthorizedScalaNativeToolSourceAgent(\"scala\")",
            "exercise",
        ])
        .await;
    assert!(denied.success_or_dump());
    assert!(denied.stdout_contains("permission target Tool"));
    assert!(denied.stdout_contains("is not allowed"));
    assert!(!denied.stdout_contains("agentAuthorized"));
}

#[test]
#[timeout("20 minutes")]
async fn scala_generated_client_invokes_imported_mcp_projection_through_golem() {
    let contract: Value = serde_json::from_str(
        &std::fs::read_to_string(workspace_path().join("test-data/gol-40/mcp-projection-v1.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(contract["contract"], "GOL-40-CONTRACT-2");
    assert_eq!(
        contract["mcpToNativeImport"]["source"]["upstreamName"],
        "catalog.lookup"
    );
    assert_eq!(
        contract["mcpToNativeImport"]["source"]["projectedName"],
        "catalog-lookup"
    );
    assert_eq!(
        contract["mcpToNativeImport"]["nativeMetadata"]["result"]["fields"][1]["type"],
        "none | streamed | blocks"
    );

    let calls = Arc::new(Mutex::new(Vec::<McpCall>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler_calls = calls.clone();
    let handler = move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
        let calls = handler_calls.clone();
        async move {
            let authorized = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                == Some("Bearer scala-mcp-token");
            if !authorized {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let result = match body["method"].as_str() {
                Some("tools/list") => json!({
                    "tools": [{
                        "name": "catalog.lookup",
                        "title": "Catalog lookup",
                        "description": "Lookup one catalog entry.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "item-id": {"type": "string", "minLength": 3},
                                "includeHistory": {"type": "boolean", "default": false}
                            },
                            "required": ["item-id"],
                            "additionalProperties": false
                        },
                        "outputSchema": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string"},
                                "revision": {"type": "integer", "minimum": 0}
                            },
                            "required": ["id", "revision"],
                            "additionalProperties": false
                        },
                        "annotations": {
                            "readOnlyHint": true,
                            "destructiveHint": false,
                            "idempotentHint": true,
                            "openWorldHint": false
                        }
                    }]
                }),
                Some("tools/call") => {
                    if body["params"]["name"] != "catalog.lookup" {
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                    let Some(item_id) = body["params"]["arguments"]["item-id"].as_str() else {
                        return StatusCode::BAD_REQUEST.into_response();
                    };
                    calls.lock().unwrap().push(McpCall {
                        item_id: item_id.to_string(),
                        authorized,
                    });
                    match item_id {
                        "none" => json!({
                            "structuredContent": {"id": "none-id", "revision": 1},
                            "content": []
                        }),
                        "text" => json!({
                            "structuredContent": {"id": "text-id", "revision": 2},
                            "content": [{
                                "type": "text",
                                "text": "finite-text",
                                "annotations": {"audience": ["assistant"], "priority": 0.75},
                                "golem-extra": "text-extension"
                            }]
                        }),
                        "binary" => json!({
                            "structuredContent": {"id": "binary-id", "revision": 3},
                            "content": [{
                                "type": "image",
                                "data": "AAEC/w==",
                                "mimeType": "image/test",
                                "annotations": {"audience": ["user"]},
                                "golem-extra": "binary-extension"
                            }]
                        }),
                        "blocks" => json!({
                            "structuredContent": {"id": "blocks-id", "revision": 4},
                            "content": [
                                {"type": "text", "text": "left"},
                                {"type": "resource_link", "uri": "file:///catalog/entry", "name": "entry"}
                            ]
                        }),
                        "failure" => json!({
                            "isError": true,
                            "content": [{"type": "text", "text": "{\"code\":\"missing\",\"item\":\"failure\"}"}]
                        }),
                        "middleware-applied" => json!({
                            "structuredContent": {"id": "middleware-id", "revision": 5},
                            "content": [{"type": "text", "text": "middleware-observed"}]
                        }),
                        _ => return StatusCode::BAD_REQUEST.into_response(),
                    }
                }
                _ => return StatusCode::BAD_REQUEST.into_response(),
            };
            axum::Json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}))
                .into_response()
        }
    };
    let mut upstream = tokio::task::JoinSet::new();
    upstream.spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route("/mcp", axum::routing::post(handler)),
        )
        .await
        .unwrap();
    });

    let mut ctx = TestContext::new();
    fs_extra::dir::copy(
        ctx.test_data_path_join(FIXTURE),
        ctx.cwd_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();
    ctx.cd(FIXTURE);
    let manifest_path = ctx.cwd_path_join("golem.yaml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("__MCP_URL__", &format!("http://127.0.0.1:{port}/mcp"));
    std::fs::write(&manifest_path, manifest).unwrap();

    ctx.start_server().await;
    let built = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(built.success_or_dump());
    let generated = std::fs::read_to_string(ctx.cwd_path_join(
        "golem-temp/bridge-sdk/scala/internal/catalog-lookup-tool-guest-client/src/main/scala/golem/bridge/client/catalog_lookup/CatalogLookupClient.scala",
    ))
    .unwrap();
    assert!(generated.contains("final class CatalogLookupClient"));
    assert!(generated.contains("def catalogLookup("));
    assert!(generated.contains("itemId: _root_.scala.Predef.String"));
    assert!(generated.contains("includehistory: _root_.scala.Option[_root_.scala.Boolean]"));
    assert!(generated.contains("McpToolError"));
    assert!(!generated.contains("scala-mcp-token"));
    assert!(!generated.contains(&format!("127.0.0.1:{port}")));

    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    for (item_id, expected) in [
        ("none", &["none-id", "None", "stdout=;stderr=none"][..]),
        (
            "text",
            &[
                "text-id",
                "text/plain",
                "assistant",
                "text-extension",
                "stdout=66696e6974652d74657874;stderr=none",
            ][..],
        ),
        (
            "binary",
            &[
                "binary-id",
                "image/test",
                "user",
                "binary-extension",
                "stdout=000102ff;stderr=none",
            ][..],
        ),
        (
            "blocks",
            &["blocks-id", "left", "file:///catalog/entry"][..],
        ),
        ("failure", &["McpToolError", "missing", "failure"][..]),
        (
            "middleware",
            &[
                "middleware-id",
                "stdout=6d6964646c65776172652d6f62736572766564;stderr=none",
            ][..],
        ),
    ] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                "ScalaToolSourceAgent(\"mcp\")",
                "invoke",
                &format!("\"{item_id}\""),
            ])
            .await;
        assert!(output.success_or_dump());
        for fragment in expected {
            assert!(output.stdout_contains(fragment), "missing {fragment:?}");
        }
    }

    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 6);
        assert!(calls.iter().all(|call| call.authorized));
        assert_eq!(
            calls
                .iter()
                .map(|call| call.item_id.as_str())
                .collect::<Vec<_>>(),
            [
                "none",
                "text",
                "binary",
                "blocks",
                "failure",
                "middleware-applied"
            ]
        );
    }
    upstream.shutdown().await;
}
