// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{TestContext, cmd, flag};
use crate::workspace_path;
use serde_json::{Value, json};
use test_r::{test, timeout};
use uuid::Uuid;

const FIXTURE: &str = "tool-middleware-conformance";

#[derive(Clone, Copy)]
enum MergeMode {
    Omitted,
    Prepend,
    Append,
    Replace,
}

impl MergeMode {
    fn manifest_name(self) -> Option<&'static str> {
        match self {
            Self::Omitted => None,
            Self::Prepend => Some("prepend"),
            Self::Append => Some("append"),
            Self::Replace => Some("replace"),
        }
    }
}

fn installation(name: &str, label: Option<&str>) -> Value {
    let mut value = json!({
        "name": name,
        "filesystemAccess": "allowed",
    });
    if let Some(label) = label {
        value["parameters"] = json!({ "label": label });
    }
    value
}

fn prepare_fixture(ctx: &mut TestContext) {
    fs_extra::dir::copy(
        ctx.test_data_path_join(FIXTURE),
        ctx.cwd_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();
    ctx.cd(FIXTURE);

    let sdk = workspace_path().join("sdks/rust/golem-rust");
    for component in ["middleware", "caller"] {
        let manifest = ctx.cwd_path_join(format!("{component}/Cargo.toml"));
        let contents = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            manifest,
            contents.replace("__GOLEM_RUST_PATH__", sdk.to_str().unwrap()),
        )
        .unwrap();
    }
}

fn configure_manifest(
    ctx: &TestContext,
    universal: &[&str],
    environment: Option<&[&str]>,
    agent: Option<&[&str]>,
    merge_mode: MergeMode,
) {
    let path = ctx.cwd_path_join("golem.yaml");
    let mut manifest: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    manifest["environments"]["local"]["tools"]["middleware"] = Value::Array(
        universal
            .iter()
            .map(|label| installation("manifest-universal", Some(label)))
            .collect(),
    );

    let environment_binding = &mut manifest["environments"]["local"]["tools"]["manifest-probe"];
    match environment {
        Some(labels) => {
            environment_binding["middleware"] = Value::Array(
                labels
                    .iter()
                    .map(|label| installation("manifest-label-layer", Some(label)))
                    .collect(),
            );
        }
        None => {
            environment_binding
                .as_object_mut()
                .unwrap()
                .remove("middleware");
        }
    }

    let agent_binding =
        &mut manifest["agents"]["MiddlewareConformanceAgent"]["tools"]["manifest-probe"];
    match agent {
        Some(labels) => {
            agent_binding["middleware"] = Value::Array(
                labels
                    .iter()
                    .map(|label| installation("manifest-label-layer", Some(label)))
                    .collect(),
            );
        }
        None => {
            agent_binding.as_object_mut().unwrap().remove("middleware");
        }
    }
    match merge_mode.manifest_name() {
        Some(mode) => agent_binding["middlewareMergeMode"] = json!(mode),
        None => {
            agent_binding
                .as_object_mut()
                .unwrap()
                .remove("middlewareMergeMode");
        }
    }

    std::fs::write(path, serde_yaml::to_string(&manifest).unwrap()).unwrap();
}

async fn build(ctx: &mut TestContext) {
    let output = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(output.success_or_dump());
}

async fn deploy(ctx: &mut TestContext) {
    let output = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(output.success_or_dump());
}

async fn invoke_agent(
    ctx: &mut TestContext,
    agent_type: &str,
    agent: &str,
    method: &str,
    args: &[&str],
) -> String {
    let constructor = format!("{agent_type}(\"{agent}\")");
    let output = ctx
        .cli(
            [flag::YES, cmd::AGENT, cmd::INVOKE, &constructor, method]
                .into_iter()
                .chain(args.iter().copied()),
        )
        .await;
    assert!(output.success_or_dump());
    output.stdout().collect::<Vec<_>>().join("\n")
}

async fn invoke(ctx: &mut TestContext, agent: &str, method: &str, args: &[&str]) -> String {
    invoke_agent(ctx, "MiddlewareConformanceAgent", agent, method, args).await
}

fn set_compatibility_case(ctx: &TestContext, tool: &str, middleware: &str, mode: &str) {
    let path = ctx.cwd_path_join("golem.yaml");
    let mut manifest: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    manifest["environments"]["local"]["deployment"]["toolCompatibilityMode"] = json!(mode);
    manifest["environments"]["local"]["tools"][tool]["middleware"] =
        Value::Array(vec![installation(middleware, None)]);
    std::fs::write(path, serde_yaml::to_string(&manifest).unwrap()).unwrap();
}

async fn tool_get_json(ctx: &mut TestContext, tool: &str) -> Value {
    let output = ctx
        .cli([flag::FORMAT, "json", cmd::TOOL, cmd::GET, tool])
        .await;
    assert!(output.success_or_dump());
    output
        .stdout_json::<Value>()
        .into_iter()
        .next()
        .expect("tool get produced JSON")
}

#[test]
#[timeout("20m")]
async fn manifest_middleware_merge_matrix_executes_in_live_order() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);

    struct Case {
        name: &'static str,
        environment: Option<&'static [&'static str]>,
        agent: Option<&'static [&'static str]>,
        mode: MergeMode,
        expected_order: &'static str,
        expected_value: &'static str,
    }

    let cases = [
        Case {
            name: "omitted",
            environment: None,
            agent: None,
            mode: MergeMode::Omitted,
            expected_order: "U1;U2;leaf:input;",
            expected_value: "ok:leaf(input)",
        },
        Case {
            name: "explicit-empty",
            environment: Some(&[]),
            agent: None,
            mode: MergeMode::Prepend,
            expected_order: "U1;U2;leaf:input;",
            expected_value: "ok:leaf(input)",
        },
        Case {
            name: "explicit-prepend",
            environment: Some(&["E"]),
            agent: Some(&["A"]),
            mode: MergeMode::Prepend,
            expected_order: "U1;U2;A;E;leaf:E(A(input));",
            expected_value: "ok:A[E[leaf(E(A(input)))]]",
        },
        Case {
            name: "default-prepend-duplicates",
            environment: Some(&["E1", "E2"]),
            agent: Some(&["A1", "A2"]),
            mode: MergeMode::Omitted,
            expected_order: "U1;U2;A1;A2;E1;E2;leaf:E2(E1(A2(A1(input))));",
            expected_value: "ok:A1[A2[E1[E2[leaf(E2(E1(A2(A1(input)))))]]]]",
        },
        Case {
            name: "append",
            environment: Some(&["E"]),
            agent: Some(&["A"]),
            mode: MergeMode::Append,
            expected_order: "U1;U2;E;A;leaf:A(E(input));",
            expected_value: "ok:E[A[leaf(A(E(input)))]]",
        },
        Case {
            name: "replace",
            environment: Some(&["E"]),
            agent: Some(&["A"]),
            mode: MergeMode::Replace,
            expected_order: "U1;U2;A;leaf:A(input);",
            expected_value: "ok:A[leaf(A(input))]",
        },
        Case {
            name: "replace-empty-keeps-universal",
            environment: Some(&["E"]),
            agent: Some(&[]),
            mode: MergeMode::Replace,
            expected_order: "U1;U2;leaf:input;",
            expected_value: "ok:leaf(input)",
        },
    ];

    configure_manifest(
        &ctx,
        &["U1", "U2"],
        cases[0].environment,
        cases[0].agent,
        cases[0].mode,
    );
    build(&mut ctx).await;
    ctx.start_server().await;
    for case in cases {
        configure_manifest(&ctx, &["U1", "U2"], case.environment, case.agent, case.mode);
        deploy(&mut ctx).await;
        let agent = format!("{}-{}", case.name, Uuid::new_v4());
        let result = invoke(&mut ctx, &agent, "invoke", &["\"input\""]).await;
        assert!(result.contains(case.expected_value), "{result}");
        let effects = invoke(&mut ctx, &agent, "effects", &[]).await;
        assert!(effects.contains(case.expected_order), "{effects}");
    }
}

#[test]
#[timeout("20m")]
async fn universal_middleware_can_be_installed_on_one_tool_binding() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);
    configure_manifest(&ctx, &[], None, Some(&[]), MergeMode::Replace);
    let path = ctx.cwd_path_join("golem.yaml");
    let mut manifest: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    manifest["agents"]["MiddlewareConformanceAgent"]["tools"]["manifest-probe"]["middleware"] =
        Value::Array(vec![installation("manifest-universal", Some("SELECTIVE"))]);
    std::fs::write(path, serde_yaml::to_string(&manifest).unwrap()).unwrap();

    build(&mut ctx).await;
    ctx.start_server().await;
    deploy(&mut ctx).await;

    let agent = format!("selective-universal-{}", Uuid::new_v4());
    let selected = invoke(&mut ctx, &agent, "invoke", &["\"selected\""]).await;
    assert!(selected.contains("ok:leaf(selected)"), "{selected}");
    let unselected = invoke(&mut ctx, &agent, "invoke_compat", &["\"unselected\""]).await;
    assert!(unselected.contains("ok:leaf: unselected"), "{unselected}");
    let effects = invoke(&mut ctx, &agent, "effects", &[]).await;
    assert!(
        effects.contains("SELECTIVE;leaf:selected;compat:unselected;"),
        "{effects}"
    );
    assert_eq!(effects.matches("SELECTIVE;").count(), 1, "{effects}");
}

#[test]
#[timeout("20m")]
async fn middleware_declared_error_retry_transformation_and_replay_are_counted() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);
    configure_manifest(&ctx, &[], None, Some(&[]), MergeMode::Replace);
    let path = ctx.cwd_path_join("golem.yaml");
    let mut manifest: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    manifest["agents"]["MiddlewareConformanceAgent"]["tools"]["manifest-probe"]["middleware"] =
        Value::Array(vec![installation("control-retry-transform", None)]);
    std::fs::write(path, serde_yaml::to_string(&manifest).unwrap()).unwrap();

    build(&mut ctx).await;
    ctx.start_server().await;
    deploy(&mut ctx).await;
    let agent = format!("control-{}", Uuid::new_v4());
    let result = invoke(&mut ctx, &agent, "invoke", &["\"value\""]).await;
    assert!(
        result.contains("ok:transformed(reject(value))|leaf(retry(value))"),
        "{result}"
    );
    let expected = "leaf:reject(value);leaf:retry(value);";
    let effects = invoke(&mut ctx, &agent, "effects", &[]).await;
    assert!(effects.contains(expected), "{effects}");

    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;
    let replayed = invoke(&mut ctx, &agent, "effects", &[]).await;
    assert!(replayed.contains(expected), "{replayed}");
    assert_eq!(replayed.matches("leaf:reject(value)").count(), 1);
    assert_eq!(replayed.matches("leaf:retry(value)").count(), 1);
}

#[test]
#[timeout("20m")]
async fn structural_projection_strict_rejection_and_nominal_runtime_validation() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);
    set_compatibility_case(
        &ctx,
        "compat-leaf",
        "structural-projection",
        "strict-equality",
    );
    build(&mut ctx).await;
    ctx.start_server().await;

    let strict = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(!strict.success_or_dump());
    let strict_error = format!(
        "{}\n{}",
        strict.stdout_text(),
        strict.stderr().collect::<Vec<_>>().join("\n")
    );
    assert!(
        strict_error.contains("structural-projection"),
        "{strict_error}"
    );
    assert!(strict_error.contains("compat-leaf"), "{strict_error}");

    set_compatibility_case(
        &ctx,
        "compat-leaf",
        "structural-projection",
        "structural-subtype",
    );
    deploy(&mut ctx).await;

    let structural = ctx
        .cli([
            flag::FORMAT,
            "json",
            cmd::TOOL,
            cmd::INVOKE,
            "--component",
            "conformance:caller",
            "compat-leaf",
            "--",
            "execute",
            r#"{"kept":"visible","discarded":"outer-only"}"#,
        ])
        .await;
    assert!(structural.success_or_dump());
    let structural_text = structural.stdout_text();
    assert!(
        structural_text.contains("leaf: visible"),
        "{structural_text}"
    );
    assert!(!structural_text.contains("leafOnly"), "{structural_text}");

    set_compatibility_case(&ctx, "compat-leaf", "structural-adapter", "nominal");
    set_compatibility_case(&ctx, "nominal-leaf", "nominal-adapter", "nominal");
    deploy(&mut ctx).await;
    let nominal = ctx
        .cli([
            flag::FORMAT,
            "json",
            cmd::TOOL,
            cmd::INVOKE,
            "--component",
            "conformance:caller",
            "nominal-leaf",
            "--",
            "check",
            r#"{"middlewareValue":"invalid-for-leaf"}"#,
        ])
        .await;
    assert!(!nominal.success_or_dump());
    let nominal_error = nominal.stderr().collect::<Vec<_>>().join("\n");
    assert!(
        nominal_error.contains("middlewareValue") || nominal_error.contains("leafValue"),
        "{nominal_error}"
    );
}

#[test]
#[timeout("20m")]
async fn adapter_uses_lookup_identity_with_presented_metadata_and_generated_client() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);
    build(&mut ctx).await;
    ctx.start_server().await;
    deploy(&mut ctx).await;

    let generated = std::fs::read_to_string(ctx.cwd_path_join(
        "golem-temp/bridge-sdk/rust/internal/compat-leaf-tool-guest-client/src/lib.rs",
    ))
    .unwrap();
    assert!(
        generated.contains("pub struct PresentedAdapterClient"),
        "{generated}"
    );
    assert!(generated.contains("ToolRpc::new("), "{generated}");
    assert!(generated.contains("\"compat-leaf\""), "{generated}");

    let agent = format!("adapter-{}", Uuid::new_v4());
    let constructor = format!("AdapterConformanceAgent(\"{agent}\")");
    let result = invoke_agent(
        &mut ctx,
        "AdapterConformanceAgent",
        &agent,
        "invoke",
        &["\"through-adapter\""],
    )
    .await;
    assert!(result.contains("ok:leaf: through-adapter"), "{result}");
    let help = ctx
        .cli([
            cmd::TOOL,
            cmd::INVOKE,
            "--agent",
            &constructor,
            "compat-leaf",
            "--",
            "--help",
        ])
        .await;
    assert!(help.success_or_dump());
    assert!(help.stdout_text().contains("presented-adapter"));
}

#[test]
#[timeout("20m")]
async fn universal_middleware_preserves_complete_discovery_metadata() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);
    configure_manifest(&ctx, &[], None, Some(&[]), MergeMode::Replace);
    build(&mut ctx).await;
    ctx.start_server().await;
    deploy(&mut ctx).await;
    let before = tool_get_json(&mut ctx, "manifest-probe").await;

    configure_manifest(&ctx, &["TRANSPARENT"], None, Some(&[]), MergeMode::Replace);
    deploy(&mut ctx).await;
    let after = tool_get_json(&mut ctx, "manifest-probe").await;
    assert_eq!(
        before["tool"]["definition"], after["tool"]["definition"],
        "universal middleware changed discovery metadata"
    );

    let contract: Value = serde_json::from_str(
        &std::fs::read_to_string(
            workspace_path().join("test-data/gol-40/rich-tool-conformance-v1.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(contract["contract"], "GOL-40-CONTRACT-1");
    assert_eq!(contract["comparison"]["permittedDifferences"], json!([]));

    let agent = format!("transparent-{}", Uuid::new_v4());
    let result = invoke(&mut ctx, &agent, "invoke", &["\"metadata\""]).await;
    assert!(result.contains("ok:leaf(metadata)"), "{result}");
    let effects = invoke(&mut ctx, &agent, "effects", &[]).await;
    assert!(effects.contains("TRANSPARENT;leaf:metadata;"), "{effects}");
}
