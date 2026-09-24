use crate::app::{TestContext, cmd, flag};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};

#[test]
#[timeout("15 minutes")]
async fn rust_mcp_clients_use_registry_credentials_and_replay_offline() {
    let calls = Arc::new(Mutex::new(Vec::<(String, Value, String)>::new()));
    let listings = Arc::new(Mutex::new(BTreeSet::<String>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = axum::routing::post({
        let calls = calls.clone();
        let listings = listings.clone();
        move |uri: Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let calls = calls.clone();
            let listings = listings.clone();
            async move {
                let expected_auth = match uri.path() {
                    "/bearer" => "Bearer test-mcp-token",
                    "/basic" => "Basic dXNlcjpwYXNz",
                    _ => return StatusCode::NOT_FOUND.into_response(),
                };
                if headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    != Some(expected_auth)
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                let result = match body["method"].as_str() {
                    Some("tools/list") => {
                        listings.lock().unwrap().insert(uri.path().to_string());
                        json!({"tools":[{
                            "name":"lookup",
                            "inputSchema":{
                                "type":"object", "properties":{"query":{"type":"string"},"limit":{"type":"integer"}},
                                "required":["query"], "additionalProperties":{"type":"string"}
                            },
                            "outputSchema":{
                                "type":"object", "properties":{"answer":{"type":"string"},"score":{"type":"integer"}},
                                "required":["answer","score"], "additionalProperties":false
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
                        let arguments = body["params"]["arguments"].clone();
                        let Some(query) = arguments["query"].as_str().map(str::to_owned) else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
                        let Some(key) = headers
                            .get("idempotency-key")
                            .and_then(|value| value.to_str().ok())
                            .filter(|value| !value.is_empty())
                        else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
                        calls
                            .lock()
                            .unwrap()
                            .push((uri.path().into(), arguments, key.into()));
                        let content = match query.as_str() {
                            "empty" => json!([]),
                            "binary" => {
                                json!([{"type":"image","data":"AAEC/w==","mimeType":"image/test"}])
                            }
                            "resource" => {
                                json!([{"type":"resource","resource":{"uri":"file:///exact.bin","blob":"ECAw","mimeType":"application/x-exact"}}])
                            }
                            "mixed" => {
                                json!([{"type":"text","text":"left"},{"type":"text","text":"right"}])
                            }
                            _ => json!([{"type":"text","text":format!("stdout:{query}")}]),
                        };
                        let score = if uri.path() == "/bearer" { 7 } else { 13 };
                        json!({"structuredContent":{"answer":format!("answer:{}:{query}", uri.path()),"score":score},"content":content})
                    }
                    _ => return StatusCode::BAD_REQUEST.into_response(),
                };
                axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result})).into_response()
            }
        }
    });
    let mut upstream = tokio::task::JoinSet::new();
    upstream.spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/bearer", handler.clone())
                .route("/basic", handler),
        )
        .await
        .unwrap();
    });
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let output = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            "mcp-client",
            flag::TEMPLATE,
            "rust",
            flag::COMPONENT_NAME,
            "mcp-client:consumer",
        ])
        .await;
    assert!(output.success_or_dump());
    ctx.cd("mcp-client");
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: mcp-client
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          mcp-client:consumer:
            dir: .
            templates: rust
            dependencies:
              tools: [bearer-lookup, basic-lookup]
        mcp:
          imports:
            local:
              - url: http://127.0.0.1:{port}/bearer
                auth:
                  bearer: test-mcp-token
                prefix: bearer
              - url: http://127.0.0.1:{port}/basic
                auth:
                  basic: {{user: user, password: pass}}
                prefix: basic
        bridge:
          rust:
            internal:
              tools: [bearer-lookup, basic-lookup]
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("src/counter_agent.rs"), indoc! {r#"
        use bearer_lookup_tool_guest_client::{BearerLookupClient, Content as BearerContent, Blocks as BearerBlocks, Body as BearerBody};
        use basic_lookup_tool_guest_client::{BasicLookupClient, Content as BasicContent, Blocks as BasicBlocks, Body as BasicBody};
        use golem_rust::{agent_definition, agent_implementation};
        use golem_rust::agentic::UnstructuredBinary;

        #[agent_definition]
        pub trait McpConsumer {
            fn new(name: String) -> Self;
            async fn run(&mut self) -> Vec<String>;
            fn status(&self) -> Vec<String>;
        }
        struct Consumer { results: Vec<String> }
        #[agent_implementation]
        impl McpConsumer for Consumer {
            fn new(_name: String) -> Self { Self { results: Vec::new() } }
            async fn run(&mut self) -> Vec<String> {
                for query in ["simple", "mixed", "empty", "binary", "resource"] {
                    let (limit, extras) = if query == "simple" { (None, vec![]) } else { (Some(3), vec![("region".into(), "west".into())]) };
                    let (result, stdout) = BearerLookupClient::new().bearer_lookup(limit, query.into(), extras).await.unwrap().collect().await.unwrap();
                    if query == "resource" {
                        let BearerContent::Blocks(blocks) = &result.content else { panic!("expected resource blocks") };
                        let [BearerBlocks::EmbeddedResource(resource)] = blocks.as_slice() else { panic!("expected one embedded resource") };
                        assert_eq!(resource.uri, "file:///exact.bin");
                        assert_eq!(resource.mime_type.as_deref(), Some("application/x-exact"));
                        let BearerBody::Blob(UnstructuredBinary::Inline { data, mime_type }) = &resource.body else { panic!("expected inline blob") };
                        assert_eq!(data, &[16, 32, 48]);
                        assert_eq!(mime_type, "application/x-exact");
                        assert!(stdout.is_empty());
                    }
                    let content = match result.content {
                        BearerContent::Blocks(blocks) => blocks.into_iter().map(|block| match block {
                            BearerBlocks::Text(text) => text.text,
                            other => format!("{other:?}"),
                        }).collect::<Vec<_>>().join(","),
                        BearerContent::Streamed(descriptor) => descriptor.mime_type,
                        BearerContent::None => "none".into(),
                    };
                    if query == "mixed" { assert_eq!(content, "left,right"); }
                    if query == "empty" { assert_eq!((&content, stdout.as_slice()), (&"none".to_string(), [].as_slice())); }
                    if query == "binary" { assert_eq!((&content, stdout.as_slice()), (&"image/test".to_string(), [0, 1, 2, 255].as_slice())); }
                    self.results.push(format!("bearer:{}:{}:{stdout:?}:{content}", result.structured.answer, result.structured.score));
                    let (limit, extras) = if query == "simple" { (None, vec![]) } else { (Some(3), vec![("region".into(), "west".into())]) };
                    let (result, stdout) = BasicLookupClient::new().basic_lookup(limit, query.into(), extras).await.unwrap().collect().await.unwrap();
                    if query == "resource" {
                        let BasicContent::Blocks(blocks) = &result.content else { panic!("expected resource blocks") };
                        let [BasicBlocks::EmbeddedResource(resource)] = blocks.as_slice() else { panic!("expected one embedded resource") };
                        assert_eq!(resource.uri, "file:///exact.bin");
                        assert_eq!(resource.mime_type.as_deref(), Some("application/x-exact"));
                        let BasicBody::Blob(UnstructuredBinary::Inline { data, mime_type }) = &resource.body else { panic!("expected inline blob") };
                        assert_eq!(data, &[16, 32, 48]);
                        assert_eq!(mime_type, "application/x-exact");
                        assert!(stdout.is_empty());
                    }
                    let content = match result.content {
                        BasicContent::Blocks(blocks) => blocks.into_iter().map(|block| match block {
                            BasicBlocks::Text(text) => text.text,
                            other => format!("{other:?}"),
                        }).collect::<Vec<_>>().join(","),
                        BasicContent::Streamed(descriptor) => descriptor.mime_type,
                        BasicContent::None => "none".into(),
                    };
                    if query == "mixed" { assert_eq!(content, "left,right"); }
                    if query == "empty" { assert_eq!((&content, stdout.as_slice()), (&"none".to_string(), [].as_slice())); }
                    if query == "binary" { assert_eq!((&content, stdout.as_slice()), (&"image/test".to_string(), [0, 1, 2, 255].as_slice())); }
                    self.results.push(format!("basic:{}:{}:{stdout:?}:{content}", result.structured.answer, result.structured.score));
                }
                self.results.clone()
            }
            fn status(&self) -> Vec<String> { self.results.clone() }
        }
    "#}).unwrap();
    let manifest = ctx.cwd_path_join("Cargo.toml");
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write_str(&manifest, source.replace("[dependencies]", indoc! {r#"
        [dependencies]
        bearer-lookup-tool-guest-client = { path = "golem-temp/bridge-sdk/rust/internal/bearer-lookup-tool-guest-client" }
        basic-lookup-tool-guest-client = { path = "golem-temp/bridge-sdk/rust/internal/basic-lookup-tool-guest-client" }
    "#}.trim_end())).unwrap();

    let built = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(built.success_or_dump());
    for name in ["bearer-lookup", "basic-lookup"] {
        let generated = generated_source_text(&ctx.cwd_path_join(format!(
            "golem-temp/bridge-sdk/rust/internal/{name}-tool-guest-client"
        )));
        assert!(
            generated.contains(name),
            "generated client must call the canonical Golem tool name '{name}'"
        );
        assert!(!generated.contains("test-mcp-token"));
        assert!(!generated.contains(&format!("127.0.0.1:{port}")));
    }
    assert_eq!(
        *listings.lock().unwrap(),
        BTreeSet::from(["/bearer".into(), "/basic".into()])
    );
    assert!(
        calls.lock().unwrap().is_empty(),
        "codegen only discovers tools"
    );
    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    assert!(!deployed.stderr_contains("Deployment discovery unavailable"));
    let agent = "McpConsumer(\"replay\")";
    let output = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "run"])
        .await;
    assert!(output.success_or_dump());
    for (prefix, score) in [("bearer", 7), ("basic", 13)] {
        assert!(output.stdout_contains(format!(
            "{prefix}:answer:/{prefix}:simple:{score}:[115, 116, 100, 111, 117, 116, 58, 115, 105, 109, 112, 108, 101]:text/plain; charset=utf-8"
        )));
        assert!(output.stdout_contains(format!(
            "{prefix}:answer:/{prefix}:mixed:{score}:[]:left,right"
        )));
        assert!(output.stdout_contains(format!("{prefix}:answer:/{prefix}:empty:{score}:[]:none")));
        assert!(output.stdout_contains(format!(
            "{prefix}:answer:/{prefix}:binary:{score}:[0, 1, 2, 255]:image/test"
        )));
        assert!(output.stdout_contains(format!("{prefix}:answer:/{prefix}:resource:{score}:[]:")));
    }
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 10);
        assert_eq!(
            calls
                .iter()
                .map(|(_, _, key)| key)
                .collect::<BTreeSet<_>>()
                .len(),
            10
        );
        assert_eq!(
            calls
                .iter()
                .map(|(path, arguments, _)| (path.as_str(), arguments.to_string()))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                ("/bearer", json!({"query":"simple"}).to_string()),
                (
                    "/bearer",
                    json!({"query":"mixed","limit":3,"region":"west"}).to_string()
                ),
                (
                    "/bearer",
                    json!({"query":"empty","limit":3,"region":"west"}).to_string()
                ),
                (
                    "/bearer",
                    json!({"query":"binary","limit":3,"region":"west"}).to_string()
                ),
                (
                    "/bearer",
                    json!({"query":"resource","limit":3,"region":"west"}).to_string()
                ),
                ("/basic", json!({"query":"simple"}).to_string()),
                (
                    "/basic",
                    json!({"query":"mixed","limit":3,"region":"west"}).to_string()
                ),
                (
                    "/basic",
                    json!({"query":"empty","limit":3,"region":"west"}).to_string()
                ),
                (
                    "/basic",
                    json!({"query":"binary","limit":3,"region":"west"}).to_string()
                ),
                (
                    "/basic",
                    json!({"query":"resource","limit":3,"region":"west"}).to_string()
                )
            ])
        );
    }

    upstream.shutdown().await;
    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;
    let replayed = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "status"])
        .await;
    assert!(replayed.success_or_dump());
    for (prefix, score) in [("bearer", 7), ("basic", 13)] {
        assert!(replayed.stdout_contains(format!(
            "{prefix}:answer:/{prefix}:simple:{score}:[115, 116, 100, 111, 117, 116, 58, 115, 105, 109, 112, 108, 101]:text/plain; charset=utf-8"
        )));
        assert!(replayed.stdout_contains(format!(
            "{prefix}:answer:/{prefix}:mixed:{score}:[]:left,right"
        )));
        assert!(
            replayed.stdout_contains(format!("{prefix}:answer:/{prefix}:empty:{score}:[]:none"))
        );
        assert!(replayed.stdout_contains(format!(
            "{prefix}:answer:/{prefix}:binary:{score}:[0, 1, 2, 255]:image/test"
        )));
        assert!(
            replayed.stdout_contains(format!("{prefix}:answer:/{prefix}:resource:{score}:[]:"))
        );
    }
    assert_eq!(calls.lock().unwrap().len(), 10);
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
