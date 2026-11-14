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
