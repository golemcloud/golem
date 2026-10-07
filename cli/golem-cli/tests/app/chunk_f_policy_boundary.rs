use super::{TestContext, cmd, flag};
use crate::workspace_path;
use serde_json::{Value, json};
use test_r::{test, timeout};
use uuid::Uuid;

const FIXTURE: &str = "chunk-f-policy-boundary";

fn prepare_fixture(ctx: &mut TestContext) {
    fs_extra::dir::copy(
        ctx.test_data_path_join(FIXTURE),
        ctx.cwd_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();
    ctx.cd(FIXTURE);

    let sdk = workspace_path().join("sdks/rust/golem-rust");
    for component in ["provider", "caller"] {
        let manifest = ctx.cwd_path_join(format!("{component}/Cargo.toml"));
        let contents = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            manifest,
            contents.replace("__GOLEM_RUST_PATH__", sdk.to_str().unwrap()),
        )
        .unwrap();
    }
}

fn update_manifest(ctx: &TestContext, update: impl FnOnce(&mut Value)) {
    let path = ctx.cwd_path_join("golem.yaml");
    let mut manifest: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    update(&mut manifest);
    std::fs::write(path, serde_yaml::to_string(&manifest).unwrap()).unwrap();
}

async fn build(ctx: &mut TestContext) {
    let output = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(output.success_or_dump());
}

async fn assert_provider_has_no_secret_imports(ctx: &TestContext) {
    let component = ctx.cwd_path_join("golem-temp/agents/chunk_f_provider_release.wasm");
    let output = tokio::process::Command::new("wasm-tools")
        .args(["component", "wit"])
        .arg(component)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let wit = String::from_utf8(output.stdout).unwrap();
    assert!(
        !wit.contains("golem:secrets/"),
        "a fixture with no secret operations must not import secret host interfaces:\n{wit}"
    );
}

async fn deploy(ctx: &mut TestContext) -> super::Output {
    ctx.cli([flag::YES, cmd::DEPLOY]).await
}

async fn create_agent(ctx: &mut TestContext, name: &str) {
    let constructor = format!("ChunkFPolicyAgent(\"{name}\")");
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::NEW,
            &constructor,
            "--config",
            "allowed=\"owner-allowed\"",
            "--config",
            "denied=\"owner-denied\"",
            "--config",
            "outside=\"owner-outside\"",
        ])
        .await;
    assert!(output.success_or_dump());
}

async fn invoke(ctx: &mut TestContext, name: &str, method: &str, args: &[&str]) -> String {
    let constructor = format!("ChunkFPolicyAgent(\"{name}\")");
    let output = ctx
        .cli(
            [flag::YES, cmd::AGENT, cmd::INVOKE, &constructor, method]
                .into_iter()
                .chain(args.iter().copied()),
        )
        .await;
    assert!(output.success_or_dump());
    output.stdout_text()
}

#[test]
#[timeout("20m")]
async fn chunk_f_config_manifest_runtime_matrix_intersects_inherited_concrete_empty_and_broadened_scopes()
 {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);
    build(&mut ctx).await;
    assert_provider_has_no_secret_imports(&ctx).await;
    ctx.start_server().await;

    struct Case {
        name: &'static str,
        environment: Value,
        agent: Option<Value>,
        expected_allowed: &'static str,
        expected_denied: &'static str,
        expected_outside: &'static str,
    }

    let cases = [
        Case {
            name: "inherited",
            environment: json!(["allowed", "denied"]),
            agent: None,
            expected_allowed: "owner-allowed",
            expected_denied: "owner-denied",
            expected_outside: "denied",
        },
        Case {
            name: "concrete",
            environment: json!(["allowed", "denied"]),
            agent: Some(json!(["allowed"])),
            expected_allowed: "owner-allowed",
            expected_denied: "denied",
            expected_outside: "denied",
        },
        Case {
            name: "empty",
            environment: json!(["allowed", "denied"]),
            agent: Some(json!([])),
            expected_allowed: "denied",
            expected_denied: "denied",
            expected_outside: "denied",
        },
        Case {
            name: "attempted-broadening",
            environment: json!(["allowed"]),
            agent: Some(json!("*")),
            expected_allowed: "owner-allowed",
            expected_denied: "denied",
            expected_outside: "denied",
        },
    ];

    for case in cases {
        update_manifest(&ctx, |manifest| {
            let environment =
                &mut manifest["environments"]["local"]["tools"]["chunk-f-config-probe"];
            environment["configKeysReadable"] = case.environment.clone();
            let agent =
                &mut manifest["agents"]["ChunkFPolicyAgent"]["tools"]["chunk-f-config-probe"];
            match case.agent.clone() {
                Some(scope) => agent["configKeysReadable"] = scope,
                None => {
                    agent.as_object_mut().unwrap().remove("configKeysReadable");
                }
            }
        });
        let deployed = deploy(&mut ctx).await;
        assert!(deployed.success_or_dump());
        let name = format!("{}-{}", case.name, Uuid::new_v4());
        create_agent(&mut ctx, &name).await;
        for (key, expected) in [
            ("allowed", case.expected_allowed),
            ("denied", case.expected_denied),
            ("outside", case.expected_outside),
        ] {
            let key_arg = format!("\"{key}\"");
            let result = invoke(&mut ctx, &name, "read_config", &[&key_arg]).await;
            assert!(
                result.contains(expected),
                "{case_name}/{key}: {result}",
                case_name = case.name
            );
        }
    }
}

#[test]
#[timeout("20m")]
async fn chunk_f_filesystem_manifest_runtime_matrix_enforces_policy_provisioning_and_owner_root() {
    let mut ctx = TestContext::new();
    prepare_fixture(&mut ctx);
    build(&mut ctx).await;
    ctx.start_server().await;

    let mut failures = Vec::new();
    for (access, provisioned, deployment_succeeds) in [
        ("allowed", false, true),
        ("allowed", true, true),
        ("unset", false, false),
        ("unset", true, true),
        ("denied", false, false),
        ("denied", true, false),
    ] {
        update_manifest(&ctx, |manifest| {
            if access == "unset" {
                manifest["environments"]["local"]["tools"]["chunk-f-filesystem-probe"]
                    .as_object_mut()
                    .unwrap()
                    .remove("filesystemAccess");
                manifest["agents"]["ChunkFPolicyAgent"]["tools"]["chunk-f-filesystem-probe"]
                    .as_object_mut()
                    .unwrap()
                    .remove("filesystemAccess");
            } else {
                manifest["environments"]["local"]["tools"]["chunk-f-filesystem-probe"]["filesystemAccess"] =
                    json!("allowed");
                manifest["agents"]["ChunkFPolicyAgent"]["tools"]["chunk-f-filesystem-probe"]["filesystemAccess"] =
                    json!(access);
            }
            let provider = &mut manifest["components"]["chunk-f:provider"];
            if provisioned {
                provider["files"] = json!([{
                    "sourcePath": "provisioned.txt",
                    "targetPath": "/chunk-f-provisioned.txt",
                    "permissions": "read-only"
                }]);
            } else {
                provider.as_object_mut().unwrap().remove("files");
            }
        });
        let deployed = deploy(&mut ctx).await;
        let deployed_successfully = deployed.success_or_dump();
        if deployed_successfully != deployment_succeeds {
            failures.push(format!(
                "access={access}, provisioned={provisioned}: expected deployment success={deployment_succeeds}, got {deployed_successfully}"
            ));
        }
        if deployed_successfully && deployment_succeeds {
            let name = format!("filesystem-{access}-{provisioned}-{}", Uuid::new_v4());
            create_agent(&mut ctx, &name).await;
            let value = format!("owner-root-{access}-{provisioned}");
            let value_arg = format!("\"{value}\"");
            let result = invoke(
                &mut ctx,
                &name,
                "filesystem_roundtrip",
                &["\"/chunk-f-owner-root.txt\"", &value_arg],
            )
            .await;
            assert!(result.contains(&value), "{result}");
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
