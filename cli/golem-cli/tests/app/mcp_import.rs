use crate::app::{TestContext, cmd, flag};
use crate::workspace_path;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};

#[test]
#[timeout("20 minutes")]
async fn exported_native_tool_roundtrips_through_import_middleware_and_replays_offline() {
    let contract: Value = serde_json::from_str(
        &std::fs::read_to_string(workspace_path().join("test-data/gol-40/mcp-projection-v1.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(contract["contract"], "GOL-40-CONTRACT-2");
    let mut contract_failures = Vec::new();

    let effects = Arc::new(Mutex::new(Vec::<String>::new()));
    let effect_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let effect_port = effect_listener.local_addr().unwrap().port();
    let effect_handler = axum::routing::post({
        let effects = effects.clone();
        move |uri: Uri| {
            let effects = effects.clone();
            async move {
                effects.lock().unwrap().push(uri.path().to_string());
                StatusCode::NO_CONTENT
            }
        }
    });
    let mut effect_server = tokio::task::JoinSet::new();
    effect_server.spawn(async move {
        axum::serve(
            effect_listener,
            axum::Router::new().fallback(effect_handler),
        )
        .await
        .unwrap();
    });
    let projected_tools = Arc::new(Mutex::new(None::<Value>));
    let mcp_fixture_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_fixture_port = mcp_fixture_listener.local_addr().unwrap().port();
    let mcp_fixture_handler = axum::routing::post({
        let projected_tools = projected_tools.clone();
        let effects = effects.clone();
        move |axum::Json(body): axum::Json<Value>| {
            let projected_tools = projected_tools.clone();
            let effects = effects.clone();
            async move {
                let result = match body["method"].as_str() {
                    Some("tools/list") => projected_tools.lock().unwrap().clone().unwrap(),
                    Some("tools/call") => {
                        let item_id = body["params"]["arguments"]["item-id"]
                            .as_str()
                            .unwrap()
                            .to_string();
                        effects
                            .lock()
                            .unwrap()
                            .push(format!("/provider/{item_id}/anonymous"));
                        if item_id == "reject" {
                            json!({
                                "isError":true,
                                "content":[{"type":"text","text":"{\"name\":\"rejected\",\"payload\":\"reject\"}"}]
                            })
                        } else {
                            json!({
                                "structuredContent":{"id":item_id,"revision":"23","principal":"anonymous"},
                                "content":[]
                            })
                        }
                    }
                    _ => return StatusCode::BAD_REQUEST.into_response(),
                };
                axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result})).into_response()
            }
        }
    });
    let mut mcp_fixture = tokio::task::JoinSet::new();
    mcp_fixture.spawn(async move {
        axum::serve(
            mcp_fixture_listener,
            axum::Router::new().fallback(mcp_fixture_handler),
        )
        .await
        .unwrap();
    });

    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let mcp_port = ctx.mcp_port();
    fs::create_dir_all(ctx.cwd_path_join("mcp-roundtrip")).unwrap();
    ctx.cd("mcp-roundtrip");
    for component in [
        "mcp-roundtrip:provider",
        "mcp-roundtrip:export-owner",
        "mcp-roundtrip:middleware",
        "mcp-roundtrip:consumer",
    ] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "rust",
                flag::COMPONENT_NAME,
                component,
            ])
            .await;
        assert!(output.success_or_dump());
    }

    write_roundtrip_manifest(&ctx, mcp_port, mcp_fixture_port, true, false);
    write_roundtrip_provider(&ctx, effect_port);
    write_roundtrip_middleware(&ctx, effect_port);
    write_roundtrip_consumer(&ctx);

    let built = ctx
        .cli([flag::YES, "--environment", "export", cmd::BUILD])
        .await;
    assert!(built.success_or_dump());
    let deployed = ctx
        .cli([flag::YES, "--environment", "export", cmd::DEPLOY])
        .await;
    assert!(deployed.success_or_dump());

    let mcp_url = format!("http://localhost:{mcp_port}/mcp");
    let client = reqwest::Client::new();
    let listed = mcp_request(&client, &mcp_url, "tools/list", json!({})).await;
    *projected_tools.lock().unwrap() = Some(listed.clone());
    let tools = listed["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["artifact_build", "artifact_render", "artifact_touch"])
    );
    let render = tools
        .iter()
        .find(|tool| tool["name"] == "artifact_render")
        .unwrap();
    assert_eq!(
        render["inputSchema"]["properties"]["item-id"]["type"],
        "string"
    );
    assert_eq!(
        render["inputSchema"]["properties"]["_stdin"]["type"],
        "string"
    );
    assert!(render["inputSchema"]["properties"]["_stdin"]["contentEncoding"].is_null());
    assert_eq!(render["outputSchema"]["type"], "object");
    if !render["outputSchema"]["properties"]["id"].is_object()
        || !render["outputSchema"]["properties"]["value"].is_null()
    {
        contract_failures.push(format!(
            "CONTRACT-2 record results must expose fields directly without a value wrapper: {}",
            render["outputSchema"]
        ));
    }
    assert!(
        render["description"]
            .as_str()
            .unwrap()
            .contains("finite buffered data (16 MiB per direction)")
    );

    let direct = mcp_request(
        &client,
        &mcp_url,
        "tools/call",
        json!({"name":"artifact_render","arguments":{"item-id":"direct","_stdin":"AAEC/w=="}}),
    )
    .await;
    assert_exported_result(&direct, "direct", "AAEC/w==");
    if !direct["structuredContent"]["id"].is_string()
        || !direct["structuredContent"]["value"].is_null()
    {
        contract_failures.push(format!(
            "CONTRACT-2 record results must expose fields directly in structuredContent: {}",
            direct["structuredContent"]
        ));
    }
    let rejected = mcp_request(
        &client,
        &mcp_url,
        "tools/call",
        json!({"name":"artifact_render","arguments":{"item-id":"reject","_stdin":""}}),
    )
    .await;
    assert_eq!(rejected["isError"], true);
    let mapped: Value =
        serde_json::from_str(rejected["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(mapped["name"], "rejected");
    assert_eq!(mapped["payload"], "reject");

    for (command, expected_fragments) in [
        (
            "secret",
            &["unsupported capability", "secret", "$.field[\"secret\"]"][..],
        ),
        ("nested-stream", &["typed streams", "input"][..]),
    ] {
        write_roundtrip_manifest(&ctx, mcp_port, mcp_fixture_port, true, false);
        let manifest_path = ctx.cwd_path_join("golem.yaml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        fs::write_str(
            &manifest_path,
            manifest.replace("include: [render, touch]", &format!("include: [{command}]")),
        )
        .unwrap();
        let deployed = ctx
            .cli([flag::YES, "--environment", "export", cmd::DEPLOY])
            .await;
        let diagnostic = if deployed.success() {
            let response = mcp_response(&client, &mcp_url, "tools/list", json!({})).await;
            response["error"].to_string()
        } else {
            format!("{}\n{}", deployed.stdout_text(), deployed.stderr_text())
        };
        for fragment in expected_fragments {
            if !diagnostic.contains(fragment) {
                contract_failures.push(format!(
                    "unsupported {command} export did not report {fragment:?}: {diagnostic}"
                ));
            }
        }
    }

    write_roundtrip_manifest(&ctx, mcp_port, mcp_fixture_port, true, false);
    let restored = ctx
        .cli([flag::YES, "--environment", "export", cmd::DEPLOY])
        .await;
    assert!(restored.success_or_dump());

    write_roundtrip_manifest(&ctx, mcp_port, mcp_fixture_port, true, true);
    let built = ctx
        .cli([flag::YES, "--environment", "import", cmd::BUILD])
        .await;
    assert!(built.success_or_dump());
    let deployed = ctx
        .cli([flag::YES, "--environment", "import", cmd::DEPLOY])
        .await;
    assert!(deployed.success_or_dump());
    let agent = "RoundtripConsumer(\"identity-boundary\")";
    let output = ctx
        .cli([
            flag::YES,
            "--environment",
            "import",
            cmd::AGENT,
            cmd::INVOKE,
            agent,
            "run",
        ])
        .await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains("roundtrip:projected:23:anonymous"));
    assert!(output.stdout_contains("error:rejected:projected-error"));

    assert_eq!(
        effects
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.starts_with("/provider/"))
            .count(),
        4,
        "direct and round-trip success/error calls must each dispatch once"
    );
    assert_eq!(
        effects
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.starts_with("/middleware/artifact-touch/"))
            .count(),
        2,
        "the imported-side universal middleware observes success and declared error"
    );
    let observed_effects = effects.lock().unwrap().clone();
    assert_eq!(
        observed_effects
            .iter()
            .filter(|path| path.as_str() == "/middleware/artifact-touch/golem-user")
            .count(),
        2,
        "the authenticated importing identity is visible to middleware before the MCP boundary: {:?}",
        observed_effects
    );
    assert_eq!(
        observed_effects
            .iter()
            .filter(|path| path.starts_with("/provider/") && path.ends_with("/anonymous"))
            .count(),
        4,
        "all direct and round-trip provider effects must be anonymous across the MCP boundary: {:?}",
        observed_effects
    );

    mcp_fixture.shutdown().await;
    write_roundtrip_manifest(&ctx, mcp_port, mcp_fixture_port, false, false);
    let removed = ctx
        .cli([flag::YES, "--environment", "export", cmd::DEPLOY])
        .await;
    assert!(removed.success_or_dump());
    write_roundtrip_manifest(&ctx, mcp_port, mcp_fixture_port, false, true);
    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;

    let replayed = ctx
        .cli([
            flag::YES,
            "--environment",
            "import",
            cmd::AGENT,
            cmd::INVOKE,
            agent,
            "status",
        ])
        .await;
    assert!(replayed.success_or_dump());
    assert!(replayed.stdout_contains("roundtrip:projected:23:anonymous"));
    assert!(replayed.stdout_contains("error:rejected:projected-error"));
    assert_eq!(
        effects.lock().unwrap().len(),
        6,
        "completed replay must not repeat upstream or middleware effects"
    );

    let fresh = ctx
        .cli([
            flag::YES,
            "--environment",
            "import",
            cmd::AGENT,
            cmd::INVOKE,
            "RoundtripConsumer(\"fresh-offline\")",
            "run",
        ])
        .await;
    assert!(
        !fresh.success(),
        "a fresh invocation must fail after the exported deployment and MCP source are removed"
    );
    assert_eq!(
        effects.lock().unwrap().len(),
        6,
        "a failed fresh invocation must not manufacture provider or middleware effects"
    );
    assert!(
        contract_failures.is_empty(),
        "CONTRACT-2 failures:\n{}",
        contract_failures.join("\n")
    );

    effect_server.shutdown().await;
}

#[test]
#[timeout("15 minutes")]
async fn typescript_mcp_client_projects_contract_and_runs_imported_middleware() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
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
                    != Some("Bearer typescript-mcp-token")
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                let result = match body["method"].as_str() {
                    Some("tools/list") => json!({"tools":[{
                        "name":"catalog.lookup",
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
                        }
                    }]}),
                    Some("tools/call") => {
                        if body["params"]["name"] != "catalog.lookup"
                            || headers
                                .get("mcp-name")
                                .and_then(|value| value.to_str().ok())
                                != Some("catalog.lookup")
                        {
                            return StatusCode::BAD_REQUEST.into_response();
                        }
                        let arguments = body["params"]["arguments"].clone();
                        let Some(item_id) = arguments["item-id"].as_str().map(str::to_owned) else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
                        calls.lock().unwrap().push(arguments);
                        if item_id == "error" {
                            json!({
                                "isError":true,
                                "content":[{"type":"text","text":"upstream denied"}]
                            })
                        } else {
                            let content = match item_id.as_str() {
                                "none" => json!([]),
                                "blocks" => json!([
                                    {"type":"text","text":"left"},
                                    {"type":"text","text":"right"}
                                ]),
                                "resource" => json!([{
                                    "type":"resource",
                                    "resource":{
                                        "uri":"file:///exact.bin",
                                        "blob":"ECAw",
                                        "mimeType":"application/x-exact"
                                    }
                                }]),
                                _ => json!([{"type":"text","text":format!("stdout:{item_id}")}]),
                            };
                            json!({
                                "structuredContent":{"id":item_id,"revision":7},
                                "content":content
                            })
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
    fs::create_dir_all(ctx.cwd_path_join("typescript-mcp-client")).unwrap();
    ctx.cd("typescript-mcp-client");
    for component in ["consumer", "middleware"] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "ts",
                flag::COMPONENT_NAME,
                &format!("typescript-mcp-client:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: typescript-mcp-client
        environments:
          local:
            server: local
            componentPresets: quick
            tools:
              middleware: [typescript-mcp-audit]
        components:
          typescript-mcp-client:consumer:
            dir: consumer
            templates: ts
            dependencies:
              tools: [catalog-lookup]
          typescript-mcp-client:middleware:
            dir: middleware
            templates: ts
        mcp:
          imports:
            local:
              - url: http://127.0.0.1:{port}/mcp
                auth:
                  bearer: typescript-mcp-token
        tools:
          middleware:
            typescript-mcp-audit:
              component: typescript-mcp-client:middleware
        agents:
          TypeScriptMcpConsumer:
            tools:
              catalog-lookup: {{}}
        bridge:
          ts:
            internal:
              tools: [catalog-lookup]
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    add_typescript_tool_client_source(
        &ctx,
        "catalog-lookup-tool-guest-client",
        "catalog-lookup-tool-guest-client.ts",
    );
    fs::write_str(
        ctx.cwd_path_join("consumer/src/counter-agent.ts"),
        indoc! {r#"
        import { z } from 'zod';
        import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
        import { CatalogLookupClient } from 'catalog-lookup-tool-guest-client';

        const TypeScriptMcpConsumer = defineAgent({
          name: 'TypeScriptMcpConsumer',
          id: { name: z.string() },
          methods: {
            call: method({ input: { itemId: z.string(), includeHistory: z.boolean() }, returns: z.array(z.string()) }),
          },
        });

        TypeScriptMcpConsumer.implement({
          init: () => ({}),
          methods: {
            async call({ itemId, includeHistory }) {
              try {
                const invocation = CatalogLookupClient.newClient().catalog_lookup(includeHistory, itemId);
                const collected = await invocation.collect();
                const projected = JSON.stringify(collected.result, (_key, value) =>
                  typeof value === 'bigint' ? value.toString() : value,
                );
                return [projected, collected.stdout === undefined ? '' : new TextDecoder().decode(collected.stdout)];
              } catch (error) {
                const failure = error as {
                  tag?: string;
                  error?: { tag?: string; value?: string };
                };
                if (
                  failure.tag === 'tool' &&
                  failure.error?.tag === 'McpToolError'
                ) {
                  return [`tool:mcp-tool-error:${failure.error.value}`, ''];
                }
                throw error;
              }
            },
          },
        });
    "#},
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("middleware/src/counter-agent.ts"),
        indoc! {r#"
        import { universalToolMiddleware } from '@golemcloud/golem-ts-sdk';

        universalToolMiddleware({
          name: 'typescript-mcp-audit',
          invoke: async (request, { underlying }) => {
            const result = await underlying.invokeAndAwait(request.commandPath, request.input, request.stdin);
            const upstream = result.stdout;
            if (upstream === undefined) return result;
            return {
              ...result,
              stdout: (async function* () {
                yield* new TextEncoder().encode('middleware:');
                yield* upstream;
              })(),
            };
          },
        });
    "#},
    )
    .unwrap();

    let built = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(built.success_or_dump());
    let generated_root =
        ctx.cwd_path_join("golem-temp/bridge-sdk/ts/internal/catalog-lookup-tool-guest-client");
    let generated = generated_source_text(&generated_root);
    assert!(generated.contains("catalog-lookup"));
    assert!(!generated.contains("typescript-mcp-token"));
    assert!(!generated.contains(&format!("127.0.0.1:{port}")));

    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    for (item_id, include_history, expected) in [
        (
            "text",
            false,
            vec![
                "\"structured\":{\"id\":\"text\",\"revision\":\"7\"}",
                "\"tag\":\"streamed\"",
                "stdout:text",
            ],
        ),
        (
            "none",
            false,
            vec![
                "\"structured\":{\"id\":\"none\",\"revision\":\"7\"}",
                "\"tag\":\"none\"",
            ],
        ),
        ("blocks", true, vec!["\"tag\":\"blocks\"", "left", "right"]),
        (
            "resource",
            false,
            vec!["file:///exact.bin", "application/x-exact"],
        ),
        ("error", false, vec!["tool:mcp-tool-error:upstream denied"]),
        ("middleware", true, vec!["middleware:stdout:middleware"]),
    ] {
        let output = ctx
            .cli([
                flag::FORMAT,
                "json",
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                "TypeScriptMcpConsumer(\"contract-2\")",
                "call",
                &serde_json::to_string(item_id).unwrap(),
                if include_history { "true" } else { "false" },
            ])
            .await;
        assert!(output.success_or_dump());
        let result = output
            .stdout_json::<Value>()
            .into_iter()
            .find(|event| event["$type"] == "agent.invoke")
            .expect("TypeScript MCP consumer returned an invocation result");
        let observed = result["resultJson"]["value"]["value"]["elements"]
            .as_array()
            .expect("TypeScript MCP consumer returned a string list")
            .iter()
            .map(|value| value["value"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        for fragment in expected {
            assert!(
                observed.contains(fragment),
                "missing {fragment} for {item_id}: {observed}"
            );
        }
    }
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            json!({"item-id":"text","includeHistory":false}),
            json!({"item-id":"none","includeHistory":false}),
            json!({"item-id":"blocks","includeHistory":true}),
            json!({"item-id":"resource","includeHistory":false}),
            json!({"item-id":"error","includeHistory":false}),
            json!({"item-id":"middleware","includeHistory":true}),
        ]
    );
    upstream.shutdown().await;
}

#[test]
#[timeout("15 minutes")]
async fn rust_mcp_clients_use_registry_credentials_and_replay_offline() {
    let calls = Arc::new(Mutex::new(Vec::<(String, Value, String)>::new()));
    let listings = Arc::new(Mutex::new(Vec::<String>::new()));
    let refreshed_schema = Arc::new(AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = axum::routing::post({
        let calls = calls.clone();
        let listings = listings.clone();
        let refreshed_schema = refreshed_schema.clone();
        move |uri: Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let calls = calls.clone();
            let listings = listings.clone();
            let refreshed_schema = refreshed_schema.clone();
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
                        listings.lock().unwrap().push(uri.path().to_string());
                        let mut tools = vec![json!({
                            "name":"lookup",
                            "inputSchema":{
                                "type":"object", "properties":{"query":{"type":"string"},"limit":{"type":"integer"}},
                                "required":["query"], "additionalProperties":{"type":"string"}
                            },
                            "outputSchema":{
                                "type":"object", "properties":{"answer":{"type":"string"},"score":{"type":"integer"}},
                                "required":["answer","score"], "additionalProperties":false
                            }
                        })];
                        if uri.path() == "/bearer" && refreshed_schema.load(Ordering::SeqCst) {
                            tools.push(json!({
                                "name":"fresh",
                                "inputSchema":{
                                    "type":"object", "properties":{"token":{"type":"string"}},
                                    "required":["token"], "additionalProperties":false
                                },
                                "outputSchema":{
                                    "type":"object", "properties":{"generation":{"type":"integer"}},
                                    "required":["generation"], "additionalProperties":false
                                }
                            }));
                        }
                        json!({"tools":tools})
                    }
                    Some("tools/call") => {
                        let tool_name = body["params"]["name"].as_str();
                        if !matches!(tool_name, Some("lookup" | "fresh"))
                            || headers
                                .get("mcp-name")
                                .and_then(|value| value.to_str().ok())
                                != tool_name
                        {
                            return StatusCode::BAD_REQUEST.into_response();
                        }
                        let arguments = body["params"]["arguments"].clone();
                        let Some(key) = headers
                            .get("idempotency-key")
                            .and_then(|value| value.to_str().ok())
                            .filter(|value| !value.is_empty())
                        else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
                        calls.lock().unwrap().push((
                            uri.path().into(),
                            arguments.clone(),
                            key.into(),
                        ));
                        if tool_name == Some("fresh") {
                            if arguments["token"] != "second-contract" {
                                return StatusCode::BAD_REQUEST.into_response();
                            }
                            return axum::Json(json!({
                                "jsonrpc":"2.0",
                                "id":body["id"],
                                "result":{"structuredContent":{"generation":2},"content":[]}
                            }))
                            .into_response();
                        }
                        let Some(query) = arguments["query"].as_str().map(str::to_owned) else {
                            return StatusCode::BAD_REQUEST.into_response();
                        };
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
                    let collected = BearerLookupClient::new().bearer_lookup(limit, query.into(), extras).await.unwrap().collect().await;
                    let result = collected.result.unwrap();
                    let stdout = collected.stdout.unwrap().unwrap_or_default();
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
                    let collected = BasicLookupClient::new().basic_lookup(limit, query.into(), extras).await.unwrap().collect().await;
                    let result = collected.result.unwrap();
                    let stdout = collected.stdout.unwrap().unwrap_or_default();
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
        listings
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["/bearer".into(), "/basic".into()])
    );
    assert!(
        calls.lock().unwrap().is_empty(),
        "codegen only discovers tools"
    );
    let marker_before_cache_hit = bridge_marker_bytes(ctx.cwd_path());
    let generated_before_cache_hit = generated_source_text(
        &ctx.cwd_path_join("golem-temp/bridge-sdk/rust/internal/bearer-lookup-tool-guest-client"),
    );
    let cached_build = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(cached_build.success_or_dump());
    assert_eq!(
        bridge_marker_bytes(ctx.cwd_path()),
        marker_before_cache_hit,
        "an unchanged build must retain the same bridge marker"
    );
    assert_eq!(
        generated_source_text(
            &ctx.cwd_path_join(
                "golem-temp/bridge-sdk/rust/internal/bearer-lookup-tool-guest-client",
            )
        ),
        generated_before_cache_hit,
        "an unchanged resolved source identity must retain the generated bridge"
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

    let bearer_listings_before_refresh = listings
        .lock()
        .unwrap()
        .iter()
        .filter(|path| path.as_str() == "/bearer")
        .count();
    refreshed_schema.store(true, Ordering::SeqCst);
    let refreshed = ctx.cli(["api", "mcp-import", "refresh", "0"]).await;
    assert!(refreshed.success_or_dump());
    assert!(refreshed.stdout_contains("bearer-fresh"));
    assert_eq!(
        listings
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.as_str() == "/bearer")
            .count(),
        bearer_listings_before_refresh + 1,
        "refresh must fetch the selected import exactly once"
    );

    let manifest_path = ctx.cwd_path_join("golem.yaml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    fs::write_str(
        &manifest_path,
        manifest.replace(
            "tools: [bearer-lookup, basic-lookup]",
            "tools: [bearer-lookup, basic-lookup, bearer-fresh]",
        ),
    )
    .unwrap();
    let cargo_path = ctx.cwd_path_join("Cargo.toml");
    let cargo = fs::read_to_string(&cargo_path).unwrap();
    fs::write_str(
        &cargo_path,
        cargo.replace(
            "bearer-lookup-tool-guest-client =",
            "bearer-fresh-tool-guest-client = { path = \"golem-temp/bridge-sdk/rust/internal/bearer-fresh-tool-guest-client\" }\nbearer-lookup-tool-guest-client =",
        ),
    )
    .unwrap();
    let source_path = ctx.cwd_path_join("src/counter_agent.rs");
    let source = fs::read_to_string(&source_path).unwrap();
    let source = source
        .replace(
            "use bearer_lookup_tool_guest_client::",
            "use bearer_fresh_tool_guest_client::BearerFreshClient;\nuse bearer_lookup_tool_guest_client::",
        )
        .replace(
            "fn status(&self) -> Vec<String>;",
            "fn status(&self) -> Vec<String>;\n            async fn refreshed_contract(&self) -> i64;",
        )
        .replace(
            "fn status(&self) -> Vec<String> { self.results.clone() }",
            r#"fn status(&self) -> Vec<String> { self.results.clone() }
            async fn refreshed_contract(&self) -> i64 {
                BearerFreshClient::new()
                    .bearer_fresh("second-contract".into())
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap()
                    .result
                    .structured
                    .generation
            }"#,
        );
    fs::write_str(&source_path, source).unwrap();

    let rebuilt = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(rebuilt.success_or_dump());
    let fresh_client =
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/internal/bearer-fresh-tool-guest-client");
    assert!(fresh_client.join("Cargo.toml").is_file());
    assert!(generated_source_text(&fresh_client).contains("bearer-fresh"));
    assert_ne!(
        bridge_marker_bytes(ctx.cwd_path()),
        marker_before_cache_hit,
        "the refreshed projection must invalidate the bridge marker"
    );
    let redeployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(redeployed.success_or_dump());
    let refreshed_output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "McpConsumer(\"refresh\")",
            "refreshed_contract",
        ])
        .await;
    assert!(refreshed_output.success_or_dump());
    assert!(refreshed_output.stdout_contains("2"));
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 11);
        assert_eq!(calls[10].0, "/bearer");
        assert_eq!(calls[10].1, json!({"token":"second-contract"}));
        assert!(calls[..10].iter().all(|call| call.2 != calls[10].2));
    }

    let fresh_source_before_unavailable = generated_source_text(&fresh_client);
    upstream.shutdown().await;
    let unavailable_refresh = ctx.cli(["api", "mcp-import", "refresh", "0"]).await;
    assert!(!unavailable_refresh.success());
    assert!(
        unavailable_refresh.stderr_contains("MCP_IMPORT_UPSTREAM_UNAVAILABLE")
            || unavailable_refresh.stdout_contains("MCP_IMPORT_UPSTREAM_UNAVAILABLE"),
        "refresh failure must explain that the MCP source is inaccessible"
    );
    assert_eq!(
        generated_source_text(&fresh_client),
        fresh_source_before_unavailable,
        "a failed refresh must preserve the last generated client"
    );
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
    assert_eq!(calls.lock().unwrap().len(), 11);
}

fn write_roundtrip_manifest(
    ctx: &TestContext,
    mcp_port: u16,
    mcp_fixture_port: u16,
    export_enabled: bool,
    import_enabled: bool,
) {
    let deployment = if export_enabled {
        format!(
            "  deployments:\n    export:\n      - domain: localhost:{mcp_port}\n        tools:\n          artifact:\n            ownerComponent: mcp-roundtrip:export-owner\n            include: [render, touch]\n"
        )
    } else {
        "  deployments:\n    export: []\n".to_string()
    };
    let import_environment = if import_enabled {
        "  import:\n    server: local\n    componentPresets: debug\n    tools:\n      middleware: [roundtrip-audit]\n"
    } else {
        ""
    };
    let import_components = if import_enabled {
        "  mcp-roundtrip:middleware:\n    dir: middleware\n    templates: rust\n  mcp-roundtrip:consumer:\n    dir: consumer\n    templates: rust\n    dependencies:\n      tools: [artifact-touch]\n"
    } else {
        ""
    };
    let import_tools = if import_enabled {
        "  middleware:\n    roundtrip-audit:\n      component: mcp-roundtrip:middleware\n"
    } else {
        ""
    };
    let import_agent = if import_enabled {
        "agents:\n  RoundtripConsumer:\n    tools:\n      artifact-touch: {}\n"
    } else {
        ""
    };
    let import_source = if import_enabled {
        format!("  imports:\n    import:\n      - url: http://127.0.0.1:{mcp_fixture_port}/mcp\n")
    } else {
        String::new()
    };
    let bridge = if import_enabled {
        "bridge:\n  rust:\n    internal:\n      tools: [artifact-touch]\n"
    } else {
        ""
    };
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {version}
            app: mcp-roundtrip
            environments:
              export:
                server: local
                componentPresets: debug
            {import_environment}
            components:
              mcp-roundtrip:provider:
                dir: provider
                templates: rust
              mcp-roundtrip:export-owner:
                dir: export-owner
                templates: rust
                dependencies:
                  tools: [mcp-roundtrip:provider/artifact]
                tools:
                  artifact: {{}}
            {import_components}
            tools:
              artifact:
                component: mcp-roundtrip:provider
            {import_tools}
            {import_agent}
            mcp:
            {deployment}{import_source}
            {bridge}
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
}

fn write_roundtrip_provider(ctx: &TestContext, effect_port: u16) {
    fs::write_str(
        ctx.cwd_path_join("provider/src/counter_agent.rs"),
        formatdoc! {r#"
            use futures_concurrency::prelude::*;
            use golem_rust::agentic::{{AgentStream, InputStream, OutputStream}};
            use golem_rust::secrets::GuestSecretHandle;
            use golem_rust::{{FromSchema, FromWire, IntoSchema, IntoWire, ToolError, WireSchema, tool_definition, tool_implementation}};

            #[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
            pub struct Rendered {{
                pub id: String,
                pub revision: u64,
                pub principal: String,
            }}

            #[derive(Debug, Clone, ToolError)]
            pub enum RenderError {{
                #[tool_error(kind = "usage-error", exit_code = 23)]
                Rejected {{ item_id: String }},
            }}

            #[tool_definition(version = "1.0.0")]
            pub trait Artifact {{
                /// Render one artifact into finite output channels.
                #[command(aliases = ["build"], annotations(idempotent = true))]
                #[arg(item_id = "option", required = true)]
                #[arg(diagnostics, channel = "stderr")]
                async fn render(
                    &self,
                    item_id: String,
                    stdin: InputStream,
                    stdout: OutputStream,
                    diagnostics: OutputStream,
                    principal: golem_rust::tool::Principal,
                ) -> Result<Rendered, RenderError>;

                /// Record one projectable non-streaming effect.
                #[arg(item_id = "option", required = true)]
                async fn touch(
                    &self,
                    item_id: String,
                    principal: golem_rust::tool::Principal,
                ) -> Result<Rendered, RenderError>;

                /// This command is intentionally not projectable to MCP.
                fn secret(&self, secret: GuestSecretHandle) -> String;

                /// This command is intentionally not projectable to MCP.
                fn nested_stream(&self, stream: AgentStream<String>) -> String;
            }}

            struct ArtifactImpl;

            #[tool_implementation]
            impl Artifact for ArtifactImpl {{
                async fn render(
                    &self,
                    item_id: String,
                    mut stdin: InputStream,
                    mut stdout: OutputStream,
                    mut diagnostics: OutputStream,
                    principal: golem_rust::tool::Principal,
                ) -> Result<Rendered, RenderError> {{
                    record_effect(&format!("/provider/{{item_id}}/{{}}", principal_class(&principal))).await;
                    let mut bytes = Vec::new();
                    while let Some(chunk) = stdin.next().await {{
                        bytes.extend(chunk.expect("MCP stdin is readable"));
                    }}
                    stdout.write(bytes).await.expect("write stdout");
                    stdout.finish().await.expect("finish stdout");
                    diagnostics
                        .write(format!("warning:{{item_id}}").into_bytes())
                        .await
                        .expect("write stderr");
                    diagnostics.finish().await.expect("finish stderr");
                    if item_id == "reject" {{
                        Err(RenderError::Rejected {{ item_id }})
                    }} else {{
                        Ok(Rendered {{
                            id: item_id,
                            revision: 23,
                            principal: principal_class(&principal).to_string(),
                        }})
                    }}
                }}

                async fn touch(
                    &self,
                    item_id: String,
                    principal: golem_rust::tool::Principal,
                ) -> Result<Rendered, RenderError> {{
                    record_effect(&format!("/provider/{{item_id}}/{{}}", principal_class(&principal))).await;
                    if item_id == "reject" {{
                        Err(RenderError::Rejected {{ item_id }})
                    }} else {{
                        Ok(Rendered {{
                            id: item_id,
                            revision: 23,
                            principal: principal_class(&principal).to_string(),
                        }})
                    }}
                }}

                fn secret(&self, _secret: GuestSecretHandle) -> String {{
                    "unsupported".into()
                }}

                fn nested_stream(&self, _stream: AgentStream<String>) -> String {{
                    "unsupported".into()
                }}
            }}

            fn principal_class(principal: &golem_rust::tool::Principal) -> &'static str {{
                match principal {{
                    golem_rust::tool::Principal::Anonymous => "anonymous",
                    golem_rust::tool::Principal::Oidc(_) => "oidc",
                    golem_rust::tool::Principal::Agent(_) => "agent",
                    golem_rust::tool::Principal::GolemUser(_) => "golem-user",
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
    add_dependency(
        &ctx.cwd_path_join("provider/Cargo.toml"),
        "futures-concurrency = \"7.6.3\"",
    );
}

fn write_roundtrip_middleware(ctx: &TestContext, effect_port: u16) {
    fs::write_str(
        ctx.cwd_path_join("middleware/src/counter_agent.rs"),
        formatdoc! {r#"
            use futures_concurrency::prelude::*;
            use golem_rust::tool::{{InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool, ToolInvokeError, UnderlyingTool}};
            use golem_rust::{{TypedSchemaValue, universal_tool_middleware}};

            #[universal_tool_middleware(name = "roundtrip-audit")]
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
    add_dependency(
        &ctx.cwd_path_join("middleware/Cargo.toml"),
        "futures-concurrency = \"7.6.3\"",
    );
}

fn write_roundtrip_consumer(ctx: &TestContext) {
    fs::write_str(
        ctx.cwd_path_join("consumer/src/counter_agent.rs"),
        indoc! {r#"
            use artifact_touch_tool_guest_client::ArtifactTouchClient;
            use golem_rust::{agent_definition, agent_implementation};

            #[agent_definition]
            pub trait RoundtripConsumer {
                fn new(name: String) -> Self;
                async fn run(&mut self) -> Vec<String>;
                fn status(&self) -> Vec<String>;
            }

            struct Consumer { results: Vec<String> }

            #[agent_implementation]
            impl RoundtripConsumer for Consumer {
                fn new(_name: String) -> Self { Self { results: Vec::new() } }

                async fn run(&mut self) -> Vec<String> {
                    let result = ArtifactTouchClient::new()
                        .artifact_touch("projected".into())
                        .await
                        .unwrap()
                        .collect()
                        .await
                        .unwrap()
                        .result;
                    self.results.push(format!(
                        "roundtrip:{}:{}:{}",
                        result.structured.id,
                        result.structured.revision,
                        result.structured.principal,
                    ));
                    let rejected = ArtifactTouchClient::new()
                        .artifact_touch("reject".into())
                        .await
                        .unwrap();
                    match rejected.collect().await {
                            Ok(_) => panic!("rejected render unexpectedly succeeded"),
                            Err(_) => {}
                    }
                    self.results.push("error:rejected:projected-error".into());
                    self.results.clone()
                }

                fn status(&self) -> Vec<String> { self.results.clone() }
            }
        "#},
    )
    .unwrap();
    let manifest = ctx.cwd_path_join("consumer/Cargo.toml");
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write_str(
        &manifest,
        source.replace(
            "[dependencies]",
            indoc! {r#"
                [dependencies]
                artifact-touch-tool-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/artifact-touch-tool-guest-client" }
            "#}
            .trim_end(),
        ),
    )
    .unwrap();
}

fn add_dependency(manifest: &std::path::Path, dependency: &str) {
    let source = fs::read_to_string(manifest).unwrap();
    fs::write_str(
        manifest,
        source.replace("[dependencies]", &format!("[dependencies]\n{dependency}")),
    )
    .unwrap();
}

fn parse_mcp_sse(body: &str) -> Value {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .find(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).unwrap())
        .unwrap_or_else(|| panic!("MCP response has no SSE data: {body}"))
}

async fn mcp_request(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Value {
    mcp_response(client, url, method, params).await["result"].clone()
}

async fn mcp_response(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Value {
    let initialize = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"chunk-j","version":"1"}}
        }))
        .send()
        .await
        .unwrap();
    let session = initialize
        .headers()
        .get("mcp-session-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(initialize.status().is_success());
    parse_mcp_sse(&initialize.text().await.unwrap());
    let initialized = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert!(initialized.status().is_success());
    let response = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .json(&json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    parse_mcp_sse(&response.text().await.unwrap())
}

fn assert_exported_result(result: &Value, item_id: &str, stdout: &str) {
    assert_eq!(result["isError"], false);
    let structured = if result["structuredContent"]["value"].is_object() {
        &result["structuredContent"]["value"]
    } else {
        &result["structuredContent"]
    };
    assert_eq!(structured["id"], item_id);
    assert_eq!(structured["revision"], "23");
    assert_eq!(structured["principal"], "anonymous");
    assert_eq!(result["content"][0]["type"], "text");
    let rendered: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(rendered, result["structuredContent"]);
    assert_eq!(result["content"][1]["type"], "text");
    assert_eq!(result["content"][1]["text"], stdout);
    assert_eq!(
        result["content"][1]["_meta"]["golem.cloud/tool-channel"],
        "stdout"
    );
    assert_eq!(result["content"][2]["type"], "text");
    assert_eq!(result["content"][2]["text"], format!("warning:{item_id}"));
    assert_eq!(
        result["content"][2]["_meta"]["golem.cloud/tool-channel"],
        "stderr"
    );
}

#[test]
fn exported_result_assertion_accepts_contract_2_record_shape() {
    let structured = json!({
        "id": "direct",
        "revision": "23",
        "principal": "anonymous"
    });
    let result = json!({
        "isError": false,
        "structuredContent": structured,
        "content": [
            {"type": "text", "text": structured.to_string()},
            {
                "type": "text",
                "text": "AAEC/w==",
                "_meta": {"golem.cloud/tool-channel": "stdout"}
            },
            {
                "type": "text",
                "text": "warning:direct",
                "_meta": {"golem.cloud/tool-channel": "stderr"}
            }
        ]
    });

    assert_exported_result(&result, "direct", "AAEC/w==");
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

fn bridge_marker_bytes(root: &std::path::Path) -> Vec<Vec<u8>> {
    let marker_root = root.join("golem-temp/task-results");
    let mut markers = std::fs::read_dir(marker_root)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter(|bytes| {
            serde_json::from_slice::<Value>(bytes)
                .ok()
                .and_then(|value| value.get("kind").cloned())
                == Some(Value::String("GenerateBridgeSdkMarkerHash".into()))
        })
        .collect::<Vec<_>>();
    markers.sort();
    markers
}

fn add_typescript_tool_client_source(
    ctx: &TestContext,
    package_name: &str,
    generated_source_name: &str,
) {
    let tsconfig_path = ctx.cwd_path_join("consumer/tsconfig.json");
    let mut tsconfig: Value =
        serde_json::from_str(&fs::read_to_string(&tsconfig_path).unwrap()).unwrap();
    tsconfig["compilerOptions"]["paths"][package_name] = json!([format!(
        "../golem-temp/bridge-sdk/ts/internal/{package_name}/{generated_source_name}"
    )]);
    tsconfig["include"].as_array_mut().unwrap().extend([
        json!("src/**/*.ts"),
        json!(format!(
            "../golem-temp/bridge-sdk/ts/internal/{package_name}/*.ts"
        )),
    ]);
    fs::write_str(
        tsconfig_path,
        serde_json::to_string_pretty(&tsconfig).unwrap() + "\n",
    )
    .unwrap();
}
