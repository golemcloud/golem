use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{test, timeout};

#[test]
#[timeout("20 minutes")]
async fn go_ambient_native_client_executes_through_golem() {
    let mut ctx = TestContext::new();
    ctx.enable_native_conformance_tool();
    ctx.start_server().await;

    fs::create_dir_all(ctx.cwd_path_join("go-native-client")).unwrap();
    ctx.cd("go-native-client");
    for (template, component) in [("go", "consumer"), ("rust", "middleware")] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                &format!("go-native-client:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {version}
            app: go-native-client
            environments:
              local:
                server: local
                componentPresets: debug
                tools:
                  middleware: [native-conformance-audit]
            components:
              go-native-client:consumer:
                dir: consumer
                templates: go
                dependencies:
                  tools: [native-conformance]
              go-native-client:middleware:
                dir: middleware
                templates: rust
            tools:
              middleware:
                native-conformance-audit:
                  component: go-native-client:middleware
            agents:
              NativeGoConsumer:
                tools:
                  native-conformance: {{}}
              UnauthorizedNativeGoConsumer:
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
              go:
                internal:
                  tools: [native-conformance]
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    let consumer = ctx.cwd_path_join("consumer");
    let module = fs::read_to_string(consumer.join("go.mod"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the consumer's go.mod names its module");
    fs::remove(consumer.join("agents")).unwrap();
    fs::write_str(
        consumer.join("native/native.go"),
        include_str!("go_native_tool_consumer.go"),
    )
    .unwrap();
    fs::write_str(
        consumer.join("main.go"),
        format!("package main\n\nimport _ \"{module}/native\"\n\nfunc main() {{}}\n"),
    )
    .unwrap();
    let go_mod = consumer.join("go.mod");
    fs::write_str(
        &go_mod,
        fs::read_to_string(&go_mod).unwrap()
            + indoc! {r#"

                require golem.local/bridge/native-conformance-tool-guest-client v0.0.0

                replace golem.local/bridge/native-conformance-tool-guest-client => ../golem-temp/bridge-sdk/go/internal/native-conformance-tool-guest-client
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
            "golem-temp/bridge-sdk/go/internal/native-conformance-tool-guest-client/go.mod"
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
            "NativeGoConsumer(\"go\")",
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
            "missing Go native conformance evidence: {expected}"
        );
    }

    let denied = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "UnauthorizedNativeGoConsumer(\"go\")",
            "exercise",
        ])
        .await;
    assert!(denied.success_or_dump());
    assert!(denied.stdout_contains("permission target Tool"));
    assert!(denied.stdout_contains("is not allowed"));
    assert!(!denied.stdout_contains("agent_authorized"));
}
