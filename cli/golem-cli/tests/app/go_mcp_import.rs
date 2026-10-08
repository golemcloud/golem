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
async fn go_mcp_client_projects_contract_and_runs_imported_middleware() {
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
                    != Some("Bearer go-mcp-token")
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
    fs::create_dir_all(ctx.cwd_path_join("go-mcp-client")).unwrap();
    ctx.cd("go-mcp-client");
    for component in ["consumer", "middleware"] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "go",
                flag::COMPONENT_NAME,
                &format!("go-mcp-client:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {version}
            app: go-mcp-client
            environments:
              local:
                server: local
                componentPresets: debug
                tools:
                  middleware: []
            components:
              go-mcp-client:consumer:
                dir: consumer
                templates: go
                dependencies:
                  tools: [catalog-lookup]
              go-mcp-client:middleware:
                dir: middleware
                templates: go
            tools:
              middleware:
                imported-audit:
                  component: go-mcp-client:middleware
            mcp:
              imports:
                local:
                  - url: http://127.0.0.1:{port}/mcp
                    auth:
                      bearer: go-mcp-token
                    prefix: catalog
            bridge:
              go:
                internal:
                  tools: [catalog-lookup]
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    replace_go_component(
        &ctx.cwd_path_join("consumer"),
        "catalog",
        include_str!("go_mcp_import_consumer.go"),
    );
    let go_mod = ctx.cwd_path_join("consumer/go.mod");
    fs::write_str(
        &go_mod,
        fs::read_to_string(&go_mod).unwrap()
            + indoc! {r#"

                require golem.local/bridge/catalog-lookup-tool-guest-client v0.0.0

                replace golem.local/bridge/catalog-lookup-tool-guest-client => ../golem-temp/bridge-sdk/go/internal/catalog-lookup-tool-guest-client
            "#},
    )
    .unwrap();
    replace_go_component(
        &ctx.cwd_path_join("middleware"),
        "audit",
        indoc! {r#"
            // Package audit is an imported-side middleware that refuses every
            // call before it reaches the MCP transport.
            package audit

            import (
            	"github.com/golemcloud/golem/sdks/go/golem"
            	"github.com/golemcloud/golem/sdks/go/golem/tool"
            )

            var ImportedAudit = tool.DefineUniversalToolMiddleware[golem.Unit]("imported-audit", tool.MiddlewareSpec{})

            var _ = ImportedAudit.Handle(func(*tool.UniversalMiddlewareContext[golem.Unit]) (golem.Option[golem.TypedValue], error) {
            	return golem.None[golem.TypedValue](), tool.ConstraintViolation("go-imported-middleware")
            })
        "#},
    );

    let built = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(built.success_or_dump());
    let generated_root =
        ctx.cwd_path_join("golem-temp/bridge-sdk/go/internal/catalog-lookup-tool-guest-client");
    assert!(generated_root.join("go.mod").is_file());
    let generated = generated_source_text(&generated_root);
    assert!(generated.contains("tool.DefineToolClient[CatalogLookupTool](\"catalog-lookup\")"));
    assert!(!generated.contains("go-mcp-token"));
    assert!(!generated.contains(&format!("127.0.0.1:{port}")));
    assert!(*listings.lock().unwrap() > 0);
    assert!(calls.lock().unwrap().is_empty());

    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    assert!(!deployed.stderr_contains("Deployment discovery unavailable"));
    let agent = "McpGoConsumer(\"contract-2\")";
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
    assert!(middleware.stdout_contains("middleware:go-imported-middleware"));
    assert_eq!(
        calls.lock().unwrap().len(),
        3,
        "the imported-side middleware must short-circuit before MCP transport"
    );

    upstream.shutdown().await;
}

fn replace_go_component(component: &std::path::Path, package: &str, source: &str) {
    let module = fs::read_to_string(component.join("go.mod"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the component's go.mod names its module");
    fs::remove(component.join("agents")).unwrap();
    fs::write_str(component.join(format!("{package}/{package}.go")), source).unwrap();
    fs::write_str(
        component.join("main.go"),
        format!("package main\n\nimport _ \"{module}/{package}\"\n\nfunc main() {{}}\n"),
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
