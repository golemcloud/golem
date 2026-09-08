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

use crate::services::application::{ApplicationError, ApplicationService};
use crate::services::component::{ComponentError, ComponentService, ComponentWriteService};
use crate::services::deployment::{DeploymentService, DeploymentWriteService};
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::tool_release::ToolReleaseService;
use golem_common::model::account::AccountId;
use golem_common::model::agent::extraction::extract_component_metadata_from_bytes;
use golem_common::model::application::{
    Application, ApplicationCreation, ApplicationId, ApplicationName,
};
use golem_common::model::component::{ComponentCreation, ComponentName};
use golem_common::model::component::{ToolDeploymentConfigCreation, ToolProvisionConfigCreation};
use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName,
};
use golem_common::model::tool::{TOOL_METADATA_WIT_VERSION, ToolName, ToolSource};
use golem_common::model::tool_release::{SystemToolAvailability, SystemToolReleaseProvision};
use golem_common::schema::tool::Tool;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const SYSTEM_APP_NAME: &str = "golem-system";
const SYSTEM_ENV_NAME: &str = "builtin-tools";

pub(crate) struct BuiltinToolDescriptor {
    pub component_name: &'static str,
    pub tool_name: &'static str,
    pub release_version: &'static str,
    pub wasm_bytes: &'static [u8],
}

// There is intentionally no production tool inventory yet. Future artifacts are embedded in
// descriptors here rather than loaded from registry-service filesystem paths.
static BUILTIN_TOOLS: &[BuiltinToolDescriptor] = &[];

#[allow(clippy::too_many_arguments)]
pub async fn provision_builtin_tools(
    builtin_tool_owner_account_id: AccountId,
    application_service: &Arc<ApplicationService>,
    environment_service: &Arc<EnvironmentService>,
    component_service: &Arc<ComponentService>,
    component_write_service: &Arc<ComponentWriteService>,
    deployment_service: &Arc<DeploymentService>,
    deployment_write_service: &Arc<DeploymentWriteService>,
    tool_release_service: &Arc<ToolReleaseService>,
) -> anyhow::Result<()> {
    provision_descriptors(
        BUILTIN_TOOLS,
        builtin_tool_owner_account_id,
        application_service,
        environment_service,
        component_service,
        component_write_service,
        deployment_service,
        deployment_write_service,
        tool_release_service,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn provision_descriptors(
    descriptors: &[BuiltinToolDescriptor],
    owner: AccountId,
    applications: &Arc<ApplicationService>,
    environments: &Arc<EnvironmentService>,
    components: &Arc<ComponentService>,
    component_writes: &Arc<ComponentWriteService>,
    deployments: &Arc<DeploymentService>,
    deployment_writes: &Arc<DeploymentWriteService>,
    releases: &Arc<ToolReleaseService>,
) -> anyhow::Result<()> {
    if descriptors.is_empty() {
        return Ok(());
    }
    let mut extracted = Vec::with_capacity(descriptors.len());
    let mut coordinates = std::collections::BTreeSet::new();
    let mut component_names = std::collections::BTreeSet::new();
    for descriptor in descriptors {
        if !coordinates.insert((descriptor.tool_name, descriptor.release_version)) {
            anyhow::bail!(
                "duplicate built-in tool release coordinate '{}@{}'",
                descriptor.tool_name,
                descriptor.release_version
            );
        }
        if !component_names.insert(descriptor.component_name) {
            anyhow::bail!(
                "duplicate built-in tool component name '{}'",
                descriptor.component_name
            );
        }
        let metadata = extract_component_metadata_from_bytes(descriptor.wasm_bytes, true, true)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to extract built-in tool '{}@{}': {error}",
                    descriptor.tool_name,
                    descriptor.release_version
                )
            })?;
        let name = ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg)?;
        let tool = metadata
            .tools
            .into_iter()
            .find(|tool| tool.name() == Some(name.as_str()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "built-in component '{}' does not export tool '{}'",
                    descriptor.component_name,
                    name
                )
            })?;
        if tool.version != descriptor.release_version {
            anyhow::bail!(
                "built-in tool '{}' metadata does not match release coordinate {}",
                name,
                descriptor.release_version
            );
        }
        releases
            .preflight_system_component_release(
                &name,
                descriptor.release_version,
                &tool,
                &ComponentName(descriptor.component_name.to_string()),
                golem_common::model::diff::Hash::new(blake3::hash(descriptor.wasm_bytes)),
            )
            .await?;
        extracted.push(tool);
    }
    let auth = AuthCtx::system();
    let app = get_or_create_application(applications, owner, &auth).await?;
    let env = get_or_create_environment(environments, app.id, &auth).await?;
    let mut staged = Vec::new();
    for (descriptor, tool) in descriptors.iter().zip(extracted) {
        let component = upload_component(
            component_writes,
            components,
            env.id,
            descriptor,
            tool,
            &auth,
        )
        .await?;
        staged.push((descriptor, component));
    }
    let mut deployment_current = true;
    for (_, staged_component) in &staged {
        match components
            .get_deployed_component(staged_component.id, &auth)
            .await
        {
            Ok(deployed) if deployed.revision == staged_component.revision => {}
            _ => deployment_current = false,
        }
    }
    if !deployment_current {
        deploy(deployments, deployment_writes, env.id, &auth).await?;
    }
    for (descriptor, staged_component) in staged {
        let component = components
            .get_deployed_component(staged_component.id, &auth)
            .await?;
        if component.revision != staged_component.revision {
            anyhow::bail!(
                "built-in tool component '{}' deployed revision changed unexpectedly",
                descriptor.component_name
            );
        }
        let name = ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg)?;
        let metadata = component.metadata.tools().get(&name).ok_or_else(|| {
            anyhow::anyhow!(
                "built-in component '{}' does not export tool '{}'",
                descriptor.component_name,
                name
            )
        })?;
        if metadata.definition.name() != Some(name.as_str())
            || metadata.definition.version != descriptor.release_version
        {
            anyhow::bail!(
                "built-in tool '{}' metadata does not match release coordinate {}",
                name,
                descriptor.release_version
            );
        }
        releases
            .provision_system_release(SystemToolReleaseProvision {
                name,
                version: descriptor.release_version.to_string(),
                source: ToolSource::Component {
                    component_id: component.id,
                    component_revision: component.revision,
                    component_name: component.component_name.clone(),
                },
                definition: metadata.definition.clone(),
                metadata_version: TOOL_METADATA_WIT_VERSION.to_string(),
                availability: SystemToolAvailability::Grantable,
            })
            .await?;
    }
    tracing::info!("Built-in tools provisioned successfully");
    Ok(())
}

async fn get_or_create_application(
    service: &Arc<ApplicationService>,
    owner: AccountId,
    auth: &AuthCtx,
) -> anyhow::Result<Application> {
    let name = ApplicationName(SYSTEM_APP_NAME.into());
    match service.get_in_account(owner, &name, auth).await {
        Ok(value) => Ok(value),
        Err(ApplicationError::ApplicationByNameNotFound(_)) => match service
            .create(owner, ApplicationCreation { name: name.clone() }, auth)
            .await
        {
            Ok(value) => Ok(value),
            Err(ApplicationError::ApplicationWithNameAlreadyExists) => {
                Ok(service.get_in_account(owner, &name, auth).await?)
            }
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    }
}

async fn get_or_create_environment(
    service: &Arc<EnvironmentService>,
    app: ApplicationId,
    auth: &AuthCtx,
) -> anyhow::Result<Environment> {
    let name = EnvironmentName(SYSTEM_ENV_NAME.into());
    match service.get_in_application(app, &name, auth).await {
        Ok(value) => Ok(value),
        Err(EnvironmentError::EnvironmentByNameNotFound(_)) => match service
            .create(
                app,
                EnvironmentCreation {
                    name: name.clone(),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                },
                auth,
            )
            .await
        {
            Ok(value) => Ok(value),
            Err(EnvironmentError::EnvironmentWithNameAlreadyExists) => {
                Ok(service.get_in_application(app, &name, auth).await?)
            }
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    }
}

async fn upload_component(
    writes: &Arc<ComponentWriteService>,
    reads: &Arc<ComponentService>,
    env: EnvironmentId,
    descriptor: &BuiltinToolDescriptor,
    tool: Tool,
    auth: &AuthCtx,
) -> anyhow::Result<Component> {
    let name = ComponentName(descriptor.component_name.into());
    let tool_name = ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg)?;
    let tool_deployment_configs = BTreeMap::from([(
        tool_name,
        ToolDeploymentConfigCreation {
            provision: ToolProvisionConfigCreation {
                config: serde_json::json!({}).into(),
                env: BTreeMap::new(),
                plugin_installations: Vec::new(),
                files: BTreeMap::new(),
            },
            environment_binding: None,
            agent_bindings: BTreeMap::new(),
        },
    )]);
    match writes
        .create(
            env,
            ComponentCreation {
                component_name: name.clone(),
                agent_types: vec![],
                agent_type_provision_configs: BTreeMap::new(),
                tools: vec![tool],
                tool_deployment_configs,
            },
            descriptor.wasm_bytes.to_vec(),
            None,
            auth,
        )
        .await
    {
        Ok(value) => Ok(value),
        Err(ComponentError::ComponentWithNameAlreadyExists(_)) => {
            let existing = reads.get_staged_component_by_name(env, &name, auth).await?;
            let expected =
                golem_common::model::diff::Hash::new(blake3::hash(descriptor.wasm_bytes));
            if existing.wasm_hash != expected {
                anyhow::bail!(
                    "built-in tool release '{}@{}' is immutable but its embedded artifact changed",
                    descriptor.tool_name,
                    descriptor.release_version
                );
            }
            Ok(existing)
        }
        Err(error) => Err(error.into()),
    }
}

async fn deploy(
    reads: &Arc<DeploymentService>,
    writes: &Arc<DeploymentWriteService>,
    env: EnvironmentId,
    auth: &AuthCtx,
) -> anyhow::Result<()> {
    let plan = reads.get_current_deployment_plan(env, auth).await?;
    writes
        .create_deployment(
            env,
            DeploymentCreation {
                current_revision: plan.current_revision,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion(Uuid::new_v4().to_string()),
                publish_tools: vec![],
                remote_tools: vec![],
                agent_secret_defaults: vec![],
                quota_resource_defaults: vec![],
                retry_policy_defaults: vec![],
                replace_incompatible_agent_secrets: false,
            },
            auth,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::Services;
    use crate::config::{ComponentCompilationConfig, LoginConfig, RegistryServiceConfig};
    use golem_common::config::{DbConfig, DbSqliteConfig};
    use golem_common::model::Empty;
    use golem_service_base::config::BlobStorageConfig;
    use test_r::{test, timeout};
    use tokio::task::JoinSet;

    #[test]
    #[timeout("120s")]
    async fn provisions_component_tool_release_idempotently_and_rejects_mismatch_without_mutation()
    {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = RegistryServiceConfig {
            db: DbConfig::Sqlite(DbSqliteConfig {
                database: temp_dir
                    .path()
                    .join("registry.db")
                    .to_string_lossy()
                    .into_owned(),
                max_connections: 4,
                foreign_keys: true,
            }),
            login: LoginConfig::Disabled(Empty {}),
            blob_storage: BlobStorageConfig::default_in_memory(),
            component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
            ..Default::default()
        };
        let owner = config.initial_accounts["builtin_tool_owner"].id;
        let mut join_set = JoinSet::new();
        let services = Services::new(&config, &mut join_set).await.unwrap();
        let wasm = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-components/tool-streaming/golem-temp/agents/",
            "golem_it_tool_streaming_rust_provider_release.wasm"
        ))
        .expect("build the tool-streaming test component before running this test");
        let wasm = Box::leak(wasm.into_boxed_slice());
        let descriptor = BuiltinToolDescriptor {
            component_name: "builtin-tool-streaming-test",
            tool_name: "streaming",
            release_version: "1.0.0",
            wasm_bytes: wasm,
        };
        let auth = AuthCtx::system();

        provision(&services, owner, std::slice::from_ref(&descriptor)).await;
        let app = services
            .application_service
            .get_in_account(owner, &ApplicationName(SYSTEM_APP_NAME.into()), &auth)
            .await
            .unwrap();
        let env = services
            .environment_service
            .get_in_application(app.id, &EnvironmentName(SYSTEM_ENV_NAME.into()), &auth)
            .await
            .unwrap();
        let first_component = services
            .component_service
            .get_staged_component_by_name(
                env.id,
                &ComponentName(descriptor.component_name.into()),
                &auth,
            )
            .await
            .unwrap();
        let first_release = only_release(&services, owner).await;
        let first_deployments = services
            .deployment_service
            .list_deployments(env.id, None, &auth)
            .await
            .unwrap();
        assert_eq!(
            first_component.revision,
            golem_common::model::component::ComponentRevision::INITIAL
        );
        assert_eq!(first_deployments.len(), 1);

        provision(&services, owner, std::slice::from_ref(&descriptor)).await;
        let repeated_component = services
            .component_service
            .get_staged_component_by_name(
                env.id,
                &ComponentName(descriptor.component_name.into()),
                &auth,
            )
            .await
            .unwrap();
        let repeated_release = only_release(&services, owner).await;
        assert_eq!(repeated_component.id, first_component.id);
        assert_eq!(repeated_component.revision, first_component.revision);
        assert_eq!(repeated_release.id, first_release.id);
        assert_eq!(
            services
                .deployment_service
                .list_deployments(env.id, None, &auth)
                .await
                .unwrap()
                .len(),
            1
        );

        let mismatch = BuiltinToolDescriptor {
            component_name: "different-builtin-tool-streaming-test",
            ..descriptor
        };
        let error = provision_descriptors(
            std::slice::from_ref(&mismatch),
            owner,
            &services.application_service,
            &services.environment_service,
            &services.component_service,
            &services.component_write_service,
            &services.deployment_service,
            &services.deployment_write_service,
            &services.tool_release_service,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("immutable"), "{error:#}");
        assert_eq!(only_release(&services, owner).await.id, first_release.id);
        assert_eq!(
            services
                .component_service
                .list_staged_components_for_environment(&env, &auth)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            services
                .deployment_service
                .list_deployments(env.id, None, &auth)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    async fn provision(
        services: &Services,
        owner: AccountId,
        descriptors: &[BuiltinToolDescriptor],
    ) {
        provision_descriptors(
            descriptors,
            owner,
            &services.application_service,
            &services.environment_service,
            &services.component_service,
            &services.component_write_service,
            &services.deployment_service,
            &services.deployment_write_service,
            &services.tool_release_service,
        )
        .await
        .unwrap();
    }

    async fn only_release(
        services: &Services,
        owner: AccountId,
    ) -> golem_common::model::tool_release::ToolRelease {
        let releases = services
            .tool_release_service
            .list_in_account(owner, &AuthCtx::system())
            .await
            .unwrap();
        assert_eq!(releases.len(), 1);
        releases.into_iter().next().unwrap()
    }
}
