use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

async fn deployed_moonbit_reflection_context() -> TestContext {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("moonbit-reflection")).unwrap();
    ctx.cd("moonbit-reflection");
    let created = ctx
        .cli([flag::YES, cmd::NEW, ".", flag::TEMPLATE, "moonbit"])
        .await;
    assert!(created.success_or_dump());

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: moonbit-reflection
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          moonbit-reflection:main:
            dir: .
            templates: moonbit
        tools:
          reflection-conformance: {{}}
        agents:
          ReflectionCaller:
            tools:
              reflection-conformance: {{}}
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    let package_path = ctx.cwd_path_join("moon.pkg");
    let package = fs::read_to_string(&package_path).unwrap().replace(
        "import {",
        "import {\n  \"golemcloud/golem_sdk/reflection\",",
    );
    fs::write_str(package_path, package).unwrap();
    fs::write_str(
        ctx.cwd_path_join("reflection.mbt"),
        indoc! {r#"
        #derive.tool("reflection-conformance")
        struct ReflectionConformanceTool {}

        #warnings("-struct_never_constructed")
        struct Meters {}

        impl @schema.QuantityUnit for Meters with fn type_id() {
          "reflection-conformance.Meters"
        }

        impl @schema.QuantityUnit for Meters with fn base_unit() { "m" }

        impl @schema.QuantityUnit for Meters with fn allowed_suffixes() { [] }

        #derive.arg("maybe", scope="option")
        pub fn ReflectionConformanceTool::canonical(
          signed : Int64,
          unsigned : UInt64,
          duration : @schema.Duration,
          quantity : @schema.Quantity[Meters],
          maybe : String?,
        ) -> String {
          let quantity = quantity.value()
          if signed == -9223372036854775807L - 1L &&
            unsigned == 18446744073709551615UL &&
            duration == @schema.Duration::new(9223372036854775807L) &&
            quantity.mantissa == -9223372036854775807L - 1L &&
            quantity.scale == -9 &&
            quantity.unit == "m" &&
            maybe is None {
            "moonbit-canonical-ok"
          } else {
            "moonbit-canonical-mismatch"
          }
        }

        #derive.agent
        struct ReflectionCaller { name : String }

        fn ReflectionCaller::new(name : String) -> ReflectionCaller { { name, } }

        fn describe_tool_error(
          error : @tool.ToolError[@reflection.ReflectedToolCustomError],
        ) -> String {
          match error {
            Rpc(error) => "rpc:\{Repr(error)}"
            RemoteTool(error) => "remote:\{Repr(error)}"
            Tool(custom) => {
              @model.drop_owned_capabilities(custom.payload.value)
              "tool"
            }
            UnknownToolError(_, payload) => {
              @model.drop_owned_capabilities(payload.value)
              "unknown"
            }
            InvalidInput(message) => "input:\{message}"
            MalformedRemoteOutput(message) => "output:\{message}"
          }
        }

        pub async fn ReflectionCaller::run(self : Self) -> String {
          ignore(self)
          let tool = @reflection.get_tool_type("reflection-conformance") catch {
            error => return "discovery:\{Repr(error)}"
          }
          let command = tool.command(["canonical"]) catch {
            error => return "command:\{Repr(error)}"
          }
          let canonical = command.invoke_json(Json::object({
            "signed": Json::string("-9223372036854775808"),
            "unsigned": Json::string("18446744073709551615"),
            "duration": Json::object({
              "nanoseconds": Json::string("9223372036854775807"),
            }),
            "quantity": Json::object({
              "mantissa": Json::string("-9223372036854775808"),
              "scale": Json::number(-9.0),
              "unit": Json::string("m"),
            }),
          }))
          match canonical {
            Ok(Some(Json::String(value))) => value
            Ok(_) => "canonical-error:unexpected"
            Err(error) => "canonical-error:\{describe_tool_error(error)}"
          }
        }
    "#},
    )
    .unwrap();

    assert!(ctx.cli([cmd::BUILD]).await.success_or_dump());
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    ctx
}

#[test]
#[timeout("20 minutes")]
async fn test_moonbit_reflected_invoke_json_preserves_canonical_wide_values() {
    let ctx = deployed_moonbit_reflection_context().await;
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("ReflectionCaller(\"{}\")", Uuid::new_v4()),
            "run",
        ])
        .await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains("moonbit-canonical-ok"));
}
