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

use crate::Tracing;
use crate::app::{Output, TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

#[test]
#[timeout("15 minutes")]
async fn installed_package_has_no_http_route_until_declared() -> anyhow::Result<()> {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "publisher", flag::TEMPLATE, "ts"])
            .await
            .success_or_dump()
    );
    ctx.cd("publisher");
    let http_api = indoc::indoc! {r#"
        httpApi:
          deployments:
            local:
              - domain: localhost:9006
                scheme: http
                agents:
                  CounterAgent: {}
    "#};
    let publisher = formatdoc! {r#"
        manifestVersion: {}
        app: publisher
        package: {{ name: publisher, version: 1.0.0 }}
        environments:
          local:
            server: local
            componentPresets: quick
        components:
          publisher:main:
            templates: ts
        {http_api}
    "#, versions::sdk::MANIFEST};
    fs::write_str(ctx.cwd_path_join("golem.yaml"), publisher)?;
    assert!(ctx.cli([cmd::BUILD, flag::YES]).await.success_or_dump());
    let pack = ctx.cli(["package", "pack"]).await;
    assert!(pack.success_or_dump());
    assert!(pack.stderr_contains(format!(
        "Omitted HTTP API export: localhost:{}",
        ctx.custom_request_port()
    )));
    let archive = ctx.cwd_path_join("golem-package.tar");

    ctx.cd("..");
    fs::create_dir_all(ctx.cwd_path_join("installer"))?;
    ctx.cd("installer");
    std::fs::copy(archive, ctx.cwd_path_join("publisher.tar"))?;
    let manifest = ctx.cwd_path_join("golem.yaml");
    fs::write_str(
        &manifest,
        formatdoc! {r#"
            manifestVersion: {}
            app: installer
            environments:
              local:
                server: local
        "#, versions::sdk::MANIFEST},
    )?;
    assert!(
        ctx.cli(["install", "./publisher.tar"])
            .await
            .success_or_dump()
    );
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let route = format!(
        "http://localhost:{}/counters/probe/value",
        ctx.custom_request_port()
    );
    let client = reqwest::Client::new();
    assert_eq!(client.get(&route).send().await?.status(), 404);

    let installed = fs::read_to_string(&manifest)?;
    fs::write_str(&manifest, format!("{installed}\n{http_api}"))?;
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let response = client.get(&route).send().await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<f64>().await?, 0.0);
    Ok(())
}

#[test]
#[timeout("15 minutes")]
async fn installed_tool_deploy_uploads_the_original_package_layer() -> anyhow::Result<()> {
    use golem_client::api::{RegistryServiceClient, RegistryServiceClientLive};
    use golem_client::{Context, Security};
    use golem_common::schema::ExternalTypedSchemaValue;
    use golem_schema::render::json_value::to_json_value;
    use serde_json::{Value, json};

    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("package-deploy"))?;
    ctx.cd("package-deploy");
    let component_name = "package-deploy:tool";
    assert!(
        ctx.cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "rust",
            flag::COMPONENT_NAME,
            component_name,
        ])
        .await
        .success_or_dump()
    );
    assert!(
        ctx.cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "rust",
            flag::COMPONENT_NAME,
            "package-deploy:owner",
        ])
        .await
        .success_or_dump()
    );
    let manifest = formatdoc! {r#"
        manifestVersion: {}
        app: package-deploy
        environments:
          local:
            server: local
            componentPresets: debug
            tools:
              defaults: []
    "#, versions::sdk::MANIFEST};
    let manifest_path = ctx.cwd_path_join("golem.yaml");
    let publisher = indoc::indoc! {r#"
        package: { name: package-deploy, version: 1.0.0 }
        tools:
          echo-tool: {}
    "#};
    let tool_component = "  package-deploy:tool:\n    dir: tool\n    templates: rust\n";
    fs::write_str(
        &manifest_path,
        format!("{manifest}{publisher}components:\n{tool_component}"),
    )?;
    fs::write_str(
        ctx.cwd_path_join("tool/src/counter_agent.rs"),
        include_str!("../../../../test-components/size-analysis/fixtures/tool/src/lib.rs"),
    )?;
    assert!(ctx.cli([cmd::BUILD, flag::YES]).await.success_or_dump());
    assert!(ctx.cli(["package", "pack"]).await.success_or_dump());
    let package =
        golem_cli::package::read(std::fs::File::open(ctx.cwd_path_join("golem-package.tar"))?)?;
    let name = golem_common::model::component::ComponentName::try_from(component_name)
        .map_err(anyhow::Error::msg)?;
    let layer_digest = &package.config().components[&name].wasm;
    let layer = package.blob(layer_digest).unwrap();

    let archive = ctx.cwd_path_join("golem-package.tar");
    let (_oci, host) = crate::package::start_registry().await?;
    let reference = format!("{host}/packages/tool:1.0");
    let oci_source = format!("oci://{reference}");
    crate::package::push_package(&reference, &std::fs::read(&archive)?).await?;
    let in_root = "packages/golem-package.tar";
    let first_outside = "../first/golem-package.tar";
    let second_outside = "../second/golem-package.tar";
    for source in [in_root, first_outside, second_outside] {
        let path = ctx.cwd_path_join(source);
        fs::create_dir_all(path.parent().unwrap())?;
        std::fs::copy(&archive, path)?;
    }
    #[cfg(not(windows))]
    let app_root = std::fs::canonicalize(ctx.cwd_path())?;
    #[cfg(windows)]
    let app_root = ctx.cwd_path();

    let registry = RegistryServiceClientLive {
        context: Context {
            client: reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build(),
            base_url: ctx.worker_service_url().parse()?,
            security_token: Security::Bearer(golem_client::LOCAL_WELL_KNOWN_TOKEN.to_string()),
        },
    };
    let mut previous_revision = None;
    let mut local_result = None;
    // The invocation owner is part of the consuming app, not the package.
    let owner = indoc::indoc! {r#"
        components:
          package-deploy:owner:
            dir: owner
            templates: rust
            tools:
              echo-tool: {}
    "#};
    for (source, expected_provenance, provenance_changed) in [
        (None, None, false),
        (Some(in_root), Some(in_root), true),
        (Some(first_outside), Some("golem-package.tar"), true),
        (Some(second_outside), Some("golem-package.tar"), false),
        (Some(oci_source.as_str()), Some(oci_source.as_str()), true),
    ] {
        if let Some(source) = source {
            fs::write_str(&manifest_path, format!("{manifest}{owner}"))?;
            assert!(ctx.cli(["install", source]).await.success_or_dump());
            let lock: serde_yaml::Value =
                serde_yaml::from_str(&fs::read_to_string(ctx.cwd_path_join("golem.lock"))?)?;
            let locked_source: std::path::PathBuf =
                if let Some(outside) = source.strip_prefix("../") {
                    let mut path = app_root.parent().unwrap().to_path_buf();
                    path.extend(outside.split('/'));
                    path
                } else {
                    source.split('/').collect()
                };
            let pin = &lock["packages"]["package-deploy"];
            if source == oci_source {
                assert_eq!(pin["source"].as_str(), Some(oci_source.as_str()));
                assert_eq!(pin["digest"].as_str(), Some(package.digest()));
                std::fs::remove_dir_all(ctx.config_dir.path().join("package-cache"))?;
            } else {
                assert_eq!(
                    pin["source"].as_str(),
                    Some(fs::path_to_str(&locked_source)?)
                );
            }
            assert!(ctx.cli([cmd::BUILD, flag::YES]).await.success_or_dump());
        } else {
            fs::write_str(
                &manifest_path,
                format!("{manifest}{publisher}{owner}{tool_component}"),
            )?;
        }
        let deploy = ctx.cli(["--format", "json", cmd::DEPLOY, flag::YES]).await;
        assert!(deploy.success_or_dump());
        let documents = deploy.stdout_json::<Value>();
        let plan = documents.iter().find(|doc| doc["$type"] == "deploy.plan");
        if provenance_changed {
            // Compare the entire deployment diff, not only the installed component.
            assert_eq!(
                plan.expect("missing deployment plan")["deploymentDiff"],
                json!({
                    "components": {
                        (component_name): {
                            "wasmChanged": false,
                            "packageMetadataChanged": true,
                            "componentConfigChanged": false,
                        }
                    },
                    "publishedToolMiddlewares": {},
                    "universalToolMiddlewaresChanged": false,
                    "toolCompatibilityModeChanged": false,
                    "environmentToolMiddlewareBindings": {},
                    "agentToolMiddlewareBindings": {},
                })
            );
        } else if source.is_some() {
            assert!(plan.is_none(), "Unexpected deployment plan: {plan:#?}");
        }
        let component = ctx
            .cli_with_input(
                ["--format", "json", cmd::COMPONENT, cmd::GET, component_name],
                &[],
            )
            .await;
        assert!(
            component.success(),
            "{}",
            String::from_utf8_lossy(&component.stderr)
        );
        let view: serde_json::Value = serde_json::from_slice(component.stdout())?;
        let id = serde_json::from_value(view["componentId"].clone())?;
        let component = registry.get_component(&id).await?;
        let uploaded = registry
            .get_component_wasm(&id, component.revision.into())
            .await?;
        assert_eq!(uploaded.as_ref(), layer);
        let revision = component.revision;
        if let Some(expected_source) = expected_provenance {
            if provenance_changed {
                assert!(revision > previous_revision.unwrap());
            } else {
                assert_eq!(revision, previous_revision.unwrap());
            }
            let origin = serde_json::to_value(component)?;
            assert_eq!(origin["packageMetadata"]["source"], expected_source);
            assert_eq!(origin["packageMetadata"]["packageDigest"], package.digest());
            assert_eq!(
                origin["packageMetadata"]["layerDigest"],
                layer_digest.as_str()
            );
        } else {
            assert!(
                serde_json::to_value(component)?
                    .get("packageMetadata")
                    .is_none()
            );
        }
        previous_revision = Some(revision);

        let invocation = ctx
            .cli_with_input(
                [
                    "--format",
                    "json",
                    cmd::TOOL,
                    cmd::INVOKE,
                    "--component",
                    "package-deploy:owner",
                    "echo-tool",
                    "--",
                    "echo",
                    "Hello from a tool",
                ],
                &[],
            )
            .await;
        assert!(
            invocation.success(),
            "{}",
            String::from_utf8_lossy(&invocation.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(invocation.stdout())?;
        let result = &response["response"]["result"];
        assert_eq!(result["type"], "Success", "{response:#}");
        let typed: ExternalTypedSchemaValue = serde_json::from_value(result["result"].clone())?;
        let typed = typed.as_inner();
        let result = to_json_value(typed.graph(), &typed.graph().root, typed.value())?;
        assert_eq!(result, "Hello from a tool");
        if source.is_some() {
            assert_eq!(Some(&result), local_result.as_ref());
        } else {
            local_result = Some(result);
        }
    }

    Ok(())
}

const STAGE_IS_SAME: &str =
    "staging area with current deployment (tool publications excluded): SAME";
const STAGE_IS_DIFFERENT: &str =
    "staging area with current deployment (tool publications excluded): DIFFERENT";

fn output_contains(output: &Output, text: &str) -> bool {
    output.stdout_contains(text) || output.stderr_contains(text)
}

/// Writes the manifest of an application with one component. With `with_api_deployments`, the
/// manifest also has one HTTP API deployment and one MCP deployment of the counter agent.
fn write_manifest(
    ctx: &TestContext,
    version: &str,
    version_check: bool,
    with_api_deployments: bool,
) {
    let api_deployments = if with_api_deployments {
        indoc::indoc! {r#"
            httpApi:
              deployments:
                local:
                  - subdomain: staging-probe
                    agents:
                      CounterAgent: {}
            mcp:
              deployments:
                local:
                  - subdomain: staging-probe-mcp
                    agents:
                      CounterAgent: {}
        "#}
    } else {
        ""
    };
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {manifest_version}
            app: staging-probe
            version: {version}
            environments:
              local:
                server: local
                componentPresets: quick
                deployment:
                  versionCheck: {version_check}
            components:
              staging-probe:main:
                templates: ts
            {api_deployments}
        "#, manifest_version = versions::sdk::MANIFEST},
    )
    .unwrap();
}

/// Creates the application and deploys it with its HTTP API deployment and MCP deployment.
async fn deploy_app_with_api_deployments(ctx: &mut TestContext, version_check: bool) {
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "staging-probe", flag::TEMPLATE, "ts"])
            .await
            .success_or_dump()
    );
    ctx.cd("staging-probe");
    write_manifest(ctx, "v1.0.0", version_check, true);
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
}

#[test]
#[timeout("10 minutes")]
async fn deploy_rejects_an_existing_version_before_it_changes_the_staging_area() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    deploy_app_with_api_deployments(&mut ctx, true).await;

    write_manifest(&ctx, "v1.0.0", true, false);

    let plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(!plan.success(), "plan must report the version conflict");
    assert!(output_contains(
        &plan,
        "Deployment version v1.0.0 already exists in the environment"
    ));

    let deploy = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(!deploy.success(), "deploy must reject the version conflict");
    assert!(output_contains(
        &deploy,
        "Deployment version v1.0.0 already exists in the environment"
    ));
    assert!(!output_contains(&deploy, "changes to the staging area"));

    write_manifest(&ctx, "v1.0.1", true, false);

    let plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(plan.success_or_dump());
    assert!(output_contains(&plan, STAGE_IS_SAME));

    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
}

#[test]
#[timeout("10 minutes")]
async fn stage_accepts_an_existing_version() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    deploy_app_with_api_deployments(&mut ctx, true).await;

    write_manifest(&ctx, "v1.0.0", true, false);
    assert!(
        ctx.cli([cmd::DEPLOY, "--stage", flag::YES])
            .await
            .success_or_dump()
    );

    let plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(!plan.success(), "plan must report the version conflict");
    assert!(output_contains(&plan, STAGE_IS_DIFFERENT));
    assert!(output_contains(
        &plan,
        "Deployment version v1.0.0 already exists in the environment"
    ));
}

#[test]
#[timeout("10 minutes")]
async fn deploy_succeeds_when_the_staging_area_deleted_entities_of_the_current_deployment() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    deploy_app_with_api_deployments(&mut ctx, false).await;

    write_manifest(&ctx, "v1.0.1", false, false);
    assert!(
        ctx.cli([cmd::DEPLOY, "--stage", flag::YES])
            .await
            .success_or_dump()
    );

    let plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(plan.success_or_dump());
    assert!(output_contains(&plan, STAGE_IS_DIFFERENT));
    assert!(output_contains(&plan, "delete HTTP API deployment"));
    assert!(output_contains(&plan, "delete MCP deployment"));

    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(plan.success_or_dump());
    assert!(output_contains(
        &plan,
        "no changes are required for the environment"
    ));
}

#[test]
#[timeout("10 minutes")]
async fn rollback_restores_entities_that_the_staging_area_deleted() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    deploy_app_with_api_deployments(&mut ctx, false).await;

    write_manifest(&ctx, "v1.0.1", false, false);
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    assert!(
        ctx.cli([cmd::DEPLOY, "--version", "v1.0.0", flag::YES])
            .await
            .success_or_dump()
    );

    let plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(plan.success_or_dump());
    assert!(output_contains(&plan, STAGE_IS_DIFFERENT));
    assert!(output_contains(&plan, "delete HTTP API deployment"));
    assert!(output_contains(&plan, "delete MCP deployment"));

    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
}
