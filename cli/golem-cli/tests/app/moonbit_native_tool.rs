use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use test_r::{test, timeout};

#[test]
#[timeout("20 minutes")]
async fn moonbit_ambient_native_client_executes_through_golem() {
    let mut ctx = TestContext::new();
    ctx.enable_native_conformance_tool();
    ctx.start_server().await;

    fs::create_dir_all(ctx.cwd_path_join("moonbit-native-client")).unwrap();
    ctx.cd("moonbit-native-client");
    for (template, component) in [("moonbit", "consumer"), ("rust", "middleware")] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                &format!("moonbit-native-client:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {version}
            app: moonbit-native-client
            environments:
              local:
                server: local
                componentPresets: debug
                tools:
                  middleware: [native-conformance-audit]
            components:
              moonbit-native-client:consumer:
                dir: consumer
                templates: moonbit
                dependencies:
                  tools: [native-conformance]
              moonbit-native-client:middleware:
                dir: middleware
                templates: rust
            tools:
              middleware:
                native-conformance-audit:
                  component: moonbit-native-client:middleware
            agents:
              NativeMoonbitConsumer:
                tools:
                  native-conformance: {{}}
              UnauthorizedNativeMoonbitConsumer:
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
              moonbit:
                internal:
                  tools: [native-conformance]
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    let moon_mod_path = ctx.cwd_path_join("moon.mod.json");
    let mut moon_mod: Value =
        serde_json::from_str(&fs::read_to_string(&moon_mod_path).unwrap()).unwrap();
    moon_mod["deps"]["native-conformance-tool-guest-client"] = json!({
        "path": "golem-temp/bridge-sdk/moonbit/internal/native-conformance-tool-guest-client"
    });
    fs::write_str(
        &moon_mod_path,
        serde_json::to_string_pretty(&moon_mod).unwrap() + "\n",
    )
    .unwrap();

    let consumer_pkg_path = ctx.cwd_path_join("consumer/moon.pkg");
    let consumer_pkg = fs::read_to_string(&consumer_pkg_path).unwrap();
    fs::write_str(
        &consumer_pkg_path,
        consumer_pkg.replace(
            "import {",
            indoc! {r#"import {
              "native-conformance-tool-guest-client/client" @native,
              "golemcloud/golem_sdk/tool" @tool,
              "moonbitlang/core/encoding/utf8","#},
        ),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/counter.mbt"),
        indoc! {r#"
            ///|
            #derive.agent
            struct NativeMoonbitConsumer {
              name : String
            }

            ///|
            fn NativeMoonbitConsumer::new(name : String) -> NativeMoonbitConsumer {
              { name, }
            }

            ///|
            pub async fn NativeMoonbitConsumer::exercise(self : Self) -> Array[String] {
              ignore(self.name)
              let client = @native.NativeConformanceClient::new()
              defer client.drop()
              let structured = match client.structured("alpha", 7UL) {
                Ok(evidence) => evidence
                Err(_) => abort("structured native invocation failed")
              }
              let supported_error = match client.supported_error("expected") {
                Err(@tool.ToolError::Tool(_)) => "error:rejected:expected"
                Err(_) => "error:unexpected"
                Ok(_) => "error:missing"
              }
              let stream = match client.finite_stream("payload") {
                Err(_) => abort("finite native stream failed to start")
                Ok(invocation) => invocation.collect()
              }
              let middleware = match client.middleware("input") {
                Ok(value) => value
                Err(_) => abort("native middleware invocation failed")
              }
              let stream_bytes = match stream.stdout {
                Ok(Some(bytes)) => bytes
                Ok(None) => abort("finite native stream omitted stdout")
                Err(_) => abort("finite native stdout failed to collect")
              }
              let stream_result = match stream.result {
                Ok(result) => result
                Err(_) => abort("finite native stream result failed")
              }
              [
                "success:" + structured.value + ":" +
                  structured.count.to_string() + ":" +
                  structured.agent_authorized.to_string(),
                supported_error,
                "stream:" + @utf8.decode(stream_bytes) + ":" +
                  stream_result.count.to_string(),
                "middleware:" + middleware,
              ]
            }

            ///|
            #derive.agent
            struct UnauthorizedNativeMoonbitConsumer {
              name : String
            }

            ///|
            fn UnauthorizedNativeMoonbitConsumer::new(
              name : String,
            ) -> UnauthorizedNativeMoonbitConsumer {
              { name, }
            }

            ///|
            pub async fn UnauthorizedNativeMoonbitConsumer::exercise(
              self : Self,
            ) -> String {
              ignore(self.name)
              let client = @native.NativeConformanceClient::new()
              defer client.drop()
              match client.structured("denied", 0UL) {
                Err(@tool.ToolError::Rpc(@tool.RpcError::Denied(message))) => message
                Err(_) => "unexpected-error"
                Ok(_) => "unexpected-success"
              }
            }

            ///|
            fn main {

            }
        "#},
    )
    .unwrap();

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

    let build = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(build.success_or_dump());
    assert!(
        ctx.cwd_path_join(
            "golem-temp/bridge-sdk/moonbit/internal/native-conformance-tool-guest-client/moon.mod.json"
        )
        .is_file()
    );
    assert!(
        !ctx.cwd_path_join("provider").exists(),
        "ambient native client must not select a local provider component"
    );

    let deploy = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deploy.success_or_dump());
    let invoke = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "NativeMoonbitConsumer(\"moonbit\")",
            "exercise",
        ])
        .await;
    assert!(invoke.success_or_dump());
    for expected in [
        "success:alpha:7:true",
        "error:rejected:expected",
        "stream:first:payload|second:2",
        "middleware:leaf(middleware(input))",
    ] {
        assert!(
            invoke.stdout_contains(expected),
            "missing MoonBit native conformance evidence: {expected}"
        );
    }

    let denied = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "UnauthorizedNativeMoonbitConsumer(\"moonbit\")",
            "exercise",
        ])
        .await;
    assert!(denied.success_or_dump());
    assert!(denied.stdout_contains("permission target Tool"));
    assert!(denied.stdout_contains("is not allowed"));
    assert!(!denied.stdout_contains("agent_authorized"));
}
