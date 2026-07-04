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
use std::time::Duration;
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
        import golem.bridge.client.counter_agent.CounterAgentClient
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

/// End-to-end test for the MoonBit bridge generator: deploys the Rust counter
/// agent, generates a MoonBit bridge SDK for it, then compiles and runs a small
/// MoonBit program that invokes the live agent through the generated, async
/// client and verifies the returned values.
///
/// Requires `moon` on the PATH (same as the MoonBit bridge compile tests).
#[test]
#[timeout("10 minutes")]
async fn test_moonbit_bridge_e2e() {
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

    // Generate the MoonBit bridge SDK for the counter agent into a known directory.
    let bridge_root = ctx.cwd_path_join("moonbit-bridge");
    let bridge_root_str = bridge_root.to_str().unwrap().to_string();
    let outputs = ctx
        .cli([
            cmd::GENERATE_BRIDGE,
            flag::LANGUAGE,
            "moonbit",
            flag::AGENT_TYPE_NAME,
            "CounterAgent",
            flag::OUTPUT_DIR,
            &bridge_root_str,
        ])
        .await;
    assert!(outputs.success_or_dump());

    let client_dir = bridge_root.join("counter-agent-client");
    assert!(
        client_dir.join("moon.mod.json").exists(),
        "generated MoonBit bridge module is missing at {}",
        client_dir.display()
    );

    // Write a small MoonBit program that drives the generated client against the
    // live local server, mirroring the Scala bridge e2e check.
    let server_url = ctx.worker_service_url();
    let token = golem_client::LOCAL_WELL_KNOWN_TOKEN;
    let main_mbt = formatdoc! {r#"
        async fn main {{
          @client.CounterAgent::configure(
            @runtime.Custom("{server_url}", "{token}"),
            "{app_name}",
            "local",
          )
          let remote = @client.CounterAgent::get("moonbit-e2e-counter")
          let first = remote.increment()
          let second = remote.increment()
          if first != 1 || second != 2 {{
            abort("Unexpected counter values")
          }}
          println(
            "MOONBIT_BRIDGE_E2E_OK first=" + first.to_string() + " second=" + second.to_string(),
          )
        }}
        "#
    };
    let module_name = std::fs::read_to_string(client_dir.join("moon.mod.json"))
        .unwrap()
        .parse::<serde_json::Value>()
        .unwrap()
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap()
        .to_string();
    let main_moon_pkg = formatdoc! {r#"
        import {{
          "moonbitlang/async" @async,
          "{module_name}/client" @client,
          "{module_name}/runtime" @runtime,
        }}

        options(
          "is-main": true,
        )
        "#
    };
    let main_dir = client_dir.join("main");
    std::fs::create_dir_all(&main_dir).unwrap();
    std::fs::write(main_dir.join("moon.pkg"), main_moon_pkg).unwrap();
    std::fs::write(main_dir.join("main.mbt"), main_mbt).unwrap();

    // Compile and run the generated client + driver with moon.
    let output = std::process::Command::new("moon")
        .arg("run")
        .arg("--target")
        .arg("native")
        .arg("main")
        .current_dir(&client_dir)
        .output()
        .expect("failed to run moon; is it installed?");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "moon run failed in {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        client_dir.display()
    );
    assert!(
        stdout.contains("MOONBIT_BRIDGE_E2E_OK first=1 second=2"),
        "MoonBit bridge e2e program did not produce the expected output.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
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
                external:
                  agents: "*"
              rust:
                external:
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

/// End-to-end test for the Rust guest tool bridge: a provider component
/// defines an `echo` tool with `#[tool_definition]`, the app manifest requests
/// a guest tool bridge for it, and a consumer component in the same app calls
/// the tool through the generated `echo-tool-guest-client` crate.
///
/// The build covers the full pipeline: provider component build -> bundled
/// component metadata extraction (`discover-tools`) -> tool matcher planning ->
/// guest tool client generation -> consumer compilation and linking against the
/// generated crate. The deployment covers registering a tool-only component
/// (the provider exports no agents).
///
/// The invocation currently asserts the worker executor's `golem:tool/host`
/// stub error: through a real deployed invocation, the generated client is
/// proven to reach the executor's `tool-rpc.new` host function (execution
/// stops there today, before `invoke-and-await`). Once the tool runtime is
/// implemented in the worker executor, the invocation is expected to succeed
/// and return `ok:echo:hello`; flip the trailing assertions accordingly —
/// only then does this test validate the generated command path and input
/// encoding against the provider.
#[test]
#[timeout("15 minutes")]
async fn test_rust_tool_guest_bridge_e2e() {
    let mut ctx = TestContext::new();
    let app_name = "tool-bridge";

    ctx.start_server().await;

    fs::create_dir_all(ctx.cwd_path_join(app_name)).unwrap();
    ctx.cd(app_name);

    for component_name in ["tool-bridge:provider", "tool-bridge:consumer"] {
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "rust",
                flag::COMPONENT_NAME,
                component_name,
            ])
            .await;
        assert!(outputs.success_or_dump());
    }

    // Replace the generated app manifest: both template components ship a
    // CounterAgent (which would collide across components) and an httpApi
    // section referencing it; instead the provider becomes a tool-only
    // component and the consumer depends on it, with a guest tool bridge
    // requested for the `echo` tool.
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: tool-bridge

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              tool-bridge:provider:
                dir: "provider"
                templates: rust
              tool-bridge:consumer:
                dir: "consumer"
                templates: rust
                dependencies:
                  tools:
                    - echo

            bridge:
              rust:
                guest:
                  tools:
                    - echo
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();

    // The provider's `src/lib.rs` re-exports `counter_agent::*`, so replacing
    // the module contents keeps the template wiring intact.
    fs::write_str(
        ctx.cwd_path_join("provider/src/counter_agent.rs"),
        indoc! { r#"
            use golem_rust::{tool_definition, tool_implementation};

            /// Echo a message back to the caller.
            #[tool_definition(version = "1.0.0")]
            pub trait Echo {
                fn echo(&self, message: String) -> String;
            }

            struct EchoImpl;

            #[tool_implementation]
            impl Echo for EchoImpl {
                fn echo(&self, message: String) -> String {
                    format!("echo:{message}")
                }
            }
        "# },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("consumer/src/counter_agent.rs"),
        indoc! { r#"
            use echo_tool_guest_client::EchoClient;
            use golem_rust::{agent_definition, agent_implementation};

            #[agent_definition]
            pub trait EchoConsumerAgent {
                fn new(name: String) -> Self;
                async fn call_echo(&self, message: String) -> String;
            }

            struct EchoConsumerImpl;

            #[agent_implementation]
            impl EchoConsumerAgent for EchoConsumerImpl {
                fn new(_name: String) -> Self {
                    Self
                }

                async fn call_echo(&self, message: String) -> String {
                    match EchoClient::new().echo(message).await {
                        Ok(value) => format!("ok:{value}"),
                        Err(error) => format!("err:{error:?}"),
                    }
                }
            }
        "# },
    )
    .unwrap();

    let consumer_cargo_toml_path = ctx.cwd_path_join("consumer/Cargo.toml");
    let consumer_cargo_toml = fs::read_to_string(&consumer_cargo_toml_path).unwrap();
    assert!(consumer_cargo_toml.contains("[dependencies]"));
    fs::write_str(
        &consumer_cargo_toml_path,
        consumer_cargo_toml.replace(
            "[dependencies]",
            indoc! { r#"
                [dependencies]
                echo-tool-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/echo-tool-guest-client" }
            "# }
            .trim_end(),
        ),
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    let generated_client_dir =
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/echo-tool-guest-client");
    assert!(generated_client_dir.join("Cargo.toml").exists());

    // The consumer build above already compiled the generated client crate as
    // a dependency; this standalone build additionally validates that the
    // generated crate compiles to wasm32-wasip2 on its own (its manifest and
    // dependency resolution are self-contained).
    let output = std::process::Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg("wasm32-wasip2")
        .current_dir(&generated_client_dir)
        .output()
        .expect("failed to run cargo; is it installed?");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "standalone wasm32-wasip2 build of the generated tool client failed in {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        generated_client_dir.display()
    );

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let uuid = Uuid::new_v4().to_string();
    let agent_constructor = format!("EchoConsumerAgent(\"{uuid}\")");
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &agent_constructor,
            "call_echo",
            "\"hello\"",
        ])
        .await;

    // The worker executor's `golem:tool/host` is not implemented yet:
    // `tool-rpc.new` traps (before `invoke-and-await` is ever reached), so the
    // invocation must fail with the stub error, proving the generated client
    // reaches the executor's tool host. Once the tool runtime lands, replace
    // this with:
    //     assert!(outputs.success_or_dump());
    //     assert!(outputs.stdout_contains("ok:echo:hello"));
    let invocation_reached_tool_host_stub =
        !outputs.success() && outputs.stderr_contains("golem:tool/host is not yet implemented");
    if !invocation_reached_tool_host_stub {
        outputs.dump();
    }
    assert!(
        invocation_reached_tool_host_stub,
        "expected the tool invocation to fail with the executor's golem:tool/host stub error"
    );
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
                external:
                  agents: "*"
              rust:
                external:
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
    let outputs = ctx.cli([flag::SHOW_SECRETS, cmd::DEPLOY, flag::YES]).await;
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

    let outputs = ctx.cli([flag::SHOW_SECRETS, cmd::DEPLOY, flag::YES]).await;
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

// Runs `agent list --mode <mode> --format json` against the live server and
// returns the rendered agent-name strings from the response. The JSON view's
// `agentName` is the rendered, language-specific agent id, e.g.
// `DurableListAgent("…")`. Panics on non-zero exit or non-JSON output so test
// failures point at the listing step that broke.
async fn list_agent_names(ctx: &TestContext, mode: &str) -> Vec<String> {
    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::LIST,
            "--mode",
            mode,
            flag::FORMAT,
            "json",
            "--max-count",
            "200",
        ])
        .await;
    assert!(
        outputs.success_or_dump(),
        "`agent list --mode {mode}` failed"
    );

    let response = outputs
        .stdout_json::<AgentListResponseView>()
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("`agent list --mode {mode}` produced no JSON output"));
    response.agents.into_iter().map(|a| a.agent_name).collect()
}

// Scaffolds a fresh Rust app with two agent types: `DurableListAgent` (durable
// by default) and `EphemeralListAgent` (explicitly `mode = "ephemeral"`), then
// builds and deploys it. The caller is responsible for invoking agents so they
// appear in the listing. Returns nothing; the app is live on the started server.
async fn setup_list_mode_filter_app(ctx: &mut TestContext, app_name: &str) {
    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    // Replace the default app manifest with a minimal single-component one,
    // dropping the default `httpApi` block so the build does not require a
    // deployed HTTP mapping for CounterAgent.
    let component_manifest_path = ctx.cwd_path_join("golem.yaml");
    fs::write_str(
        &component_manifest_path,
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: {app_name}

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            components:
              {app_name}:rust-main:
                templates: rust
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();

    // Replace the generated `src/counter_agent.rs` with two agents: one durable
    // (the `#[agent_definition]` default) and one explicitly ephemeral via
    // `mode = "ephemeral"`. `src/lib.rs` already re-exports `counter_agent::*`.
    let component_source_code_file = ctx.cwd_path_join("src/counter_agent.rs");
    fs::write_str(
        &component_source_code_file,
        indoc! { r#"
            use golem_rust::{agent_definition, agent_implementation};

            #[agent_definition]
            pub trait DurableListAgent {
                fn new(id: String) -> Self;
                fn ping(&self) -> String;
            }

            struct DurableListImpl {
                id: String,
            }

            #[agent_implementation]
            impl DurableListAgent for DurableListImpl {
                fn new(id: String) -> Self {
                    Self { id }
                }

                fn ping(&self) -> String {
                    format!("durable:{}", self.id)
                }
            }

            #[agent_definition(mode = "ephemeral")]
            pub trait EphemeralListAgent {
                fn new(id: String) -> Self;
                fn ping(&self) -> String;
            }

            struct EphemeralListImpl {
                id: String,
            }

            #[agent_implementation]
            impl EphemeralListAgent for EphemeralListImpl {
                fn new(id: String) -> Self {
                    Self { id }
                }

                fn ping(&self) -> String {
                    format!("ephemeral:{}", self.id)
                }
            }
        "# },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
}

// Invokes `agent_type_name("<id>")` `ping` once, creating a worker/oplog entry
// for that agent instance. Panics on non-zero exit or missing ping echo so a
// failure points at the invocation step that broke.
async fn invoke_list_agent(ctx: &TestContext, agent_type_name: &str, id: &str, echo_prefix: &str) {
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("{agent_type_name}(\"{id}\")"),
            "ping",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        outputs.stdout_contains(format!("{echo_prefix}{id}")),
        "invocation of {agent_type_name}(\"{id}\") did not echo {echo_prefix}{id}"
    );
}

const MODE_FILTER_INSTANCE_COUNT: usize = 20;

// Verifies that `agent list --mode ephemeral|durable|all` correctly partitions
// the listing by agent durability mode, using a non-trivial number of agent
// instances (20 durable + 20 ephemeral) rather than a single pair.
//
// The CLI injects a `mode == <mode>` metadata filter for `--mode ephemeral` and
// `--mode durable`, while `--mode all` forwards no mode filter so the executor
// scans both modes. The executor's `modes_from_filter` then narrows the oplog
// scan to the requested mode (defaulting to durable when the filter is empty).
//
// Asserts:
//   - `--mode ephemeral` lists exactly the 20 ephemeral agents and no durable ones
//   - `--mode durable`   lists exactly the 20 durable agents and no ephemeral ones
//   - `--mode all`       lists all 40 agents
//
// Any of these assertions failing surfaces a regression in the
// CLI→worker-service→executor mode-filtering path.
#[test]
#[timeout("15 minutes")]
async fn test_agent_list_mode_filter() {
    let mut ctx = TestContext::new();
    let app_name = "agent-list-mode-filter";

    setup_list_mode_filter_app(&mut ctx, app_name).await;

    // Create 20 durable + 20 ephemeral agent instances. Each invocation with a
    // fresh unique id constructs a new agent, and `ping` ensures it gets a
    // worker/oplog entry the listing can surface.
    let durable_ids: Vec<String> = (0..MODE_FILTER_INSTANCE_COUNT)
        .map(|_| Uuid::new_v4().to_string())
        .collect();
    let ephemeral_ids: Vec<String> = (0..MODE_FILTER_INSTANCE_COUNT)
        .map(|_| Uuid::new_v4().to_string())
        .collect();

    for id in &durable_ids {
        invoke_list_agent(&ctx, "DurableListAgent", id, "durable:").await;
    }
    for id in &ephemeral_ids {
        invoke_list_agent(&ctx, "EphemeralListAgent", id, "ephemeral:").await;
    }

    let count_durable = |names: &[String]| {
        names
            .iter()
            .filter(|n| n.starts_with("DurableListAgent("))
            .count()
    };
    let count_ephemeral = |names: &[String]| {
        names
            .iter()
            .filter(|n| n.starts_with("EphemeralListAgent("))
            .count()
    };

    let ephemeral_names = list_agent_names(&ctx, "ephemeral").await;
    let e_count = count_ephemeral(&ephemeral_names);
    let d_count = count_durable(&ephemeral_names);
    assert_eq!(
        e_count, MODE_FILTER_INSTANCE_COUNT,
        "`agent list --mode ephemeral` must list exactly {MODE_FILTER_INSTANCE_COUNT} ephemeral \
         agents, got {e_count} ephemeral and {d_count} durable: {ephemeral_names:?}"
    );
    assert_eq!(
        d_count, 0,
        "`agent list --mode ephemeral` must not list durable agents, got {d_count}: {ephemeral_names:?}"
    );

    let durable_names = list_agent_names(&ctx, "durable").await;
    let d_count = count_durable(&durable_names);
    let e_count = count_ephemeral(&durable_names);
    assert_eq!(
        d_count, MODE_FILTER_INSTANCE_COUNT,
        "`agent list --mode durable` must list exactly {MODE_FILTER_INSTANCE_COUNT} durable \
         agents, got {d_count} durable and {e_count} ephemeral: {durable_names:?}"
    );
    assert_eq!(
        e_count, 0,
        "`agent list --mode durable` must not list ephemeral agents, got {e_count}: {durable_names:?}"
    );

    let all_names = list_agent_names(&ctx, "all").await;
    let d_count = count_durable(&all_names);
    let e_count = count_ephemeral(&all_names);
    assert_eq!(
        d_count, MODE_FILTER_INSTANCE_COUNT,
        "`agent list --mode all` must list exactly {MODE_FILTER_INSTANCE_COUNT} durable agents, \
         got {d_count}: {all_names:?}"
    );
    assert_eq!(
        e_count, MODE_FILTER_INSTANCE_COUNT,
        "`agent list --mode all` must list exactly {MODE_FILTER_INSTANCE_COUNT} ephemeral agents, \
         got {e_count}: {all_names:?}"
    );
}

// Verifies that the TS REPL `:agent-list --mode ephemeral|durable|all` colon
// command forwards the `--mode` parameter to the underlying `agent list` CLI
// invocation and that the listing is partitioned correctly.
//
// The TS REPL's `:agent-list` command delegates to `golem-cli agent list` via
// the REPL control socket. If the `--mode` flag were silently dropped (e.g. the
// colon-command arg parser failed to forward it), the listing would fall back
// to the `durable` default and never show ephemeral agents. This test catches
// that by asserting `EphemeralListAgent` appears for `--mode ephemeral` and
// `DurableListAgent` appears for `--mode durable`.
//
// Uses a Rust app + TS REPL (the same pattern as `test_rust_counter`) so the
// test does not depend on the TS component template's agent_guest.wasm.
#[test]
#[timeout("10 minutes")]
async fn test_agent_list_mode_filter_in_ts_repl() {
    let mut ctx = TestContext::new();
    let app_name = "agent-list-mode-repl";

    setup_list_mode_filter_app(&mut ctx, app_name).await;

    // Create a small set of durable + ephemeral instances. The REPL test proves
    // mode forwarding, not scale — 3 of each is enough to distinguish the
    // partitions and keep the interactive session fast.
    for _ in 0..3 {
        let id = Uuid::new_v4().to_string();
        invoke_list_agent(&ctx, "DurableListAgent", &id, "durable:").await;
    }
    for _ in 0..3 {
        let id = Uuid::new_v4().to_string();
        invoke_list_agent(&ctx, "EphemeralListAgent", &id, "ephemeral:").await;
    }

    // The REPL renders `agent list` as a text table in the PTY. Long agent names
    // like `EphemeralListAgent("<uuid>")` wrap across multiple terminal rows in
    // the narrow (80-column) PTY, so matching the full name fails. Instead we
    // match on the first 8 characters of each agent type name (`Ephemera` /
    // `DurableL`) — exactly the width of the Agent name column — which is enough
    // to prove the mode filter was forwarded: if `--mode ephemeral` were dropped,
    // the default `durable` listing would show `DurableL` and never `Ephemera`.
    //
    // We don't pass `--format json` to the REPL colon command: the REPL renders
    // `agent list` as a text table by default, which is the natural output to
    // match here. The short column-width prefixes are enough to prove the mode
    // filter was forwarded.
    ctx.cli_interactive_repl_test(
        [
            cmd::REPL,
            flag::LANGUAGE,
            "ts",
            flag::YES,
            "--disable-stream",
        ],
        move |session| {
            session.set_expect_timeout(Some(Duration::from_secs(300)));
            session.expect_regex(r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>")?;

            // The colon-command adapter shows the prompt once or twice after
            // each command: once from `displayPrompt()` in the action's finally
            // block, and sometimes again from the REPL's own eval callback. We
            // always consume the first prompt, then try to consume a second
            // with a short timeout. Without consuming at least the first prompt,
            // the next `:agent-list` command is not reliably forwarded to the
            // server — the REPL's input buffer gets cleared by
            // `clearBufferedCommand()` in the action's finally block.
            let prompt_regex = r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>";

            // `:agent-list --mode ephemeral` must forward the mode and show only
            // ephemeral agents. If the mode were dropped, the default `durable`
            // listing would show `DurableL` and `Ephemera` would never appear,
            // timing out this expectation.
            session.send_line_and_expect_regex(r":agent-list --mode ephemeral", r"Ephemera")?;
            session.expect_regex(prompt_regex)?;
            session.set_expect_timeout(Some(Duration::from_secs(5)));
            let _ = session.expect_regex(prompt_regex);
            session.set_expect_timeout(Some(Duration::from_secs(300)));

            // `:agent-list --mode durable` must forward the mode and show durable
            // agents.
            session.send_line_and_expect_regex(r":agent-list --mode durable", r"DurableL")?;
            session.expect_regex(prompt_regex)?;
            session.set_expect_timeout(Some(Duration::from_secs(5)));
            let _ = session.expect_regex(prompt_regex);
            session.set_expect_timeout(Some(Duration::from_secs(300)));

            // `:agent-list --mode all` must show both partitions.
            session.send_line_and_expect_regex(r":agent-list --mode all", r"DurableL")?;
            session.expect_regex(r"Ephemera")?;
            session.expect_regex(prompt_regex)?;
            session.set_expect_timeout(Some(Duration::from_secs(5)));
            let _ = session.expect_regex(prompt_regex);
            session.set_expect_timeout(Some(Duration::from_secs(300)));

            session.send_line(".exit")?;
            session.expect_eof()?;

            Ok(())
        },
    )
    .await;
}

// JSON view of the `agent list` structured output. We only need the `agents`
// array and the rendered `agentName` field; serde ignores the `outputType`
// discriminator injected by the CLI's structured-output wrapper.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentListResponseView {
    agents: Vec<AgentListView>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentListView {
    agent_name: String,
}

// Use UPDATE_GOLDENFILES=1 or `cargo make cli-integration-tests-update-golden-files` to update files
fn check_agent_types_golden_file(
    application_path: &Path,
    language: GuestLanguage,
) -> anyhow::Result<()> {
    let mut mint = Mint::new(test_data_path().join("goldenfiles/extracted-agent-types"));
    let mut mint_file =
        mint.new_goldenfile(format!("code_first_snippets_{}.json", language.id()))?;

    let extract_dir = application_path.join("golem-temp/extracted-component-metadata");
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
    let metadata_source = entries[0].path();

    // The extracted metadata file bundles agent types and tools; the golden
    // files cover the agent type list only (as a bare array).
    let metadata_json =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&metadata_source)?)?;
    let agent_types_json = metadata_json.get("agentTypes").with_context(|| {
        format!(
            "Missing agentTypes field in extracted component metadata {}",
            metadata_source.display()
        )
    })?;
    let formatted_agent_types_json = serde_json::to_string_pretty(agent_types_json)?;

    mint_file.write_all(formatted_agent_types_json.as_bytes())?;

    Ok(())
}
