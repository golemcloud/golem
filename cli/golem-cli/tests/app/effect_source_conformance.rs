use crate::app::{TestContext, cmd, flag};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};

#[test]
#[timeout("20 minutes")]
async fn effect_ambient_native_client_executes_through_golem() {
    let mut ctx = TestContext::new();
    ctx.enable_native_conformance_tool();
    ctx.start_server().await;

    let output = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            "effect-native-source",
            flag::TEMPLATE,
            "effect",
        ])
        .await;
    assert!(output.success_or_dump());
    ctx.cd("effect-native-source");
    let output = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "rust",
            flag::COMPONENT_NAME,
            "effect-native-source:middleware",
        ])
        .await;
    assert!(output.success_or_dump());
    fs::write_str(
        ctx.cwd_path_join("effect-main/src/counter-agent.ts"),
        fs::read_to_string(ctx.test_data_path_join("effect-source-conformance/native-consumer.ts"))
            .unwrap(),
    )
    .unwrap();
    configure_effect_bridge_path(&ctx, "native-conformance");
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {version}
            app: effect-native-source
            environments:
              local:
                server: local
                componentPresets: debug
                tools:
                  middleware: [native-conformance-audit]
            components:
              effect-native-source:consumer:
                dir: effect-main
                templates: effect
                dependencies:
                  tools: [native-conformance]
              effect-native-source:middleware:
                dir: middleware
                templates: rust
            tools:
              middleware:
                native-conformance-audit:
                  component: effect-native-source:middleware
            agents:
              EffectNativeConsumer:
                tools:
                  native-conformance: {{}}
              UnauthorizedEffectNativeConsumer:
                initialCard:
                  lowerBound:
                    positive:
                      - 'filesystem(?agent) @ ?agent : * : /**'
                      - 'network() @ ?agent : * : *'
                      - 'env(?agent) @ ?agent : * : *'
                      - 'oplog(?agent) @ ?agent : * : *'
                      - 'config(?agent) @ ?agent : * : *'
                      - 'secret(?env) @ ?agent : * : *'
                      - 'agent(?env/*/*) @ ?agent : * : *'
                      - 'environment(?env) @ ?agent : * : *'
                      - 'component(?component) @ ?agent : * : *'
                      - 'kv(?env) @ ?agent : * : *.**'
                      - 'blob(?env) @ ?agent : * : *.**'
                      - 'rdbms(?env) @ ?agent : * : *.*.*'
                      - 'card(?account) @ ?agent : * : *'
                    negative:
                      - 'tool(?env/*/*) @ ?agent : * : *'
                  upperBound: {{ positive: [], negative: [] }}
            bridge:
              effect:
                internal:
                  tools: [native-conformance]
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    write_native_middleware(&ctx);

    let built = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(built.success_or_dump());
    let generated = generated_source_text(&ctx.cwd_path_join(
        "golem-temp/bridge-sdk/effect/internal/native-conformance-tool-guest-client",
    ));
    for expected in [
        "native-conformance",
        "structured",
        "supported_error",
        "finite_stream",
        "middleware",
    ] {
        assert!(generated.contains(expected), "missing {expected}");
    }
    assert!(
        !ctx.cwd_path_join("provider").exists(),
        "ambient native client must not select a local provider component"
    );

    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "EffectNativeConsumer(\"effect\")",
            "run",
        ])
        .await;
    assert!(output.success_or_dump());
    for expected in [
        "alpha",
        "count\\\":7",
        "agentAuthorized\\\":true",
        "Rejected",
        "expected",
        "payload",
        "count\\\":2",
        "[102,105,114,115,116,58,112,97,121,108,111,97,100,124,115,101,99,111,110,100]",
        "leaf(middleware(input))",
    ] {
        assert!(output.stdout_contains(expected), "missing {expected}");
    }

    let denied = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "UnauthorizedEffectNativeConsumer(\"effect\")",
            "run",
        ])
        .await;
    assert!(denied.success_or_dump());
    assert!(denied.stdout_contains("denied"));
    assert!(denied.stdout_contains("permission target Tool"));
    assert!(denied.stdout_contains("is not allowed"));
    assert!(!denied.stdout_contains("agentAuthorized"));
}

#[test]
#[timeout("15 minutes")]
async fn effect_imported_mcp_client_projects_contract_and_runs_middleware() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let middleware_effects = Arc::new(Mutex::new(Vec::<String>::new()));
    let middleware_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let middleware_port = middleware_listener.local_addr().unwrap().port();
    let middleware_handler = axum::routing::post({
        let middleware_effects = middleware_effects.clone();
        move |uri: axum::http::Uri| {
            let middleware_effects = middleware_effects.clone();
            async move {
                middleware_effects
                    .lock()
                    .unwrap()
                    .push(uri.path().to_string());
                StatusCode::NO_CONTENT
            }
        }
    });
    let middleware_server = tokio::spawn(async move {
        axum::serve(
            middleware_listener,
            axum::Router::new().route("/{*path}", middleware_handler),
        )
        .await
        .unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = axum::routing::post({
        let calls = calls.clone();
        move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let calls = calls.clone();
            async move {
                if headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    != Some("Bearer effect-mcp-token")
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                let result = match body["method"].as_str() {
                    Some("tools/list") => json!({"tools":[{
                        "name":"lookup",
                        "title":"Catalog lookup",
                        "description":"Lookup one catalog entry.",
                        "inputSchema":{
                            "type":"object",
                            "properties":{
                                "item-id":{"type":"string","minLength":3},
                                "includeHistory":{"type":"boolean","default":false}
                            },
                            "required":["item-id"],
                            "additionalProperties":false
                        },
                        "outputSchema":{
                            "type":"object",
                            "properties":{
                                "id":{"type":"string"},
                                "revision":{"type":"integer","minimum":0}
                            },
                            "required":["id","revision"],
                            "additionalProperties":false
                        },
                        "annotations":{
                            "readOnlyHint":true,
                            "destructiveHint":false,
                            "idempotentHint":true,
                            "openWorldHint":false
                        }
                    }]}),
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
                        calls.lock().unwrap().push(arguments.clone());
                        match arguments["item-id"].as_str() {
                            Some("streamed") => json!({
                                "structuredContent":{"id":"streamed","revision":7},
                                "content":[{"type":"text","text":"single-content"}]
                            }),
                            Some("blocks") => json!({
                                "structuredContent":{"id":"blocks","revision":8},
                                "content":[
                                    {"type":"text","text":"left","annotations":{"audience":["user"],"priority":0.5},"_meta":{"source":"effect-test"}},
                                    {"type":"resource_link","uri":"https://example.test/catalog/blocks","name":"catalog","mimeType":"application/json"}
                                ]
                            }),
                            Some("tool-error") => json!({
                                "isError":true,
                                "content":[{"type":"text","text":"catalog denied"}]
                            }),
                            Some("invalid") => json!({
                                "structuredContent":{"id":"invalid","revision":-1},
                                "content":[]
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
    let upstream = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/mcp", handler))
            .await
            .unwrap();
    });

    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let output = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            "effect-mcp-source",
            flag::TEMPLATE,
            "effect",
        ])
        .await;
    assert!(output.success_or_dump());
    ctx.cd("effect-mcp-source");
    let output = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "rust",
            flag::COMPONENT_NAME,
            "effect-mcp-source:middleware",
        ])
        .await;
    assert!(output.success_or_dump());
    write_middleware(&ctx, middleware_port);
    fs::write_str(
        ctx.cwd_path_join("effect-main/src/counter-agent.ts"),
        fs::read_to_string(ctx.test_data_path_join("effect-source-conformance/mcp-consumer.ts"))
            .unwrap(),
    )
    .unwrap();
    let tsconfig_path = ctx.cwd_path_join("effect-main/tsconfig.json");
    let mut tsconfig: Value =
        serde_json::from_str(&fs::read_to_string(&tsconfig_path).unwrap()).unwrap();
    tsconfig["compilerOptions"]["baseUrl"] = json!(".");
    tsconfig["compilerOptions"]["paths"]["catalog-lookup-tool-guest-client"] = json!([
        "../golem-temp/bridge-sdk/effect/internal/catalog-lookup-tool-guest-client/catalog-lookup-tool-guest-client.ts"
    ]);
    tsconfig["include"].as_array_mut().unwrap().push(json!(
        "../golem-temp/bridge-sdk/effect/internal/catalog-lookup-tool-guest-client/*.ts"
    ));
    fs::write_str(
        tsconfig_path,
        serde_json::to_string_pretty(&tsconfig).unwrap(),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {version}
            app: effect-mcp-source
            environments:
              local:
                server: local
                componentPresets: debug
                tools:
                  middleware: [effect-mcp-audit]
            components:
              effect-mcp-source:consumer:
                dir: effect-main
                templates: effect
                dependencies:
                  tools: [catalog-lookup]
              effect-mcp-source:middleware:
                dir: middleware
                templates: rust
            mcp:
              imports:
                local:
                  - url: http://127.0.0.1:{port}/mcp
                    auth:
                      bearer: effect-mcp-token
                    prefix: catalog
            tools:
              middleware:
                effect-mcp-audit:
                  component: effect-mcp-source:middleware
            agents:
              EffectMcpConsumer:
                tools:
                  catalog-lookup: {{}}
            bridge:
              effect:
                internal:
                  tools: [catalog-lookup]
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    let built = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(built.success_or_dump());
    let generated =
        generated_source_text(&ctx.cwd_path_join(
            "golem-temp/bridge-sdk/effect/internal/catalog-lookup-tool-guest-client",
        ));
    assert!(generated.contains("catalog-lookup"));
    assert!(!generated.contains("effect-mcp-token"));
    assert!(!generated.contains(&format!("127.0.0.1:{port}")));

    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "EffectMcpConsumer(\"contract\")",
            "run",
        ])
        .await;
    assert!(output.success_or_dump());
    for expected in [
        "streamed",
        "revision\\\":7",
        "[115,105,110,103,108,101,45,99,111,110,116,101,110,116]",
        "blocks",
        "revision\\\":8",
        "left",
        "https://example.test/catalog/blocks",
        "effect-test",
        "tool:mcp-tool-error:catalog denied",
        "rpc:remote-tool-error",
        "invalid-result",
    ] {
        assert!(output.stdout_contains(expected), "missing {expected}");
    }
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            json!({"item-id":"streamed","includeHistory":false}),
            json!({"item-id":"blocks","includeHistory":true}),
            json!({"item-id":"tool-error","includeHistory":false}),
            json!({"item-id":"invalid","includeHistory":false}),
        ]
    );
    assert_eq!(
        *middleware_effects.lock().unwrap(),
        vec!["/middleware/catalog-lookup/golem-user"; 4]
    );
    upstream.abort();
    middleware_server.abort();
}

fn write_middleware(ctx: &TestContext, effect_port: u16) {
    fs::write_str(
        ctx.cwd_path_join("middleware/src/counter_agent.rs"),
        formatdoc! {r#"
            use futures_concurrency::prelude::*;
            use golem_rust::tool::{{InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool, ToolInvokeError, UnderlyingTool}};
            use golem_rust::{{TypedSchemaValue, universal_tool_middleware}};

            #[universal_tool_middleware(name = "effect-mcp-audit")]
            async fn audit(
                tool_name: String,
                _tool_metadata: Tool,
                command_path: Vec<String>,
                input: TypedSchemaValue,
                stdin: Option<InputStream>,
                stdout: Option<OutputStream>,
                stderr: Option<OutputStream>,
                principal: Principal,
                underlying: UnderlyingTool,
            ) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {{
                record_effect(&format!("/middleware/{{tool_name}}/{{}}", principal_class(&principal))).await;
                underlying.invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr).await
            }}

            fn principal_class(principal: &Principal) -> &'static str {{
                match principal {{
                    Principal::Anonymous => "anonymous",
                    Principal::Oidc(_) => "oidc",
                    Principal::Agent(_) => "agent",
                    Principal::GolemUser(_) => "golem-user",
                }}
            }}

            async fn record_effect(path: &str) {{
                use golem_rust::wasip3::http::{{client, types}};
                use golem_rust::wasip3::wit_future;
                let headers = types::Fields::from_list(&[]).unwrap();
                let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
                let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
                request.set_method(&types::Method::Post).unwrap();
                request.set_scheme(Some(&types::Scheme::Http)).unwrap();
                request.set_authority(Some("127.0.0.1:{effect_port}")).unwrap();
                request.set_path_with_query(Some(path)).unwrap();
                let send = async move {{ client::send(request).await.unwrap() }};
                let finish = async move {{
                    trailers_tx.write(Ok(None)).await.unwrap();
                    transmit.await.unwrap();
                }};
                let (response, ()) = (send, finish).join().await;
                assert_eq!(response.get_status_code(), 204);
            }}
        "#},
    )
    .unwrap();
    let manifest = ctx.cwd_path_join("middleware/Cargo.toml");
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write_str(
        manifest,
        source.replace(
            "[dependencies]",
            "[dependencies]\nfutures-concurrency = \"7.6.3\"",
        ),
    )
    .unwrap();
}

fn write_native_middleware(ctx: &TestContext) {
    fs::write_str(
        ctx.cwd_path_join("middleware/src/counter_agent.rs"),
        indoc! {r#"
            use golem_rust::schema::{SchemaValue, TypedSchemaValue};
            use golem_rust::tool::{
                InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool,
                ToolInvokeError, UnderlyingTool,
            };
            use golem_rust::universal_tool_middleware;

            #[universal_tool_middleware(name = "native-conformance-audit")]
            async fn audit(
                tool_name: String,
                _tool_metadata: Tool,
                command_path: Vec<String>,
                input: TypedSchemaValue,
                stdin: Option<InputStream>,
                stdout: Option<OutputStream>,
                stderr: Option<OutputStream>,
                _principal: Principal,
                underlying: UnderlyingTool,
            ) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
                let input = if tool_name == "native-conformance"
                    && command_path == ["middleware".to_string()]
                {
                    let (graph, mut value) = input.into_parts();
                    let SchemaValue::Record { fields } = &mut value else {
                        panic!("middleware input is a record")
                    };
                    let SchemaValue::String(argument) = &mut fields[0] else {
                        panic!("middleware argument is a string")
                    };
                    *argument = format!("middleware({argument})");
                    TypedSchemaValue::new(graph, value)
                } else {
                    input
                };
                underlying
                    .invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr)
                    .await
            }
        "#},
    )
    .unwrap();
}

fn configure_effect_bridge_path(ctx: &TestContext, tool_name: &str) {
    let package = format!("{tool_name}-tool-guest-client");
    let tsconfig_path = ctx.cwd_path_join("effect-main/tsconfig.json");
    let mut tsconfig: Value =
        serde_json::from_str(&fs::read_to_string(&tsconfig_path).unwrap()).unwrap();
    tsconfig["compilerOptions"]["baseUrl"] = json!(".");
    tsconfig["compilerOptions"]["paths"][&package] = json!([format!(
        "../golem-temp/bridge-sdk/effect/internal/{package}/{package}.ts"
    )]);
    tsconfig["include"]
        .as_array_mut()
        .unwrap()
        .push(json!(format!(
            "../golem-temp/bridge-sdk/effect/internal/{package}/*.ts"
        )));
    fs::write_str(
        tsconfig_path,
        serde_json::to_string_pretty(&tsconfig).unwrap(),
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
