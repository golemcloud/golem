use crate::app::{TestContext, cmd, flag};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};

#[test]
#[timeout("15 minutes")]
async fn moonbit_mcp_client_projects_contract_and_runs_imported_middleware() {
    let calls = Arc::new(Mutex::new(Vec::<(Value, String)>::new()));
    let listings = Arc::new(Mutex::new(0usize));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = axum::routing::post({
        let calls = calls.clone();
        let listings = listings.clone();
        move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let calls = calls.clone();
            let listings = listings.clone();
            async move {
                if headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    != Some("Bearer moonbit-mcp-token")
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                let result = match body["method"].as_str() {
                    Some("tools/list") => {
                        *listings.lock().unwrap() += 1;
                        json!({"tools":[{
                            "name":"lookup",
                            "title":"Contract lookup",
                            "description":"Exercises the imported MCP projection.",
                            "inputSchema":{
                                "type":"object",
                                "properties":{"query":{"type":"string"}},
                                "required":["query"],
                                "additionalProperties":false
                            },
                            "outputSchema":{
                                "type":"object",
                                "properties":{"answer":{"type":"string"},"score":{"type":"integer"}},
                                "required":["answer","score"],
                                "additionalProperties":false
                            }
                        }]})
                    }
                    Some("tools/call") => {
                        if body["params"]["name"] != "lookup"
                            || headers
                                .get("mcp-name")
                                .and_then(|value| value.to_str().ok())
                                != Some("lookup")
                        {
                            return StatusCode::BAD_REQUEST.into_response();
                        }
                        let Some(idempotency_key) = headers
                            .get("idempotency-key")
                            .and_then(|value| value.to_str().ok())
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned)
                        else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
                        let arguments = body["params"]["arguments"].clone();
                        let Some(query) = arguments["query"].as_str() else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
                        calls
                            .lock()
                            .unwrap()
                            .push((arguments.clone(), idempotency_key));
                        match query {
                            "streamed" => json!({
                                "structuredContent":{"answer":"stream-answer","score":7},
                                "content":[{"type":"text","text":"finite-stream"}]
                            }),
                            "blocks" => json!({
                                "structuredContent":{"answer":"blocks-answer","score":11},
                                "content":[
                                    {"type":"text","text":"left"},
                                    {"type":"text","text":"right"}
                                ]
                            }),
                            "error" => json!({
                                "isError":true,
                                "content":[{"type":"text","text":"supported-mcp-error"}]
                            }),
                            _ => return StatusCode::BAD_REQUEST.into_response(),
                        }
                    }
                    _ => return StatusCode::BAD_REQUEST.into_response(),
                };
                axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result})).into_response()
            }
        }
    });
    let mut upstream = tokio::task::JoinSet::new();
    upstream.spawn(async move {
        axum::serve(listener, axum::Router::new().route("/mcp", handler))
            .await
            .unwrap();
    });

    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("moonbit-mcp-client")).unwrap();
    ctx.cd("moonbit-mcp-client");
    for component in ["consumer", "middleware"] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "moonbit",
                flag::COMPONENT_NAME,
                &format!("moonbit-mcp-client:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {version}
            app: moonbit-mcp-client
            environments:
              local:
                server: local
                componentPresets: debug
                tools:
                  middleware: []
            components:
              moonbit-mcp-client:consumer:
                dir: consumer
                templates: moonbit
                dependencies:
                  tools: [catalog-lookup]
              moonbit-mcp-client:middleware:
                dir: middleware
                templates: moonbit
            tools:
              middleware:
                imported-audit:
                  component: moonbit-mcp-client:middleware
            mcp:
              imports:
                local:
                  - url: http://127.0.0.1:{port}/mcp
                    auth:
                      bearer: moonbit-mcp-token
                    prefix: catalog
            bridge:
              moonbit:
                internal:
                  tools: [catalog-lookup]
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    let moon_mod_path = ctx.cwd_path_join("moon.mod.json");
    let mut moon_mod: Value =
        serde_json::from_str(&fs::read_to_string(&moon_mod_path).unwrap()).unwrap();
    moon_mod["deps"]["catalog-lookup-tool-guest-client"] = json!({
        "path": "golem-temp/bridge-sdk/moonbit/internal/catalog-lookup-tool-guest-client"
    });
    fs::write_str(
        &moon_mod_path,
        serde_json::to_string_pretty(&moon_mod).unwrap() + "\n",
    )
    .unwrap();

    add_imports(
        &ctx.cwd_path_join("consumer/moon.pkg"),
        indoc! {r#"
              "catalog-lookup-tool-guest-client/client" @catalog,
              "golemcloud/golem_sdk/tool" @tool,
        "#},
    );
    fs::write_str(
        ctx.cwd_path_join("consumer/counter.mbt"),
        indoc! {r#"
            ///|
            #derive.agent
            struct McpMoonbitConsumer {
              name : String
            }

            ///|
            fn McpMoonbitConsumer::new(name : String) -> McpMoonbitConsumer {
              { name, }
            }

            ///|
            pub async fn McpMoonbitConsumer::run(
              self : Self,
              query : String,
            ) -> String {
              ignore(self.name)
              let client = @catalog.CatalogLookupClient::new()
              defer client.drop()
              let invocation = match client.catalog_lookup(query) {
                Ok(invocation) => invocation
                Err(@tool.ToolError::RemoteTool(
                  @tool.RemoteToolError::ConstraintViolation(message),
                )) => return "middleware:" + message
                Err(_) => return "start-error"
              }
              match invocation.collect() {
                Err(@tool.ToolError::Tool(
                  @catalog.CatalogLookupError::McpToolError(message),
                )) => "error:" + message
                Err(@tool.ToolError::RemoteTool(
                  @tool.RemoteToolError::ConstraintViolation(message),
                )) => "middleware:" + message
                Err(_) => "collect-error"
                Ok(collected) => {
                  let result = collected.result
                  match query {
                    "streamed" => {
                      guard result.structured.answer == "stream-answer" &&
                        result.structured.score == 7L else {
                        return "bad-structured"
                      }
                      guard result.content is @catalog.Content::Streamed(
                        { mime_type: "text/plain; charset=utf-8", .. },
                      ) else {
                        return "bad-stream-metadata"
                      }
                      guard collected.stdout is Some(b"finite-stream") else {
                        return "bad-stream-bytes"
                      }
                      "streamed:stream-answer:7:finite-stream"
                    }
                    "blocks" => {
                      guard result.structured.answer == "blocks-answer" &&
                        result.structured.score == 11L else {
                        return "bad-structured"
                      }
                      guard result.content is @catalog.Content::Blocks([
                        @catalog.Blocks::Text({ text: "left", .. }),
                        @catalog.Blocks::Text({ text: "right", .. }),
                      ]) else {
                        return "bad-blocks"
                      }
                      guard collected.stdout is Some(b"") else {
                        return "unexpected-stdout"
                      }
                      "blocks:left:right:blocks-answer:11"
                    }
                    _ => "unexpected-success"
                  }
                }
              }
            }

            ///|
            fn main {

            }
        "#},
    )
    .unwrap();

    add_imports(
        &ctx.cwd_path_join("middleware/moon.pkg"),
        "  \"golemcloud/golem_sdk/tool-middleware\" @toolMiddleware,\n",
    );
    fs::write_str(
        ctx.cwd_path_join("middleware/counter.mbt"),
        indoc! {r#"
            ///|
            #derive.universal_tool_middleware("imported-audit")
            pub async fn imported_audit(
              invocation : @toolMiddleware.RawToolInvocation,
              underlying : @toolMiddleware.UnderlyingTool,
            ) -> Result[
              @toolMiddleware.RawInvocationResult,
              @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],
            ] {
              ignore(invocation.tool_name())
              ignore(underlying)
              Err(
                @toolMiddleware.ToolInvokeError::ConstraintViolation(
                  "moonbit-imported-middleware",
                ),
              )
            }

            ///|
            fn main {

            }
        "#},
    )
    .unwrap();

    let built = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(built.success_or_dump());
    let generated_root = ctx
        .cwd_path_join("golem-temp/bridge-sdk/moonbit/internal/catalog-lookup-tool-guest-client");
    assert!(generated_root.join("moon.mod.json").is_file());
    let generated = generated_source_text(&generated_root);
    assert!(generated.contains("@tool.ToolClient::new(\"catalog-lookup\")"));
    assert!(!generated.contains("moonbit-mcp-token"));
    assert!(!generated.contains(&format!("127.0.0.1:{port}")));
    assert!(*listings.lock().unwrap() > 0);
    assert!(calls.lock().unwrap().is_empty());

    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    assert!(!deployed.stderr_contains("Deployment discovery unavailable"));
    let agent = "McpMoonbitConsumer(\"contract-2\")";
    for (query, expected) in [
        ("streamed", "streamed:stream-answer:7:finite-stream"),
        ("blocks", "blocks:left:right:blocks-answer:11"),
        ("error", "error:supported-mcp-error"),
    ] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                agent,
                "run",
                &serde_json::to_string(query).unwrap(),
            ])
            .await;
        assert!(output.success_or_dump());
        assert!(output.stdout_contains(expected));
    }
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls
                .iter()
                .map(|(arguments, _)| arguments.clone())
                .collect::<Vec<_>>(),
            ["streamed", "blocks", "error"]
                .map(|query| json!({"query":query}))
                .to_vec()
        );
        assert_eq!(
            calls
                .iter()
                .map(|(_, key)| key)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
    }

    let manifest_path = ctx.cwd_path_join("golem.yaml");
    let mut manifest: Value =
        serde_yaml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["environments"]["local"]["tools"]["middleware"] = json!(["imported-audit"]);
    fs::write_str(&manifest_path, serde_yaml::to_string(&manifest).unwrap()).unwrap();
    let redeployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(redeployed.success_or_dump());
    let middleware = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            agent,
            "run",
            "\"streamed\"",
        ])
        .await;
    assert!(middleware.success_or_dump());
    assert!(middleware.stdout_contains("middleware:moonbit-imported-middleware"));
    assert_eq!(
        calls.lock().unwrap().len(),
        3,
        "the imported-side middleware must short-circuit before MCP transport"
    );

    upstream.shutdown().await;
}

fn add_imports(path: &std::path::Path, imports: &str) {
    let package = fs::read_to_string(path).unwrap();
    fs::write_str(
        path,
        package.replace("import {", &format!("import {{\n{imports}")),
    )
    .unwrap();
}

fn generated_source_text(root: &std::path::Path) -> String {
    let mut result = String::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.push_str(&generated_source_text(&path));
        } else if let Ok(source) = std::fs::read_to_string(path) {
            result.push_str(&source);
        }
    }
    result
}
