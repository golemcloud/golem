use crate::app::{cmd, flag, TestContext};
use crate::Tracing;
use golem_cli::fs;
use indoc::indoc;
use std::path::Path;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);

#[test]
async fn test_ts_counter() {
    let mut ctx = TestContext::new();
    let app_name = "counter";

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter"])
        .await;
    assert!(outputs.success());

    let uuid = Uuid::new_v4().to_string();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("app:counter/counter-agent(\"{uuid}\")"),
            "increment",
        ])
        .await;
    assert!(outputs.success());

    assert!(outputs.stdout_contains("- 1"));
}

// Invocations on code-first typescript agents, with complex types / functions.
// Each function call is executed via RPC, and at every stage, mostly return type is same as input type.
// Early in the code-first release, some of these cases failed at the Golem execution stage
// (post type extraction). This test ensures such issues are caught automatically
// and act as a regression-test.
#[test]
async fn test_ts_code_first_complex() {
    let mut ctx = TestContext::new();

    let app_name = "ts-code-first";

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;

    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "ts:agent"]).await;

    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("golem.yaml"),
    );

    let component_source_code_main_file = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("src")
            .join("main.ts"),
    );

    let component_source_code_model_file = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("src")
            .join("model.ts"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              ts:agent:
                template: ts
        "# },
    )
    .unwrap();

    fs::copy(
        "test-data/ts-code-first-snippets/main.ts",
        &component_source_code_main_file,
    )
    .unwrap();

    fs::copy(
        "test-data/ts-code-first-snippets/model.ts",
        &component_source_code_model_file,
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    async fn run_and_assert(ctx: &TestContext, func: &str, args: &[&str]) {
        let uuid = Uuid::new_v4().to_string();

        let agent_constructor = format!("ts:agent/foo-agent(\"{uuid}\")");

        let mut cmd = vec![flag::YES, cmd::AGENT, cmd::INVOKE, &agent_constructor, func];
        cmd.extend_from_slice(args);

        let outputs = ctx.cli(cmd).await;
        assert!(outputs.success(), "function {func} failed");
    }

    // fun with void return
    run_and_assert(&ctx, "fun-void-return", &["\"sample\""]).await;

    // fun without return type
    run_and_assert(&ctx, "fun-no-return", &["\"sample\""]).await;

    // function optional (that has null, defined as union)
    run_and_assert(
        &ctx,
        "fun-optional",
        &[
            "some(case1(\"foo\"))",
            "{a: some(\"foo\")}",
            "{a: some(case1(\"foo\"))}",
            "{a: some(case1(\"foo\"))}",
            "{a: some(\"foo\")}",
            "some(\"foo\")",
            "some(case3(\"foo\"))",
        ],
    )
    .await;

    // function with a simple object
    run_and_assert(&ctx, "fun-object-type", &[r#"{a: "foo", b: 42, c: true}"#]).await;

    // function with a very complex object
    let argument = r#"
      {a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: union-type1("foo"), f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ("foo", 42, true), i: ("foo", 42, {a: "foo", b: 42, c: true}), j: [("foo", 42), ("foo", 42), ("foo", 42)], k: {n: 42}}
    "#;

    run_and_assert(&ctx, "fun-object-complex-type", &[argument]).await;

    // union type that has anonymous terms
    run_and_assert(&ctx, "fun-union-type", &[r#"union-type1("foo")"#]).await;

    // A complex union type
    run_and_assert(
        &ctx,
        "fun-union-complex-type",
        &[r#"union-complex-type1("foo")"#],
    )
    .await;

    // Union that includes literals and boolean (string literal input)
    run_and_assert(&ctx, "fun-union-with-literals", &[r#"lit1"#]).await;

    // Union that includes literals and boolean (boolean input)
    run_and_assert(
        &ctx,
        "fun-union-with-literals",
        &[r#"union-with-literals1(true)"#],
    )
    .await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["foo"]).await;

    // Unstructured text type
    run_and_assert(&ctx, "fun-unstructured-text", &["url(\"foo\")"]).await;

    // Unstructured binary
    run_and_assert(&ctx, "fun-unstructured-binary", &["url(\"foo\")"]).await;

    // Multimodal
    run_and_assert(&ctx, "fun-multimodal", &["[input-text({val: \"foo\"})]"]).await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["bar"]).await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["baz"]).await;

    // A number type
    run_and_assert(&ctx, "fun-number", &["42"]).await;

    // A string type
    run_and_assert(&ctx, "fun-string", &[r#""foo""#]).await;

    // A boolean type
    run_and_assert(&ctx, "fun-boolean", &["true"]).await;

    // A map type
    run_and_assert(&ctx, "fun-map", &[r#"[("foo", 42), ("bar", 42)]"#]).await;

    assert!(outputs.success());

    // A tagged union
    run_and_assert(&ctx, "fun-tagged-union", &[r#"a("foo")"#]).await;

    assert!(outputs.success());

    // A simple tuple type
    run_and_assert(&ctx, "fun-tuple-type", &[r#"("foo", 42, true)"#]).await;

    // A complex tuple type
    run_and_assert(
        &ctx,
        "fun-tuple-complex-type",
        &[r#"("foo", 42, {a: "foo", b: 42, c: true})"#],
    )
    .await;

    // A list complex type
    run_and_assert(
        &ctx,
        "fun-list-complex-type",
        &[r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#],
    ).await;

    // A function with null return
    run_and_assert(&ctx, "fun-null-return", &[r#""foo""#]).await;

    // A function with undefined return
    run_and_assert(&ctx, "fun-undefined-return", &[r#""foo""#]).await;

    // A function with result type
    run_and_assert(&ctx, "fun-result-exact", &[r#"ok("foo")"#]).await;

    // A function with (untagged) result-like type - but not result
    run_and_assert(
        &ctx,
        "fun-either-optional",
        &[r#"{ok: some("foo"), err: none}"#],
    )
    .await;

    // Functions using the builtin result type
    run_and_assert(&ctx, "fun-builtin-result-vs", &[r#"some("yay")"#]).await;
    run_and_assert(&ctx, "fun-builtin-result-vs", &[r#"none"#]).await;

    run_and_assert(&ctx, "fun-builtin-result-sv", &[r#"none"#]).await;
    run_and_assert(&ctx, "fun-builtin-result-sv", &[r#"some("yay")"#]).await;

    run_and_assert(&ctx, "fun-builtin-result-sn", &[r#"case1("yay")"#]).await;
    run_and_assert(&ctx, "fun-builtin-result-sn", &[r#"case2(42)"#]).await;

    // TODO: fix root cause for this
    // An arrow function
    // run_and_assert(&ctx, "fun-arrow-sync", &[r#""foo""#]).await;

    // A function that takes many inputs
    run_and_assert(
        &ctx,
        "fun-all",
        &[
            r#"{a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: union-type1("foo"), f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ("foo", 42, true), i: ("foo", 42, {a: "foo", b: 42, c: true}), j: [("foo", 42), ("foo", 42), ("foo", 42)], k: {n: 42}}"#,
            r#"union-type1("foo")"#,
            r#"union-complex-type1("foo")"#,
            r#"42"#,
            r#""foo""#,
            r#"true"#,
            r#"[("foo", 42), ("foo", 42), ("foo", 42)]"#,
            r#"("foo", 42, {a: "foo", b: 42, c: true})"#,
            r#"("foo", 42, true)"#,
            r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#,
            r#"{a: "foo", b: 42, c: true}"#,
            r#"okay("foo")"#,
            r#"{ok: some("foo"), err: some("foo")}"#,
            r#"some(case1("foo"))"#,
            r#"{a: some("foo")}"#,
            r#"{a: some(case1("foo"))}"#,
            r#"{a: some(case1("foo"))}"#,
            r#"{a: some("foo")}"#,
            r#"some("foo")"#,
            r#"some(case3("foo"))"#,
            r#"a("foo")"#
        ],
    )
        .await;
}

#[test]
async fn test_common_dep_plugs_errors() {
    let mut ctx = TestContext::new();
    let app_name = "common_dep_plug_errors";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:weather-agent"])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("golem.yaml"),
    );
    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts

            dependencies:
              app:weather-agent:
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_brave.wasm
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_google.wasm
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_serper.wasm
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_tavily.wasm
        "# },
    )
        .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(!outputs.success());
    assert2::assert!(outputs.stderr_contains_ordered(
        [
            "error: an error occurred when building the composition graph: multiple plugs found for export golem:web-search/types@1.0.0, only use one of them:",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_brave.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_google.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_serper.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_tavily.wasm",
        ]
    ));

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts

            dependencies:
              app:weather-agent:
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_brave.wasm
        "# },
    )
        .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts
        "# },
    )
    .unwrap();

    fs::write_str(
        &component_source_code,
        indoc! { r#"
            import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';
            import * as websearch from 'golem:web-search/web-search@1.0.0';

            @agent()
            class Agent extends BaseAgent {
              async search(query: string): Promise<string> {
                let result = websearch.searchOnce({
                  query: query,
                });

                console.log(result);

                return "ok";
              }
            }
        "# },
    )
    .unwrap();

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "agent()",
            "search",
            "query",
        ])
        .await;
    assert!(!outputs.success());
    // If this fails, then adjust format_stack_line to match it
    assert!(outputs.stderr_contains("Library golem:web-search/web-search@1.0.0 called without being linked with an implementation"))
}

#[test]
async fn test_component_env_var_substitution() {
    let mut ctx = TestContext::new();
    let app_name = "env_var_substitution";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:weather-agent"])
        .await;
    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("golem.yaml"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts
                env:
                  NORMAL: 'REALLY'
                  VERY_CUSTOM_ENV_VAR_SECRET_1: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}'
                  VERY_CUSTOM_ENV_VAR_SECRET_2: '{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
                  COMPOSED: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
        "# },
    )
    .unwrap();

    ctx.start_server();

    // Building is okay, as that does not resolve env vars
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    // But deploying will do so, so it should fail
    let outputs = ctx.cli([flag::SHOW_SENSITIVE, cmd::APP, cmd::DEPLOY]).await;
    assert!(!outputs.success());

    assert!(outputs.stderr_contains_ordered([
        "key:       COMPOSED",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_1",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_2",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "Failed to prepare environment variables for component: app:weather-agent",
    ]));

    // After providing the missing env vars, deploy should work
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_1", "123");
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_3", "456");

    let outputs = ctx.cli([flag::SHOW_SENSITIVE, cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    assert!(outputs.stdout_contains_ordered([
        "COMPOSED=123-456",
        "NORMAL=REALLY",
        "VERY_CUSTOM_ENV_VAR_SECRET_1=123",
        "VERY_CUSTOM_ENV_VAR_SECRET_2=456",
    ]));
}

#[test]
async fn test_http_api_merging() {
    let mut ctx = TestContext::new();
    let app_name = "http_api_merging";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter1"])
        .await;
    assert!(outputs.success());
    let component1_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-counter1")
            .join("golem.yaml"),
    );

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter2"])
        .await;
    assert!(outputs.success());
    let component2_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-counter2")
            .join("golem.yaml"),
    );

    // Add mergeable definitions and deployments to both components
    fs::write_str(
        &component1_manifest_path,
        indoc! { r#"
            components:
              app:counter1:
                template: ts

            httpApi:
              definitions:
                def-a:
                  version: 0.0.1
                  routes:
                  - method: GET
                    path: /a
                    binding:
                      componentName: app:counter1
                      response: |
                        let agent = counter-agent("a");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-a
        "# },
    )
    .unwrap();

    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter2:
                template: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter2
                      response: |
                        let agent = counter-agent("b");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-b
        "# },
    )
    .unwrap();

    // Check that the merged manifest is loadable
    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(!outputs.stdout_contains("error"));
    assert!(!outputs.stderr_contains("error"));
    assert!(outputs.stderr_contains_ordered([
        "Application API definitions:",
        "  def-a@0.0.1",
        "  def-b@0.0.2",
        "Application API deployments for profile local:",
        "  localhost:9006",
        "    def-a",
        "    def-b",
    ]));

    // But we still cannot define the same deployment <-> definition in two places:
    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter:
                template: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter
                      response: |
                        let agent = counter-agent("b");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-b
                  - def-a
        "# },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains(
        "error: HTTP API Deployment local - localhost:9006 - def-a is defined in multiple sources"
    ));

    // Let's switch back to the good config and deploy, then call the exposed APIs
    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter2:
                template: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter2
                      response: |
                        let agent = counter-agent("b");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-b
        "# },
    )
    .unwrap();

    ctx.start_server();

    let outputs = ctx
        .cli([cmd::APP, cmd::DEPLOY, flag::REDEPLOY_ALL, flag::YES])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "API def-a/0.0.1 deployed at localhost:9006",
        "API def-b/0.0.2 deployed at localhost:9006"
    ]));
}

#[test]
async fn test_invoke_and_repl_agent_id_casing_and_normalizing() {
    let mut ctx = TestContext::new();
    let app_name = "common_dep_plug_errors";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "app:agent"]).await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::write_str(
        &component_source_code,
        indoc! { r#"
            import { BaseAgent, agent, } from '@golemcloud/golem-ts-sdk';

            type Complex = {
              oneField: string;
              anotherField: number;
            }

            @agent()
            class LongAgentName extends BaseAgent {
              params: Complex;
              constructor(params: Complex) {
                super();
                this.params = params;
              }

              async ask(question: Complex): Promise<[Complex, Complex]> {
                return [this.params, question];
              }
            }
        "# },
    )
    .unwrap();

    ctx.start_server();

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            flag::YES,
            flag::REDEPLOY_ALL,
            r#"long-agent-name({one-field: "1212", another-field: 100})"#,
            "ask",
            r#"{one-field: "1", another-field: 2}"#,
        ])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        r#"long-agent-name({one-field:"1212",another-field:100})"#,
        r#"({one-field: "1212", another-field: 100}, {one-field: "1", another-field: 2})"#,
    ]));

    let outputs = ctx
        .cli([
            cmd::REPL,
            flag::FORMAT,
            "json",
            flag::SCRIPT,
            r#"
                let x = long-agent-name({one-field: "1212", another-field: 100});
                x.ask({one-field: "1", another-field: 2})
            "#,
        ])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains(
        r#"{"another-field":100.0,"one-field":"1212"},{"another-field":2.0,"one-field":"1"}"#
    ));
}

#[test]
async fn test_naming_extremes() {
    let mut ctx = TestContext::new();
    let app_name = "test_naming_extremes";

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "app:agent"]).await;
    assert!(outputs.success());

    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::copy(
        "test-data/ts-code-first-snippets/naming_extremes.ts",
        &component_source_code,
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            flag::YES,
            r#"test-agent("x")"#,
            "test-all",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::GET,
            &format!("string-agent(    \"{}\"    )", " ".repeat(447)), // HTTP API should normalize it
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::GET,
            &format!(
                "struct-agent(  {{x:\"{}\"  ,  y    : \"{}\", z: \"{}\" }})", // HTTP API should normalize it
                " ".repeat(102),
                " ".repeat(102),
                "/".repeat(102)
            ),
        ])
        .await;
    assert!(outputs.success());
}
