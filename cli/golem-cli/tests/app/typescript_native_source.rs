use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use test_r::{test, timeout};

#[test]
#[timeout("15 minutes")]
async fn typescript_ambient_native_client_covers_contract_without_local_provider() {
    let mut ctx = TestContext::new();
    ctx.enable_native_conformance_tool();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("typescript-native-source")).unwrap();
    ctx.cd("typescript-native-source");
    for component in ["consumer", "middleware"] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "ts",
                flag::COMPONENT_NAME,
                &format!("typescript-native-source:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: typescript-native-source
        environments:
          local:
            server: local
            componentPresets: quick
            tools:
              middleware: [typescript-native-rewrite]
        components:
          typescript-native-source:consumer:
            dir: consumer
            templates: ts
            dependencies:
              tools: [native-conformance]
          typescript-native-source:middleware:
            dir: middleware
            templates: ts
        tools:
          middleware:
            typescript-native-rewrite:
              component: typescript-native-source:middleware
        agents:
          TypeScriptNativeConsumer:
            tools:
              native-conformance: {{}}
          UnauthorizedTypeScriptNativeConsumer:
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
          ts:
            internal:
              tools: [native-conformance]
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    add_typescript_tool_client_source(
        &ctx,
        "native-conformance-tool-guest-client",
        "native-conformance-tool-guest-client.ts",
    );
    fs::write_str(
        ctx.cwd_path_join("consumer/src/counter-agent.ts"),
        indoc! {r#"
        import { z } from 'zod';
        import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
        import { NativeConformanceClient } from 'native-conformance-tool-guest-client';

        const TypeScriptNativeConsumer = defineAgent({
          name: 'TypeScriptNativeConsumer',
          id: { name: z.string() },
          methods: { run: method({ input: {}, returns: z.array(z.string()) }) },
        });

        TypeScriptNativeConsumer.implement({
          init: () => ({}),
          methods: {
            async run() {
              const client = NativeConformanceClient.newClient();
              const structured = await client.structured('alpha', 7n);
              let supportedError = 'unexpected-success';
              try {
                await client.supported_error('expected');
              } catch (error) {
                const failure = error as {
                  tag?: string;
                  error?: { tag?: string; value?: string };
                };
                if (failure.tag !== 'tool' || failure.error?.tag !== 'Rejected') throw error;
                supportedError = `error:rejected:${failure.error.value}`;
              }
              const streaming = client.finite_stream('payload');
              if (streaming.stdout === undefined) throw new Error('finite-stream omitted stdout');
              const collected = await streaming.collect();
              if (collected.stdout.status === 'rejected') throw collected.stdout.reason;
              if (collected.stdout.value === undefined) throw new Error('finite-stream omitted collected stdout');
              if (collected.result.status === 'rejected') throw collected.result.reason;
              const stdout = new TextDecoder().decode(collected.stdout.value);
              const streamResult = collected.result.value;
              const middleware = await client.middleware('input');
              return [
                `success:${structured.value}:${structured.count}:${structured.agentAuthorized}`,
                supportedError,
                `stream:${stdout}:${streamResult.count}:${streamResult.agentAuthorized}`,
                `middleware:${middleware}`,
              ];
            },
          },
        });

        const UnauthorizedTypeScriptNativeConsumer = defineAgent({
          name: 'UnauthorizedTypeScriptNativeConsumer',
          id: { name: z.string() },
          methods: { run: method({ input: {}, returns: z.string() }) },
        });

        UnauthorizedTypeScriptNativeConsumer.implement({
          init: () => ({}),
          methods: {
            async run() {
              try {
                await NativeConformanceClient.newClient().structured('denied', 0n);
                return 'unexpected-success';
              } catch (error) {
                return JSON.stringify(error, (_key, value) =>
                  typeof value === 'bigint' ? value.toString() : value,
                );
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
          name: 'typescript-native-rewrite',
          invoke: (request, { underlying }) => underlying.invokeAndAwait(
            request.commandPath,
            {
              ...request.input,
              value: {
                ...request.input.value,
                valueNodes: request.input.value.valueNodes.map(node =>
                  node.tag === 'string-value' && node.val === 'input'
                    ? { ...node, val: 'middleware(input)' }
                    : node,
                ),
              },
            },
            request.stdin,
          ),
        });
    "#},
    )
    .unwrap();

    let built = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(built.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/ts/internal/native-conformance-tool-guest-client/native-conformance-tool-guest-client.ts")
            .is_file()
    );
    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "TypeScriptNativeConsumer(\"source-native\")",
            "run",
        ])
        .await;
    assert!(output.success_or_dump());
    for expected in [
        "success:alpha:7:true",
        "error:rejected:expected",
        "stream:first:payload|second:2:true",
        "middleware:leaf(middleware(input))",
    ] {
        assert!(output.stdout_contains(expected), "missing {expected}");
    }

    let denied = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "UnauthorizedTypeScriptNativeConsumer(\"source-native\")",
            "run",
        ])
        .await;
    assert!(denied.success_or_dump());
    assert!(denied.stdout_contains("denied"));
    assert!(denied.stdout_contains("permission target Tool"));
    assert!(denied.stdout_contains("is not allowed"));
    assert!(!denied.stdout_contains("agentAuthorized"));
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
