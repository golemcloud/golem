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
use golem_common::schema::tool::Tool;
use serde_json::{Value, json};
use test_r::{test, timeout};
use uuid::Uuid;

#[path = "chunk_d_test_first_metadata_oracle.rs"]
mod metadata_oracle;

const FIXTURE: &str = "chunk-d-test-first-tool-middleware";

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

fn prepare_fixture(ctx: &mut TestContext, include_rich_tool: bool) {
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
    if include_rich_tool {
        std::fs::write(
            ctx.cwd_path_join("middleware/src/rich_provider.rs"),
            rich_provider_source(),
        )
        .unwrap();
    } else {
        let source = ctx.cwd_path_join("middleware/src/lib.rs");
        let contents = std::fs::read_to_string(&source).unwrap();
        std::fs::write(source, contents.replace("mod rich_provider;\n", "")).unwrap();

        let manifest_path = ctx.cwd_path_join("golem.yaml");
        let mut manifest: Value =
            serde_yaml::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        manifest["environments"]["local"]["tools"]
            .as_object_mut()
            .unwrap()
            .remove("artifact");
        manifest["tools"]
            .as_object_mut()
            .unwrap()
            .remove("artifact");
        manifest["tools"].as_object_mut().unwrap().remove("render");
        manifest["agents"]["MiddlewareConformanceAgent"]["tools"]
            .as_object_mut()
            .unwrap()
            .remove("artifact");
        std::fs::write(manifest_path, serde_yaml::to_string(&manifest).unwrap()).unwrap();
    }
}

fn require_presented_adapter_client(ctx: &TestContext) {
    let source = ctx.cwd_path_join("caller/src/lib.rs");
    let contents = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        source,
        contents.replace("CompatLeafClient", "PresentedAdapterClient"),
    )
    .unwrap();
}

fn rich_provider_source() -> String {
    let source = std::fs::read_to_string(
        workspace_path().join("sdks/rust/golem-rust/tests/rich_tool_conformance.rs"),
    )
    .unwrap();
    let body = source
        .split_once("mod fixture {")
        .unwrap()
        .1
        .split_once("    fn type_json(")
        .unwrap()
        .0;
    let body = body
        .lines()
        .filter(|line| {
            !line.contains("use serde_json::{Value, json};") && !line.contains("use test_r::test;")
        })
        .map(|line| line.strip_prefix("    ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"{body}

use golem_rust::tool_implementation;

struct ArtifactImpl;

#[tool_implementation]
impl Artifact for ArtifactImpl {{
    fn render(&self, _region: Region, _trace: bool, _profile: Profile) -> RenderSubtree {{
        RenderSubtree
    }}
}}

struct RenderImpl;

#[tool_implementation]
impl Render for RenderImpl {{
    fn render(
        &self,
        _region: Region,
        _trace: bool,
        _profile: Profile,
        request: ArtifactRequest,
        _inputs: Vec<PathBuf>,
        _format: ReportFormat,
        _tag: Vec<String>,
        _define: BTreeMap<String, i64>,
        _color: ColorMode,
        _checksum: bool,
        _verbose: u32,
        _stdin: Option<InputStream>,
        _stdout: OutputStream,
        _stderr: Option<OutputStream>,
    ) -> Result<ArtifactReport, RenderError> {{
        super::record("artifact:render;");
        Ok(ArtifactReport {{
            artifact_id: 41,
            digest: "deadbeef".to_string(),
            labels: request.labels,
            warnings: vec![request.source],
        }})
    }}

    fn status(
        &self,
        _region: Region,
        _trace: bool,
        _profile: Profile,
        artifact_id: u64,
    ) -> RenderStatus {{
        super::record(&format!("artifact:status:{{artifact_id}};"));
        RenderStatus::Ready
    }}
}}
"#
    )
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

fn discovered_definition(value: &Value) -> Tool {
    serde_json::from_value(value["tool"]["definition"].clone())
        .expect("tool get definition uses the public Tool schema")
}

#[test]
#[timeout("20m")]
async fn chunk_d_manifest_1_merge_matrix_executes_in_literal_order() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx, false);

    struct Case {
        name: &'static str,
        environment: Option<&'static [&'static str]>,
        agent: Option<&'static [&'static str]>,
        mode: MergeMode,
        expected_order: &'static str,
        expected_value: &'static str,
        expected_leaf: &'static str,
    }

    let cases = [
        Case {
            name: "omitted",
            environment: None,
            agent: None,
            mode: MergeMode::Omitted,
            expected_order: "U1;U2;leaf:input;",
            expected_value: "ok:leaf(input)",
            expected_leaf: "leaf:input;",
        },
        Case {
            name: "explicit-empty",
            environment: Some(&[]),
            agent: None,
            mode: MergeMode::Prepend,
            expected_order: "U1;U2;leaf:input;",
            expected_value: "ok:leaf(input)",
            expected_leaf: "leaf:input;",
        },
        Case {
            name: "explicit-prepend",
            environment: Some(&["E"]),
            agent: Some(&["A"]),
            mode: MergeMode::Prepend,
            expected_order: "U1;U2;A;E;leaf:E(A(input));",
            expected_value: "ok:A[E[leaf(E(A(input)))]]",
            expected_leaf: "leaf:E(A(input));",
        },
        Case {
            name: "default-prepend-duplicates",
            environment: Some(&["E1", "E2"]),
            agent: Some(&["A1", "A2"]),
            mode: MergeMode::Omitted,
            expected_order: "U1;U2;A1;A2;E1;E2;leaf:E2(E1(A2(A1(input))));",
            expected_value: "ok:A1[A2[E1[E2[leaf(E2(E1(A2(A1(input))))]]]]]",
            expected_leaf: "leaf:E2(E1(A2(A1(input))));",
        },
        Case {
            name: "append",
            environment: Some(&["E"]),
            agent: Some(&["A"]),
            mode: MergeMode::Append,
            expected_order: "U1;U2;E;A;leaf:A(E(input));",
            expected_value: "ok:E[A[leaf(A(E(input)))]]",
            expected_leaf: "leaf:A(E(input));",
        },
        Case {
            name: "replace",
            environment: Some(&["E"]),
            agent: Some(&["A"]),
            mode: MergeMode::Replace,
            expected_order: "U1;U2;A;leaf:A(input);",
            expected_value: "ok:A[leaf(A(input))]",
            expected_leaf: "leaf:A(input);",
        },
        Case {
            name: "replace-empty-keeps-universal",
            environment: Some(&["E"]),
            agent: Some(&[]),
            mode: MergeMode::Replace,
            expected_order: "U1;U2;leaf:input;",
            expected_value: "ok:leaf(input)",
            expected_leaf: "leaf:input;",
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
        assert_eq!(result.matches(case.expected_value).count(), 1, "{result}");
        let effects = invoke(&mut ctx, &agent, "effects", &[]).await;
        assert!(effects.contains(case.expected_order), "{effects}");
        assert_eq!(effects.matches(case.expected_order).count(), 1, "{effects}");
        assert_eq!(effects.matches(case.expected_leaf).count(), 1, "{effects}");
    }
}

#[test]
#[timeout("20m")]
async fn chunk_d_control_1_declared_error_retry_transform_and_replay_are_exact() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx, false);
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
    assert_eq!(
        result
            .matches("ok:transformed(reject(value))|leaf(retry(value))")
            .count(),
        1,
        "{result}"
    );
    let expected = "leaf:reject(value);leaf:retry(value);";
    let effects = invoke(&mut ctx, &agent, "effects", &[]).await;
    assert!(effects.contains(expected), "{effects}");
    assert_eq!(effects.matches("leaf:").count(), 2, "{effects}");

    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;
    let replayed = invoke(&mut ctx, &agent, "effects", &[]).await;
    assert!(replayed.contains(expected), "{replayed}");
    assert_eq!(replayed.matches("leaf:reject(value)").count(), 1);
    assert_eq!(replayed.matches("leaf:retry(value)").count(), 1);
    assert_eq!(replayed.matches("leaf:").count(), 2, "{replayed}");
}

#[test]
#[timeout("20m")]
async fn chunk_d_manifest_2_compatibility_modes_project_values_and_errors() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx, false);
    set_compatibility_case(
        &ctx,
        "compat-leaf",
        "structural-projection",
        "structural-subtype",
    );
    build(&mut ctx).await;
    ctx.start_server().await;
    deploy(&mut ctx).await;

    let projected_definition =
        serde_json::to_string(&tool_get_json(&mut ctx, "compat-leaf").await["tool"]["definition"])
            .unwrap();
    for expected in ["execute", "discarded", "rejected", "middleware-only"] {
        assert!(
            projected_definition.contains(expected),
            "{projected_definition}"
        );
    }
    for hidden_inner_detail in ["inspect", "leafOnly", "private-detail"] {
        assert!(
            !projected_definition.contains(hidden_inner_detail),
            "{projected_definition}"
        );
    }

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

    let structural_error = ctx
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
            r#"{"kept":"reject","discarded":"outer-only"}"#,
        ])
        .await;
    assert!(!structural_error.success_or_dump());
    let structural_error_text = format!(
        "{}\n{}",
        structural_error.stdout_text(),
        structural_error.stderr().collect::<Vec<_>>().join("\n")
    );
    assert!(
        structural_error_text.contains("rejected") && structural_error_text.contains("23"),
        "{structural_error_text}"
    );
    assert!(
        !structural_error_text.contains("private-detail")
            && !structural_error_text.contains("leafOnly"),
        "{structural_error_text}"
    );

    set_compatibility_case(
        &ctx,
        "compat-leaf",
        "structural-projection",
        "strict-equality",
    );
    let strict = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(!strict.success_or_dump());
    let strict_error = strict.stderr().collect::<Vec<_>>().join("\n");
    assert!(
        strict_error.contains("structural-projection"),
        "{strict_error}"
    );
    assert!(strict_error.contains("compat-leaf"), "{strict_error}");

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
async fn chunk_d_adapter_1_generates_client_from_presented_identity_but_binds_leaf_lookup() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx, false);
    require_presented_adapter_client(&ctx);
    build(&mut ctx).await;
    ctx.start_server().await;
    deploy(&mut ctx).await;

    let discovered = tool_get_json(&mut ctx, "compat-leaf").await;
    assert_eq!(discovered["tool"]["name"], "compat-leaf");
    assert_eq!(
        discovered["tool"]["definition"]["commands"]["nodes"][0]["name"],
        "presented-adapter"
    );

    let generated = std::fs::read_to_string(ctx.cwd_path_join(
        "golem-temp/bridge-sdk/rust/internal/compat-leaf-tool-guest-client/src/lib.rs",
    ))
    .unwrap();
    assert!(generated.contains("PresentedAdapterClient"), "{generated}");
    assert!(
        !generated.contains("pub struct CompatLeafClient"),
        "{generated}"
    );

    let agent = format!("adapter-{}", Uuid::new_v4());
    let constructor = format!("AdapterConformanceAgent(\"{agent}\")");
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
    let result = invoke_agent(
        &mut ctx,
        "AdapterConformanceAgent",
        &agent,
        "invoke",
        &["\"through-adapter\""],
    )
    .await;
    assert!(result.contains("ok:leaf: through-adapter"), "{result}");
}

#[test]
#[timeout("20m")]
async fn chunk_d_transparency_1_preserves_contract_1_and_executes_universal_layer() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx, true);
    configure_manifest(&ctx, &[], None, Some(&[]), MergeMode::Replace);
    build(&mut ctx).await;
    ctx.start_server().await;
    deploy(&mut ctx).await;
    let before = tool_get_json(&mut ctx, "artifact").await;

    configure_manifest(&ctx, &["TRANSPARENT"], None, Some(&[]), MergeMode::Replace);
    deploy(&mut ctx).await;
    let after = tool_get_json(&mut ctx, "artifact").await;
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
    metadata_oracle::assert_complete_contract(
        "before universal middleware",
        &contract["tool"],
        &discovered_definition(&before),
    );
    metadata_oracle::assert_complete_contract(
        "after universal middleware",
        &contract["tool"],
        &discovered_definition(&after),
    );

    let agent = format!("transparent-{}", Uuid::new_v4());
    let constructor = format!("MiddlewareConformanceAgent(\"{agent}\")");
    let result = ctx
        .cli([
            flag::FORMAT,
            "json",
            cmd::TOOL,
            cmd::INVOKE,
            "--agent",
            &constructor,
            "artifact",
            "--",
            "render",
            "status",
            "41",
        ])
        .await;
    assert!(result.success_or_dump());
    assert!(result.stdout_text().contains("ready"));
    let effects = invoke(&mut ctx, &agent, "effects", &[]).await;
    assert!(
        effects.contains("TRANSPARENT;artifact:status:41;"),
        "{effects}"
    );
    assert_eq!(effects.matches("TRANSPARENT;").count(), 1, "{effects}");
    assert_eq!(
        effects.matches("artifact:status:41;").count(),
        1,
        "{effects}"
    );
}
