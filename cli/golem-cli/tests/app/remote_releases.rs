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

use crate::app::{TestContext, cmd, flag};
use crate::{Tracing, workspace_path};
use chrono::{DateTime, Utc};
use golem_cli::fs;
use golem_client::api::{RegistryServiceClient, RegistryServiceClientLive};
use golem_client::model::{DeploymentCreation, TokenCreation};
use golem_client::{Context, Security};
use golem_common::model::account::{AccountCreation, AccountEmail, AccountId};
use golem_common::model::application::{Application, ApplicationCreation, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{
    ComponentCreation, ComponentName, ToolDeploymentConfigCreation, ToolProvisionConfigCreation,
};
use golem_common::model::deployment::DeploymentVersion;
use golem_common::model::diff::Hashable;
use golem_common::model::environment::{Environment, EnvironmentCreation, EnvironmentName};
use golem_common::model::environment_tool_grant::EnvironmentToolGrantCreation;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::tool::ToolName;
use golem_common::model::tool_release::{ToolReleaseById, ToolReleaseReference};
use golem_common::schema::SchemaGraph;
use golem_common::schema::tool::{
    CommandBody, CommandNode, CommandTree, Doc, Globals, Positionals, Tool,
};
use reqwest_middleware::ClientBuilder;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use test_r::{inherit_test_dep, test, timeout};
use tokio::fs::File;
use url::Url;
use uuid::Uuid;

inherit_test_dep!(Tracing);

struct RegistryUser {
    account_id: AccountId,
    account_email: AccountEmail,
    token: TokenSecret,
    client: RegistryServiceClientLive,
}

fn registry_client(base_url: &Url, token: &str) -> RegistryServiceClientLive {
    RegistryServiceClientLive {
        context: Context {
            client: ClientBuilder::new(reqwest::Client::new()).build(),
            base_url: base_url.clone(),
            security_token: Security::Bearer(token.to_string()),
        },
    }
}

async fn create_registry_user(
    admin: &RegistryServiceClientLive,
    base_url: &Url,
    label: &str,
) -> anyhow::Result<RegistryUser> {
    let name = format!("{label}-{}", Uuid::new_v4());
    let account = admin
        .create_account(&AccountCreation {
            name: name.clone(),
            email: AccountEmail::new(format!("{name}@golem.cloud")),
            roles: Vec::new(),
        })
        .await?;
    let token = admin
        .create_token(
            &account.id.0,
            &TokenCreation {
                expires_at: DateTime::<Utc>::MAX_UTC,
            },
        )
        .await?
        .secret;
    let client = registry_client(base_url, token.secret());

    Ok(RegistryUser {
        account_id: account.id,
        account_email: account.email,
        token,
        client,
    })
}

async fn create_app_and_environment(
    user: &RegistryUser,
    label: &str,
) -> anyhow::Result<(Application, Environment)> {
    let application = user
        .client
        .create_application(
            &user.account_id.0,
            &ApplicationCreation {
                name: ApplicationName(format!("{label}-app-{}", Uuid::new_v4())),
            },
        )
        .await?;
    let environment = user
        .client
        .create_environment(
            &application.id.0,
            &EnvironmentCreation {
                name: EnvironmentName(format!("{label}-env-{}", Uuid::new_v4())),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
        )
        .await?;
    Ok((application, environment))
}

fn remote_release_tool(version: &str) -> Tool {
    Tool {
        version: version.to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "search".to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: Some(CommandBody {
                    positionals: Positionals::default(),
                    options: Vec::new(),
                    flags: Vec::new(),
                    constraints: Vec::new(),
                    stdin: None,
                    stdout: None,
                    result: None,
                    errors: Vec::new(),
                    annotations: None,
                }),
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

fn publisher_tool_config() -> ToolDeploymentConfigCreation {
    ToolDeploymentConfigCreation {
        provision: ToolProvisionConfigCreation {
            config: NormalizedJsonValue::new(serde_json::json!({})),
            env: BTreeMap::new(),
            plugin_installations: Vec::new(),
            files: BTreeMap::new(),
        },
        environment_binding: None,
        agent_bindings: BTreeMap::new(),
    }
}

fn wasm_files_under(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut wasm_files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension() == Some(OsStr::new("wasm")) {
                wasm_files.push(path);
            }
        }
    }
    Ok(wasm_files)
}

#[test]
#[timeout("2m")]
async fn remote_release_bridge_automatically_reconciles_its_environment_grant(
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut ctx = TestContext::new();
    ctx.start_server().await;

    let base_url = Url::parse(&format!("http://localhost:{}", ctx.router_port()))?;
    let admin = registry_client(&base_url, golem_client::LOCAL_WELL_KNOWN_TOKEN);
    let publisher = create_registry_user(&admin, &base_url, "tool-publisher").await?;
    let consumer = create_registry_user(&admin, &base_url, "tool-consumer").await?;
    let (_, publisher_environment) =
        create_app_and_environment(&publisher, "tool-publisher").await?;
    let (consumer_application, consumer_environment) =
        create_app_and_environment(&consumer, "tool-consumer").await?;
    let tool_name = ToolName::try_from("search").unwrap();

    let component_wasm =
        workspace_path().join("sdks/ts/packages/golem-ts-sdk/wasm/agent_guest.wasm");
    publisher
        .client
        .create_component(
            &publisher_environment.id.0,
            &ComponentCreation {
                component_name: ComponentName::try_from("publisher-tools:search")
                    .map_err(anyhow::Error::msg)?,
                agent_types: Vec::new(),
                agent_type_provision_configs: BTreeMap::new(),
                tools: vec![remote_release_tool("1.2.0")],
                tool_deployment_configs: BTreeMap::from([(
                    tool_name.clone(),
                    publisher_tool_config(),
                )]),
            },
            File::open(&component_wasm).await?,
            None::<File>,
        )
        .await?;

    let publisher_plan = publisher
        .client
        .get_environment_deployment_plan(&publisher_environment.id.0)
        .await?;
    let mut publisher_hash_input = publisher_plan.to_diffable();
    publisher_hash_input
        .published_tools
        .insert(tool_name.to_string());
    publisher
        .client
        .deploy_environment(
            &publisher_environment.id.0,
            &DeploymentCreation {
                current_revision: publisher_plan.current_revision,
                expected_deployment_hash: publisher_hash_input.hash()?,
                version: DeploymentVersion("publisher-1.2.0".to_string()),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                publish_tools: vec![tool_name.clone()],
                remote_tools: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;

    let release = publisher
        .client
        .list_account_tool_releases(&publisher.account_id.0)
        .await?
        .values
        .into_iter()
        .find(|release| release.name == tool_name && release.version == "1.2.0")
        .expect("publisher deployment must create search@1.2.0");

    let yaml_string = |value: &str| serde_json::to_string(value).unwrap();
    let consumer_manifest = format!(
        r#"manifestVersion: 1.6.0
app: {application}

tools:
  search:
    release:
      account: {publisher_account}
      name: search
      version: "1.2.0"

environments:
  {environment}:
    server:
      url: {server_url}
      workerUrl: {server_url}
      allowInsecure: true
      auth:
        staticToken: {token}

bridge:
  rust:
    internal:
      outputDir: generated
      tools: [search]
"#,
        application = yaml_string(&consumer_application.name.0),
        environment = yaml_string(&consumer_environment.name.0),
        server_url = yaml_string(base_url.as_str()),
        token = yaml_string(consumer.token.secret()),
        publisher_account = yaml_string(publisher.account_email.as_str()),
    );
    fs::write_str(ctx.cwd_path_join("golem.yaml"), &consumer_manifest)?;

    let staged_deployment = ctx.cli([cmd::DEPLOY, "--stage"]).await;
    assert!(!staged_deployment.success());
    assert!(
        staged_deployment.stdout_contains("requires environment tool grant changes")
            || staged_deployment.stderr_contains("requires environment tool grant changes")
    );
    assert!(
        consumer
            .client
            .list_environment_tool_grants(&consumer_environment.id.0)
            .await?
            .values
            .is_empty(),
        "staging must not create an environment grant"
    );

    let deployment_plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(deployment_plan.success_or_dump());
    assert!(
        deployment_plan.stdout_contains("environment tool grant reconciliation")
            || deployment_plan.stderr_contains("environment tool grant reconciliation")
    );
    assert!(
        deployment_plan.stdout_contains("Planning stopped")
            || deployment_plan.stderr_contains("Planning stopped")
    );
    assert!(
        deployment_plan.stdout_contains("--plan does not apply them")
            || deployment_plan.stderr_contains("--plan does not apply them")
    );
    assert!(
        consumer
            .client
            .list_environment_tool_grants(&consumer_environment.id.0)
            .await?
            .values
            .is_empty(),
        "planning must not create the automatic grant"
    );

    let granted_build = ctx
        .cli([flag::YES, cmd::BUILD, flag::STEP, "gen-bridge"])
        .await;
    assert!(granted_build.success_or_dump());
    assert!(
        granted_build.stdout_contains("environment tool grants required by the build")
            || granted_build.stderr_contains("environment tool grants required by the build")
    );
    assert!(
        granted_build.stdout_contains("remote tool bridge access through the selected environment")
            || granted_build
                .stderr_contains("remote tool bridge access through the selected environment")
    );
    assert!(
        granted_build.stdout_contains("Committed environment tool grant setup")
            || granted_build.stderr_contains("Committed environment tool grant setup")
    );
    let grants = consumer
        .client
        .list_environment_tool_grants(&consumer_environment.id.0)
        .await?
        .values;
    assert_eq!(grants.len(), 1);
    let grant = &grants[0];
    assert!(grant.grant.automatic);
    assert_eq!(grant.release.id, release.id);
    assert!(
        ctx.cwd_path_join("generated/search-tool-guest-client/Cargo.toml")
            .is_file()
    );
    assert!(
        wasm_files_under(ctx.cwd_path())?.is_empty(),
        "remote-release bridge generation must not fetch or require publisher WASM"
    );

    let marker_dir = ctx.cwd_path_join("golem-temp/task-results");
    let mut remote_release_bridge_marker = None;
    for entry in std::fs::read_dir(&marker_dir)? {
        let marker: serde_json::Value = serde_json::from_slice(&std::fs::read(entry?.path())?)?;
        if marker.get("kind").and_then(serde_json::Value::as_str)
            == Some("GenerateBridgeSdkMarkerHash")
        {
            remote_release_bridge_marker = Some(marker);
            break;
        }
    }
    let marker_input = remote_release_bridge_marker
        .expect("remote-release bridge generation must write a cache marker")
        .get("hashInput")
        .and_then(serde_json::Value::as_str)
        .expect("remote-release bridge cache marker must retain its hash input")
        .to_string();
    for expected in [
        release.id.to_string(),
        release.metadata_version,
        release.metadata_digest.to_string(),
        grant.release.source_digest.to_string(),
    ] {
        assert!(
            marker_input.contains(&expected),
            "remote-release bridge cache identity must include {expected}: {marker_input}"
        );
    }

    consumer
        .client
        .delete_automatic_environment_tool_grant(&grant.grant.id.0)
        .await?;
    let deployment = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployment.success_or_dump());
    assert!(deployment.stdout_contains_ordered([
        "Planning environment tool grant reconciliation",
        "Applying environment tool grant setup as a separate committed step",
        "Committed environment tool grant setup",
        "Preparing deployment",
        "Deploying staged changes to the environment",
    ]));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        format!(
            r#"manifestVersion: 1.6.0
app: {application}

environments:
  {environment}:
    server:
      url: {server_url}
      workerUrl: {server_url}
      allowInsecure: true
      auth:
        staticToken: {token}
"#,
            application = yaml_string(&consumer_application.name.0),
            environment = yaml_string(&consumer_environment.name.0),
            server_url = yaml_string(base_url.as_str()),
            token = yaml_string(consumer.token.secret()),
        ),
    )?;
    let deletion_only_plan = ctx.cli([cmd::DEPLOY, "--plan"]).await;
    assert!(deletion_only_plan.success_or_dump());
    assert!(deletion_only_plan.stdout_contains_ordered([
        "Planning environment tool grant reconciliation",
        "Preparing deployment",
    ]));
    assert!(
        !deletion_only_plan.stdout_contains("Planning stopped")
            && !deletion_only_plan.stderr_contains("Planning stopped"),
        "grant cleanup must not prevent the remaining deployment plan"
    );
    let stale_grants = consumer
        .client
        .list_environment_tool_grants(&consumer_environment.id.0)
        .await?
        .values;
    assert_eq!(
        stale_grants.len(),
        1,
        "planning must not delete stale grants"
    );
    assert!(stale_grants[0].grant.automatic);
    fs::write_str(ctx.cwd_path_join("golem.yaml"), &consumer_manifest)?;

    let administrator_managed = consumer
        .client
        .create_environment_tool_grant(
            &consumer_environment.id.0,
            &EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ById(ToolReleaseById {
                    release_id: release.id,
                }),
            },
        )
        .await?;
    assert!(!administrator_managed.grant.automatic);
    assert!(
        consumer
            .client
            .delete_automatic_environment_tool_grant(&administrator_managed.grant.id.0)
            .await
            .is_err(),
        "automatic reconciliation must not delete an administrator-managed grant"
    );
    let automatic_again = consumer
        .client
        .create_automatic_environment_tool_grant(
            &consumer_environment.id.0,
            &EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ById(ToolReleaseById {
                    release_id: release.id,
                }),
            },
        )
        .await?;
    assert!(automatic_again.grant.automatic);

    consumer
        .client
        .delete_automatic_environment_tool_grant(&automatic_again.grant.id.0)
        .await?;
    let administrator_managed = consumer
        .client
        .create_environment_tool_grant(
            &consumer_environment.id.0,
            &EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ById(ToolReleaseById {
                    release_id: release.id,
                }),
            },
        )
        .await?;
    assert!(
        !administrator_managed.grant.automatic,
        "a grant created through the administrator-managed endpoint must not remain automatic"
    );

    Ok(())
}
