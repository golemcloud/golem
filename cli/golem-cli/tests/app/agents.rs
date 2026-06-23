use crate::app::{TestContext, cmd, flag, merge_into_manifest};
use crate::crate_path;
use std::path::PathBuf;

fn test_data_path() -> PathBuf {
    crate_path().join("test-data")
}
use crate::Tracing;
use anyhow::Context;

use goldenfile::Mint;
use golem_cli::fs;
use golem_cli::model::GuestLanguage;
use golem_cli::versions;
use indoc::{formatdoc, indoc};
use std::io::Write;
use std::path::Path;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

#[test]
async fn test_rust_counter() {
    let mut ctx = TestContext::new();
    let app_name = "counter";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    // Test with CLI invoke
    {
        let uuid = Uuid::new_v4().to_string();
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                &format!("CounterAgent(\"{uuid}\")"),
                "increment",
            ])
            .await;
        assert!(outputs.success_or_dump());
        assert!(!outputs.stdout_contains("error"));
        assert!(!outputs.stderr_contains("error"));
        assert!(outputs.stdout_contains_ordered(["Invocation result in Rust syntax:", "1"]));
    }

    // Test with TS REPL
    {
        let uuid = Uuid::new_v4().to_string();
        let outputs = ctx
            .cli([
                cmd::REPL,
                flag::LANGUAGE,
                "ts",
                flag::SCRIPT,
                &format!("(await CounterAgent.get(\"{uuid}\")).increment()"),
            ])
            .await;
        assert!(outputs.success_or_dump());
        assert!(outputs.stdout_contains_ordered(vec!["Preparing TypeScript REPL", "1"]));
        assert!(outputs.stderr_contains_ordered(vec!["> awaiting Promise<number>"]));
        assert!(!outputs.stdout_contains("error"));
        assert!(!outputs.stderr_contains("error"));
    }
}

/// End-to-end test for the Scala bridge generator: deploys the Rust counter
/// agent, generates a Scala bridge SDK for it, then compiles and runs a small
/// Scala program that invokes the live agent through the generated, future-based
/// client and verifies the returned values.
///
/// Requires `sbt` on the PATH (same as the Scala bridge cross-compile tests).
#[test]
#[timeout("15 minutes")]
async fn test_scala_bridge_e2e() {
    let mut ctx = TestContext::new();
    let app_name = "counter";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    // Generate the Scala bridge SDK for the counter agent into a known directory.
    let bridge_root = ctx.cwd_path_join("scala-bridge");
    let bridge_root_str = bridge_root.to_str().unwrap().to_string();
    let outputs = ctx
        .cli([
            cmd::GENERATE_BRIDGE,
            flag::LANGUAGE,
            "scala",
            flag::AGENT_TYPE_NAME,
            "CounterAgent",
            flag::OUTPUT_DIR,
            &bridge_root_str,
        ])
        .await;
    assert!(outputs.success_or_dump());

    let client_dir = bridge_root.join("counter-agent-client");
    assert!(
        client_dir.join("build.sbt").exists(),
        "generated Scala bridge project is missing at {}",
        client_dir.display()
    );

    // Write a small Scala program that drives the generated client against the
    // live local server, mirroring the TS REPL e2e check above.
    let server_url = ctx.worker_service_url();
    let token = golem_client::LOCAL_WELL_KNOWN_TOKEN;
    let main_scala = formatdoc! {r#"
        import golem.bridge.client.CounterAgentClient
        import golem.bridge.runtime.GolemServer

        import scala.concurrent.Await
        import scala.concurrent.duration._

        object Main {{
          def main(args: Array[String]): Unit = {{
            CounterAgentClient.configure(
              GolemServer.Custom("{server_url}", "{token}"),
              "{app_name}",
              "local"
            )
            val timeout = 60.seconds
            val remote  = Await.result(CounterAgentClient.get("scala-e2e-counter"), timeout)
            val first   = Await.result(remote.increment(), timeout)
            val second  = Await.result(remote.increment(), timeout)
            if (first.value != 1L || second.value != 2L) {{
              sys.error(s"Unexpected counter values: first=${{first.value}} second=${{second.value}}")
            }}
            println("SCALA_BRIDGE_E2E_OK first=" + first.value + " second=" + second.value)
          }}
        }}
        "#
    };
    let scala_main_dir = client_dir.join("src").join("main").join("scala");
    std::fs::create_dir_all(&scala_main_dir).unwrap();
    std::fs::write(scala_main_dir.join("Main.scala"), main_scala).unwrap();

    // Compile and run the generated client + driver with sbt.
    let output = std::process::Command::new("sbt")
        .arg("--batch")
        .arg("runMain Main")
        .current_dir(&client_dir)
        .output()
        .expect("failed to run sbt; is it installed?");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sbt run failed in {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        client_dir.display()
    );
    assert!(
        stdout.contains("SCALA_BRIDGE_E2E_OK first=1 second=2"),
        "Scala bridge e2e program did not produce the expected output.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

#[test]
async fn test_rust_code_first_with_rpc_and_all_types() {
    let mut ctx = TestContext::new();

    let app_name = "rust-code-first";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;

    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let component_manifest_path = ctx.cwd_path_join("golem.yaml");

    let component_source_code_lib_file = ctx.cwd_path_join("src/lib.rs");

    let component_source_code_model_file = ctx.cwd_path_join("src/model.rs");

    fs::write_str(
        &component_manifest_path,
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-code-first

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            components:
              rust-code-first:rust-main:
                templates: rust

            # We also test that we can generate the bridge SDKs during the build process
            bridge:
              ts:
                agents: "*"
              rust:
                agents: "*"
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("rust-code-first-snippets/lib.rs"),
        &component_source_code_lib_file,
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("rust-code-first-snippets/model.rs"),
        &component_source_code_model_file,
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    check_agent_types_golden_file(ctx.cwd_path(), GuestLanguage::Rust).unwrap();

    async fn run_and_assert(ctx: &TestContext, func: &str, args: &[&str]) {
        let uuid = Uuid::new_v4().to_string();

        let agent_constructor = format!("FooAgent(Some(\"{uuid}\"))");

        let mut cmd = vec![flag::YES, cmd::AGENT, cmd::INVOKE, &agent_constructor, func];
        cmd.extend_from_slice(args);

        let outputs = ctx.cli(cmd).await;
        assert!(outputs.success_or_dump(), "function {func} failed");
    }

    run_and_assert(&ctx, "get_id", &[]).await;

    run_and_assert(&ctx, "fun_string", &["\"sample\""]).await;

    // A char type
    run_and_assert(&ctx, "fun_char", &[r#"'a'"#]).await;

    // Testing trigger invocation
    run_and_assert(&ctx, "fun_string_fire_and_forget", &["\"sample\""]).await;

    // Testing scheduled invocation
    run_and_assert(&ctx, "fun_string_later", &["\"sample\""]).await;

    run_and_assert(&ctx, "fun_u8", &["42"]).await;

    run_and_assert(&ctx, "fun_i8", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun_u16", &["42"]).await;

    run_and_assert(&ctx, "fun_i16", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun_i32", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun_u32", &["42"]).await;

    run_and_assert(&ctx, "fun_u64", &["42"]).await;

    run_and_assert(&ctx, "fun_i64", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun_f32", &["3.14"]).await;

    run_and_assert(&ctx, "fun_f64", &["3.1415926535"]).await;

    run_and_assert(&ctx, "fun_boolean", &["true"]).await;

    let all_primitives_arg = r#"
    {
        u8v: 42,
        u16v: 42,
        u32v: 42,
        u64v: 42,
        i8v: -42,
        i16v: -42,
        i32v: -42,
        i64v: -42,
        f32v: 3.14,
        f64v: 3.1415926535,
        boolv: true,
        charv: 'a',
        stringv: "sample"
    }
    "#;

    run_and_assert(&ctx, "fun_all_primitives", &[all_primitives_arg]).await;

    run_and_assert(&ctx, "fun_tuple_simple", &[r#"("sample", 3.14, true)"#]).await;

    run_and_assert(
        &ctx,
        "fun_tuple_complex",
        &[&format!("(\"sample\", 3.14, {all_primitives_arg}, true)")],
    )
    .await;

    run_and_assert(
        &ctx,
        "fun_map",
        &[r#"{"foo" => 1, "bar" => 2, "baz" => 3}"#],
    )
    .await;

    let collections_arg = r#"
    {
        list_u8: [1, 2, 3, 4, 5],
        list_str: ["foo", "bar", "baz"],
        map_num: {"pi" => 3.14, "e" => 2.71, "phi" => 1.61},
        map_text: {1 => "one", 2 => "two", 3 => "three"}
    }
    "#;

    run_and_assert(&ctx, "fun_collections", &[collections_arg]).await;

    let simple_struct_arg = r#"
    {
        name: "test",
        value: 3.14,
        flag: true,
        symbol: 't',
    }
    "#;

    run_and_assert(&ctx, "fun_struct_simple", &[simple_struct_arg]).await;

    let nested_struct_arg = r#"
    {
        id: "nested1",
        simple: {
            name: "inner",
            value: 2.71,
            flag: false,
            symbol: 'i',
        },
        list: [
            {
                name: "list1",
                value: 1.61,
                flag: true,
                symbol: 'l',
            },
            {
                name: "list2",
                value: 0.577,
                flag: false,
                symbol: 'm',
            }
        ],
        map: {"a" => 1, "b" => 2},
        option: Some("optional value"),
        result: Ok("result value")
    }
    "#;

    run_and_assert(&ctx, "fun_struct_nested", &[nested_struct_arg]).await;

    let complex_struct_arg = r#"
    {
        primitives: {
            u8v: 1,
            u16v: 2,
            u32v: 3,
            u64v: 4,
            i8v: -1,
            i16v: -2,
            i32v: -3,
            i64v: -4,
            f32v: 1.1,
            f64v: 2.2,
            boolv: true,
            charv: 'c',
            stringv: "complex"
        },
        options_results_bounds: {
            option_u8: Some(128),
            option_str: Some("option value"),
            res_ok: Ok("success"),
            res_num_err: Err("number error"),
            res_unit_ok: Ok("b"),
            res_unit_err: Err("a"),
            bound_u8: Included(1),
            bound_str: Excluded("z")
        },
        tuples: {
            pair: ("pair", 2.22),
            triple: ("triple", 3.33, true),
            mixed: ( -8, 16, 4.4)
        },
        collections: {
            list_u8: [10, 20, 30],
            list_str: ["x", "y", "z"],
            map_num: {"a" => 1.11, "b" => 2.22, "c" => 3.33},
            map_text: {100 => "hundred", 200 => "two hundred", 300 => "three hundred"}
        },
        simple_struct: {
            name: "comp_simple",
            value: 5.55,
            flag: false,
            symbol: 's',
        },
        nested_struct: {
            id: "comp_nested",
            simple: {
                name: "comp_inner",
                value: 6.66,
                flag: true,
                symbol: 'i',
            },
            list: [],
            map: {},
            option: None,
            result: Ok("nested result")
        },
        enum_simple: U8(100),
        enum_collections: Vec([1, 2, 3]),
        enum_complex: UnitA
    }
    "#;

    run_and_assert(&ctx, "fun_struct_complex", &[complex_struct_arg]).await;

    run_and_assert(&ctx, "fun_simple_enum", &["I64(-12345)"]).await;

    run_and_assert(&ctx, "fun_result", &["Ok(\"success\")"]).await;
    run_and_assert(&ctx, "fun_result", &["Err(\"failed\")"]).await;

    run_and_assert(&ctx, "fun_result_unit_ok", &["Ok(())"]).await;

    run_and_assert(&ctx, "fun_result_unit_err", &["Err(())"]).await;

    run_and_assert(&ctx, "fun_result_unit_both", &["Ok(())"]).await;

    let result_complex_arg = r#"
    Ok({
        id: "res_comp",
        simple: {
            name: "res_inner",
            value: 7.77,
            flag: false,
            symbol: 'r',
        },
        list: [],
        map: {},
        option: None,
        result: Ok("result in nested")
    })
    "#;

    run_and_assert(&ctx, "fun_result_complex", &[result_complex_arg]).await;

    run_and_assert(&ctx, "fun_option", &["Some(\"optional value\")"]).await;

    let option_complex_arg = r#"
    Some({
        id: "opt_comp",
        simple: {
            name: "opt_inner",
            value: 8.88,
            flag: true,
            symbol: 'o',
        },
        list: [],
        map: {},
        option: None,
        result: Err("error in nested")
    })
    "#;

    run_and_assert(&ctx, "fun_option_complex", &[option_complex_arg]).await;

    run_and_assert(&ctx, "fun_enum_with_only_literals", &["A"]).await;

    // TODO: Re-enable once CLI WAVE argument parsing supports multimodal/unstructured types
    // run_and_assert(
    //     &ctx,
    //     "fun_multi_modal",
    //     &[r#"[text("foo"), text("foo"), data({id: 1, name: "foo"})]"#],
    // )
    // .await;
    //
    // run_and_assert(
    //     &ctx,
    //     "fun_multi_modal_basic",
    //     &[r#"[text(url("foo"))]"#],
    // )
    // .await;
    //
    // run_and_assert(
    //     &ctx,
    //     "fun_unstructured_text",
    //     &[r#"url("foo")"#],
    // )
    // .await;
    //
    // run_and_assert(
    //     &ctx,
    //     "fun_unstructured_text",
    //     &[r#"inline({data: "foo", text-type: none})"#],
    // )
    // .await;
    //
    // run_and_assert(
    //     &ctx,
    //     "fun_unstructured_text_lc",
    //     &[r#"url("foo")"#],
    // )
    // .await;
    //
    // run_and_assert(
    //     &ctx,
    //     "fun_unstructured_text_lc",
    //     &[r#"inline({data: "foo", text-type: some({language-code: "en"})})"#],
    // )
    // .await;
    //
    // run_and_assert(
    //     &ctx,
    //     "fun_unstructured_binary",
    //     &[r#"url("foo")"#],
    // )
    // .await;
}

#[test]
async fn test_ts_counter() {
    let mut ctx = TestContext::new();
    let app_name = "counter";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    // Test with CLI invoke
    {
        let uuid = Uuid::new_v4().to_string();
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                &format!("CounterAgent(\"{uuid}\")"),
                "increment",
            ])
            .await;
        assert!(outputs.success_or_dump());
        assert!(!outputs.stdout_contains("error"));
        assert!(!outputs.stderr_contains("error"));
        assert!(outputs.stdout_contains_ordered(["Invocation result in TypeScript syntax:", "1"]));
    }

    // Test with TS REPL
    {
        let uuid = Uuid::new_v4().to_string();
        let outputs = ctx
            .cli([
                cmd::REPL,
                flag::LANGUAGE,
                "ts",
                flag::SCRIPT,
                &format!("(await CounterAgent.get(\"{uuid}\")).increment()"),
            ])
            .await;
        assert!(outputs.success_or_dump());
        assert!(outputs.stdout_contains_ordered(vec!["Preparing TypeScript REPL", "1"]));
        assert!(outputs.stderr_contains_ordered(vec!["> awaiting Promise<number>"]));
        assert!(!outputs.stdout_contains("error"));
        assert!(!outputs.stderr_contains("error"));
    }
}

#[test]
async fn test_long_agent_id_rejected_in_invoke_repl_and_rpc() {
    let mut ctx = TestContext::new();
    let app_name = "long-agent-id-rejected";

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let component_manifest_path = ctx.cwd_path_join("golem.yaml");
    let component_source_code_main_file = ctx.cwd_path_join("src/main.ts");

    fs::write_str(
        &component_manifest_path,
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: long-agent-id-rejected

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            components:
              long-agent-id-rejected:ts-main:
                templates: ts
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();

    fs::write_str(
        &component_source_code_main_file,
        indoc! { r#"
            import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';

            @agent()
            class TargetAgent extends BaseAgent {
              id: string;

              constructor(id: string) {
                super();
                this.id = id;
              }

              async ping(): Promise<string> {
                return `pong:${this.id}`;
              }
            }

            @agent()
            class CallerAgent extends BaseAgent {
              id: string;

              constructor(id: string) {
                super();
                this.id = id;
              }

              async callTarget(targetId: string): Promise<string> {
                return await (await TargetAgent.get(targetId)).ping();
              }
            }
        "# },
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let long_id = "x".repeat(5000);
    let target_agent = format!("TargetAgent(\"{long_id}\")");

    let outputs = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, &target_agent, "ping"])
        .await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains("Agent id is too long"));

    let outputs = ctx
        .cli([
            cmd::REPL,
            flag::LANGUAGE,
            "ts",
            flag::SCRIPT,
            &format!("(await TargetAgent.get(\"{long_id}\")).ping()"),
        ])
        .await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains("Agent id is too long"));

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "CallerAgent(\"ok\")",
            "callTarget",
            &format!("\"{long_id}\""),
        ])
        .await;
    assert!(!outputs.success());
    assert!(
        outputs
            .stderr_contains("Agent Service - Error: 500 Internal Server Error, Invocation Failed")
    );
}

// Invocations on code-first typescript agents, with complex types / functions.
// Each function call is executed via RPC, and at every stage, mostly return type is same as input type.
// Early in the code-first release, some of these cases failed at the Golem execution stage
// (post type extraction). This test ensures such issues are caught automatically
// and act as a regression-test.
#[test]
async fn test_ts_code_first_with_rpc_and_all_types() {
    let mut ctx = TestContext::new();

    let app_name = "ts-code-first";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;

    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    assert!(outputs.success_or_dump());

    let component_manifest_path = ctx.cwd_path_join("golem.yaml");

    let component_source_code_main_file = ctx.cwd_path_join("src/main.ts");

    let component_source_code_model_file = ctx.cwd_path_join("src/model.ts");

    fs::write_str(
        &component_manifest_path,
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: ts-code-first

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            components:
              ts-code-first:ts-main:
                templates: ts

            # We also test that we can generate the bridge SDKs during the build process
            bridge:
              ts:
                agents: "*"
              rust:
                agents: "*"
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("ts-code-first-snippets/main.ts"),
        &component_source_code_main_file,
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("ts-code-first-snippets/model.ts"),
        &component_source_code_model_file,
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    check_agent_types_golden_file(ctx.cwd_path(), GuestLanguage::TypeScript).unwrap();

    async fn run_and_assert(ctx: &TestContext, func: &str, args: &[&str]) {
        let uuid = Uuid::new_v4().to_string();

        let agent_constructor = format!("FooAgent(\"{uuid}\")");

        let mut cmd = vec![flag::YES, cmd::AGENT, cmd::INVOKE, &agent_constructor, func];
        cmd.extend_from_slice(args);

        let outputs = ctx.cli(cmd).await;
        assert!(outputs.success_or_dump(), "function {func} failed");
    }

    // fun with void return
    run_and_assert(&ctx, "funVoidReturn", &["\"sample\""]).await;

    // fun without return type
    run_and_assert(&ctx, "funNoReturn", &["\"sample\""]).await;

    // function optional (that has null, defined as union)
    run_and_assert(
        &ctx,
        "funOptional",
        &[
            r#"{tag: "case1", value: "foo"}"#,
            r#"{a: "foo"}"#,
            r#"{a: {tag: "case1", value: "foo"}}"#,
            r#"{a: {tag: "case1", value: "foo"}}"#,
            r#"{a: "foo"}"#,
            r#""foo""#,
            r#"{tag: "UnionType2", value: "foo"}"#,
        ],
    )
    .await;

    run_and_assert(&ctx, "funOptionalQMark", &[r#""x""#, "null", r#""y""#]).await;

    // function with a simple object
    run_and_assert(&ctx, "funObjectType", &[r#"{a: "foo", b: 42, c: true}"#]).await;

    // recursive type (a tree referencing itself) — proves recursion is supported end-to-end
    // over the real REST path (D5)
    run_and_assert(
        &ctx,
        "funRecursive",
        &[
            r#"{label: "root", children: [{label: "a", children: []}, {label: "b", children: [{label: "c", children: []}]}]}"#,
        ],
    )
    .await;

    // function with a very complex object
    let argument = r#"
      {a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: {tag: "UnionType2", value: "foo"}, f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ["foo", 42, true], i: ["foo", 42, {a: "foo", b: 42, c: true}], j: {"foo" => 42, "foo" => 42, "foo" => 42}, k: {n: 42}}
    "#;

    run_and_assert(&ctx, "funObjectComplexType", &[argument]).await;

    // union type that has anonymous terms
    run_and_assert(
        &ctx,
        "funUnionType",
        &[r#"{tag: "UnionType2", value: "foo"}"#],
    )
    .await;

    // A complex union type
    run_and_assert(
        &ctx,
        "funUnionComplexType",
        &[r#"{tag: "UnionComplexType2", value: "foo"}"#],
    )
    .await;

    // Union that includes literals and boolean (string literal input)
    run_and_assert(&ctx, "funUnionWithLiterals", &[r#"{tag: "lit1"}"#]).await;

    // Union that includes literals and boolean (boolean input)
    run_and_assert(
        &ctx,
        "funUnionWithLiterals",
        &[r#"{tag: "UnionWithLiterals1", value: true}"#],
    )
    .await;

    // Union that has only literals
    run_and_assert(&ctx, "funUnionWithOnlyLiterals", &[r#""foo""#]).await;

    // TODO: Re-enable once CLI WAVE argument parsing supports multimodal/unstructured types
    // // Unstructured text type
    // run_and_assert(&ctx, "funUnstructuredText", &["url(\"foo\")"]).await;
    //
    // // Unstructured binary
    // run_and_assert(&ctx, "funUnstructuredBinary", &["url(\"foo\")"]).await;
    //
    // // Multimodal
    // run_and_assert(
    //     &ctx,
    //     "funMultimodal",
    //     &["[text(inline({data: \"data\", text-type: none}))]"],
    // )
    // .await;
    //
    // run_and_assert(
    //     &ctx,
    //     "funMultimodalAdvanced",
    //     &["[text(\"foo\")]"],
    // )
    // .await;

    // Union that has only literals
    run_and_assert(&ctx, "funUnionWithOnlyLiterals", &[r#""bar""#]).await;

    // Union that has only literals
    run_and_assert(&ctx, "funUnionWithOnlyLiterals", &[r#""baz""#]).await;

    // A number type
    run_and_assert(&ctx, "funNumber", &["42"]).await;

    // A string type
    run_and_assert(&ctx, "funString", &[r#""foo""#]).await;

    // A boolean type
    run_and_assert(&ctx, "funBoolean", &["true"]).await;

    // A map type
    run_and_assert(&ctx, "funMap", &[r#"{"foo" => 42, "bar" => 42}"#]).await;

    assert!(outputs.success_or_dump());

    // A tagged union
    run_and_assert(&ctx, "funTaggedUnion", &[r#"{tag: "a", value: "foo"}"#]).await;

    assert!(outputs.success_or_dump());

    // A simple tuple type
    run_and_assert(&ctx, "funTupleType", &[r#"["foo", 42, true]"#]).await;

    // A complex tuple type
    run_and_assert(
        &ctx,
        "funTupleComplexType",
        &[r#"["foo", 42, {a: "foo", b: 42, c: true}]"#],
    )
    .await;

    // A list complex type
    run_and_assert(
        &ctx,
        "funListComplexType",
        &[r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#],
    ).await;

    // A function with null return
    run_and_assert(&ctx, "funNullReturn", &[r#""foo""#]).await;

    // A function with undefined return
    run_and_assert(&ctx, "funUndefinedReturn", &[r#""foo""#]).await;

    // A function with result type
    run_and_assert(&ctx, "funResultExact", &[r#"{ok: "foo"}"#]).await;

    // A function with (untagged) result-like type - but not result
    run_and_assert(&ctx, "funEitherOptional", &[r#"{ok: "foo", err: null}"#]).await;

    // Functions using the builtin result type
    run_and_assert(&ctx, "funBuiltinResultVS", &[r#"{ok: null}"#]).await;
    run_and_assert(&ctx, "funBuiltinResultVS", &[r#"{error: "foo"}"#]).await;

    run_and_assert(&ctx, "funBuiltinResultSV", &[r#"{ok: "foo"}"#]).await;
    run_and_assert(&ctx, "funBuiltinResultSV", &[r#"{error: null}"#]).await;

    run_and_assert(&ctx, "funBuiltinResultSN", &[r#"{ok: "yay"}"#]).await;
    run_and_assert(&ctx, "funBuiltinResultSN", &[r#"{error: 42}"#]).await;

    run_and_assert(&ctx, "funResultLikeWithVoid", &[r#"{error: null}"#]).await;
    run_and_assert(&ctx, "funResultLikeWithVoid", &[r#"{ok: null}"#]).await;

    // An arrow function
    run_and_assert(&ctx, "funArrowSync", &[r#""foo""#]).await;

    // A function that takes many inputs
    run_and_assert(
        &ctx,
        "funAll",
        &[
            r#"{a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: {tag: "UnionType2", value: "foo"}, f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ["foo", 42, true], i: ["foo", 42, {a: "foo", b: 42, c: true}], j: {"foo" => 42, "foo" => 42, "foo" => 42}, k: {n: 42}}"#,
            r#"{tag: "UnionType2", value: "foo"}"#,
            r#"{tag: "UnionComplexType2", value: "foo"}"#,
            r#"42"#,
            r#""foo""#,
            r#"true"#,
            r#"{"foo" => 42, "foo" => 42, "foo" => 42}"#,
            r#"["foo", 42, {a: "foo", b: 42, c: true}]"#,
            r#"["foo", 42, true]"#,
            r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#,
            r#"{a: "foo", b: 42, c: true}"#,
            r#"{tag: "okay", value: "foo"}"#,
            r#"{ok: "foo", err: "foo"}"#,
            r#"{tag: "case1", value: "foo"}"#,
            r#"{a: "foo"}"#,
            r#"{a: {tag: "case1", value: "foo"}}"#,
            r#"{a: {tag: "case1", value: "foo"}}"#,
            r#"{a: "foo"}"#,
            r#""foo""#,
            r#"{tag: "UnionType2", value: "foo"}"#,
            r#"{tag: "a", value: "foo"}"#
        ],
    )
        .await;
}

#[test]
async fn test_component_env_var_substitution() {
    let mut ctx = TestContext::new();
    let app_name = "env-var-substitution";

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let component_manifest_path = ctx.cwd_path_join("golem.yaml");

    merge_into_manifest(
        &component_manifest_path,
        indoc! { r#"
            components:
              env-var-substitution:ts-main:
                templates: ts
                env:
                  NORMAL: 'REALLY'
                  VERY_CUSTOM_ENV_VAR_SECRET_1: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}'
                  VERY_CUSTOM_ENV_VAR_SECRET_2: '{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
                  COMPOSED: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
        "# },
    )
    .unwrap();

    ctx.start_server().await;

    // Building is okay, as that does not resolve env vars
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    // But deploying will do so, so it should fail
    let outputs = ctx
        .cli([flag::SHOW_SENSITIVE, cmd::DEPLOY, flag::YES])
        .await;
    assert!(!outputs.success());

    assert!(outputs.stdout_contains_ordered([
        "key:       COMPOSED",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "Failed to substitute environment variable(s) (VERY_CUSTOM_ENV_VAR_SECRET_1, VERY_CUSTOM_ENV_VAR_SECRET_3) for COMPOSED",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_1",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}",
        "Failed to substitute environment variable(s) (VERY_CUSTOM_ENV_VAR_SECRET_1) for VERY_CUSTOM_ENV_VAR_SECRET_1",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_2",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "Failed to substitute environment variable(s) (VERY_CUSTOM_ENV_VAR_SECRET_3) for VERY_CUSTOM_ENV_VAR_SECRET_2"
    ]));

    // After providing the missing env vars, deploy should work
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_1", "123");
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_3", "456");

    let outputs = ctx
        .cli([flag::SHOW_SENSITIVE, cmd::DEPLOY, flag::YES])
        .await;
    assert!(outputs.success_or_dump());

    assert!(outputs.stdout_contains_ordered([
        "COMPOSED: 123-456",
        "NORMAL: REALLY",
        "VERY_CUSTOM_ENV_VAR_SECRET_1: '123'",
        "VERY_CUSTOM_ENV_VAR_SECRET_2: '456'",
    ]));
}

#[test]
async fn test_invoke_and_repl_agent_id_casing_and_normalizing() {
    let mut ctx = TestContext::new();
    let app_name = "agent-id-casing-and-normalizing";

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "ts",
            flag::COMPONENT_NAME,
            "app:agent",
        ])
        .await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    let component_golem_yaml = ctx.cwd_path_join(Path::new("app-agent").join("golem.yaml"));
    fs::write_str(
        &component_golem_yaml,
        indoc! { r#"
          components:
            app:agent:
              templates: ts
        "#},
    )
    .unwrap();

    let component_source_code =
        ctx.cwd_path_join(Path::new("app-agent").join("src").join("main.ts"));

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

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            flag::YES,
            r#"LongAgentName({oneField: "1212", anotherField: 100})"#,
            "ask",
            r#"{oneField: "1", anotherField: 2}"#,
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_ordered([
        r#"LongAgentName({ oneField: "1212", anotherField: 100 })"#,
        r#"[{ oneField: "1212", anotherField: 100 }, { oneField: "1", anotherField: 2 }]"#,
    ]));

    let outputs = ctx
        .cli([
            cmd::REPL,
            flag::LANGUAGE,
            "ts",
            flag::FORMAT,
            "json",
            flag::SCRIPT,
            r#"
                (await LongAgentName.get({oneField: "1212", anotherField: 100})).ask({oneField: "1", anotherField: 2})
            "#,
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_ordered(vec![
        "[",
        "  {",
        r#"    "oneField": "1212","#,
        r#"    "anotherField": 100"#,
        "  },",
        "  {",
        r#"    "oneField": "1","#,
        r#"    "anotherField": 2"#,
        "  }",
        "]"
    ]));
}

#[test]
async fn test_naming_extremes() {
    let mut ctx = TestContext::new();
    let app_name = "test-naming-extremes";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "ts",
            flag::COMPONENT_NAME,
            "app:agent",
        ])
        .await;
    assert!(outputs.success_or_dump());

    let component_golem_yaml = ctx.cwd_path_join(Path::new("app-agent").join("golem.yaml"));

    fs::write_str(
        component_golem_yaml,
        indoc! { r#"
            components:
              app:agent:
                templates: ts
        "# },
    )
    .unwrap();

    let component_source_code =
        ctx.cwd_path_join(Path::new("app-agent").join("src").join("main.ts"));

    fs::copy(
        ctx.test_data_path_join("ts-code-first-snippets/naming_extremes.ts"),
        &component_source_code,
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            flag::YES,
            r#"TestAgent("x")"#,
            "testAll",
        ])
        .await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::GET,
            &format!("StringAgent(    \"{}\"    )", " ".repeat(447)), // HTTP API should normalize it
        ])
        .await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::GET,
            &format!(
                "StructAgent(  {{x:\"{}\"  ,  y    : \"{}\", z: \"{}\" }})", // HTTP API should normalize it
                " ".repeat(102),
                " ".repeat(102),
                "/".repeat(102)
            ),
        ])
        .await;
    assert!(outputs.success_or_dump());
}

// Use UPDATE_GOLDENFILES=1 or `cargo make cli-integration-tests-update-golden-files` to update files
fn check_agent_types_golden_file(
    application_path: &Path,
    language: GuestLanguage,
) -> anyhow::Result<()> {
    let mut mint = Mint::new(test_data_path().join("goldenfiles/extracted-agent-types"));
    let mut mint_file =
        mint.new_goldenfile(format!("code_first_snippets_{}.json", language.id()))?;

    let extract_dir = application_path.join("golem-temp/extracted-agent-types");
    let entries = std::fs::read_dir(&extract_dir)
        .with_context(|| format!("Failed to read directory {}", extract_dir.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    if entries.len() != 1 {
        anyhow::bail!(
            "Expected exactly one entry in {}, got: {:?}",
            extract_dir.display(),
            entries
        );
    }
    let agent_types_source = entries[0].path();

    let formatted_agent_types_json =
        serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(
            &fs::read_to_string(&agent_types_source)?,
        )?)?;

    mint_file.write_all(formatted_agent_types_json.as_bytes())?;

    Ok(())
}
