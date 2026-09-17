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
    let calls = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
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
                                "type":"object", "properties":{"query":{"type":"string"}},
                                "required":["query"], "additionalProperties":false
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
                        let Some(query) = body["params"]["arguments"]["query"].as_str() else {
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
                            .push((uri.path().into(), query.into(), key.into()));
                        let content = if query == "mixed" {
                            json!([{"type":"text","text":"left"},{"type":"text","text":"right"}])
                        } else {
                            json!([{"type":"text","text":format!("stdout:{query}")}])
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
        use bearer_lookup_tool_guest_client::{BearerLookupClient, Content as BearerContent, Blocks as BearerBlocks};
        use basic_lookup_tool_guest_client::{BasicLookupClient, Content as BasicContent, Blocks as BasicBlocks};
        use golem_rust::{agent_definition, agent_implementation};

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
                for query in ["simple", "mixed"] {
                    let (result, stdout) = BearerLookupClient::new().bearer_lookup(query.into()).unwrap().collect().await.unwrap();
                    let content = match result.content {
                        BearerContent::Blocks(blocks) => blocks.into_iter().map(|block| match block {
                            BearerBlocks::Text(text) => text.text,
                            _ => panic!("expected text block"),
                        }).collect::<Vec<_>>().join(","),
                        BearerContent::Streamed(descriptor) => descriptor.mime_type,
                        BearerContent::None => panic!("expected content"),
                    };
                    if query == "mixed" { assert_eq!(content, "left,right"); }
                    self.results.push(format!("bearer:{}:{}:{}:{content}", result.structured.answer, result.structured.score, String::from_utf8(stdout).unwrap()));
                    let (result, stdout) = BasicLookupClient::new().basic_lookup(query.into()).unwrap().collect().await.unwrap();
                    let content = match result.content {
                        BasicContent::Blocks(blocks) => blocks.into_iter().map(|block| match block {
                            BasicBlocks::Text(text) => text.text,
                            _ => panic!("expected text block"),
                        }).collect::<Vec<_>>().join(","),
                        BasicContent::Streamed(descriptor) => descriptor.mime_type,
                        BasicContent::None => panic!("expected content"),
                    };
                    if query == "mixed" { assert_eq!(content, "left,right"); }
                    self.results.push(format!("basic:{}:{}:{}:{content}", result.structured.answer, result.structured.score, String::from_utf8(stdout).unwrap()));
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
        assert!(output.stdout_contains(&format!(
            "{prefix}:answer:/{prefix}:simple:{score}:stdout:simple:text/plain; charset=utf-8"
        )));
        assert!(output.stdout_contains(&format!(
            "{prefix}:answer:/{prefix}:mixed:{score}::left,right"
        )));
    }
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(
            calls
                .iter()
                .map(|(_, _, key)| key)
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        assert_eq!(
            calls
                .iter()
                .map(|(path, query, _)| (path.as_str(), query.as_str()))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                ("/bearer", "simple"),
                ("/bearer", "mixed"),
                ("/basic", "simple"),
                ("/basic", "mixed")
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
        assert!(replayed.stdout_contains(&format!(
            "{prefix}:answer:/{prefix}:simple:{score}:stdout:simple:text/plain; charset=utf-8"
        )));
        assert!(replayed.stdout_contains(&format!(
            "{prefix}:answer:/{prefix}:mixed:{score}::left,right"
        )));
    }
    assert_eq!(calls.lock().unwrap().len(), 4);
}
