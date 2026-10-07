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
use crate::services::auth::AuthService;
use crate::services::builtin_artifact::BuiltinArtifactResolver;
use crate::services::component::{ComponentError, ComponentService, ComponentWriteService};
use crate::services::deployment::{DeploymentService, DeploymentWriteService};
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::tool_middleware_release::ToolMiddlewareReleaseService;
use crate::services::tool_release::ToolReleaseService;
use golem_common::model::account::AccountId;
use golem_common::model::agent::extraction::extract_component_metadata_from_bytes;
use golem_common::model::application::{
    Application, ApplicationCreation, ApplicationId, ApplicationName,
};
use golem_common::model::component::{ComponentCreation, ComponentName, ComponentUpdate};
use golem_common::model::component::{
    ToolDeploymentConfigCreation, ToolDeploymentConfigUpdate, ToolProvisionConfigCreation,
    ToolProvisionConfigUpdate,
};
use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName,
};
use golem_common::model::tool::{TOOL_METADATA_WIT_VERSION, ToolName, ToolSource};
use golem_common::model::tool_middleware::{
    TOOL_MIDDLEWARE_METADATA_WIT_VERSION, ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::model::tool_middleware_release::SystemToolMiddlewareReleaseProvision;
use golem_common::model::tool_release::{SystemToolAvailability, SystemToolReleaseProvision};
use golem_common::schema::tool::{Tool, ToolMiddleware};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const SYSTEM_APP_NAME: &str = "golem-system";
const SYSTEM_ENV_NAME: &str = "builtin-tools";

#[derive(Clone, Copy)]
pub struct BuiltinExportDescriptor {
    pub component_name: &'static str,
    pub artifact_id: &'static str,
    pub export_name: &'static str,
    pub release_version: &'static str,
    pub kind: BuiltinExportKind,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuiltinExportKind {
    Tool,
    Middleware,
}

enum ExtractedBuiltinExport {
    Tool(Tool),
    Middleware(ToolMiddleware),
}

#[derive(Default)]
struct ComponentExports {
    tools: Vec<Tool>,
    middlewares: Vec<ToolMiddleware>,
}

static BUILTIN_EXPORTS: &[BuiltinExportDescriptor] = &[
    BuiltinExportDescriptor {
        component_name: "bash",
        artifact_id: "bash",
        export_name: "bash",
        release_version: "0.2.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools",
        export_name: "read-file",
        release_version: "0.4.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools",
        export_name: "write-file",
        release_version: "0.4.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools",
        export_name: "ls",
        release_version: "0.1.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools",
        export_name: "grep",
        release_version: "0.1.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools",
        export_name: "edit-file",
        release_version: "0.4.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools",
        export_name: "path-policy",
        release_version: "0.1.1",
        kind: BuiltinExportKind::Middleware,
    },
    BuiltinExportDescriptor {
        component_name: "javascript-tools",
        artifact_id: "javascript_tools",
        export_name: "node",
        release_version: "0.1.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "javascript-tools",
        artifact_id: "javascript_tools",
        export_name: "npm",
        release_version: "10.9.9+golem.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "javascript-tools",
        artifact_id: "javascript_tools",
        export_name: "npx",
        release_version: "10.9.9+golem.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "typescript-tools",
        artifact_id: "typescript_tools",
        export_name: "tsc",
        release_version: "5.9.2+golem.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "web-fetch",
        artifact_id: "web_fetch",
        export_name: "web-fetch",
        release_version: "0.1.1",
        kind: BuiltinExportKind::Tool,
    },
    BuiltinExportDescriptor {
        component_name: "git-tool",
        artifact_id: "git_tool",
        export_name: "git",
        release_version: "0.1.8",
        kind: BuiltinExportKind::Tool,
    },
];

#[allow(clippy::too_many_arguments)]
pub async fn provision_builtin_tools(
    artifact_resolver: &BuiltinArtifactResolver,
    builtin_tool_owner_account_id: AccountId,
    auth_service: &Arc<AuthService>,
    application_service: &Arc<ApplicationService>,
    environment_service: &Arc<EnvironmentService>,
    component_service: &Arc<ComponentService>,
    component_write_service: &Arc<ComponentWriteService>,
    deployment_service: &Arc<DeploymentService>,
    deployment_write_service: &Arc<DeploymentWriteService>,
    tool_release_service: &Arc<ToolReleaseService>,
    tool_middleware_release_service: &Arc<ToolMiddlewareReleaseService>,
) -> anyhow::Result<()> {
    let artifacts = artifact_resolver
        .resolve_many(
            BUILTIN_EXPORTS
                .iter()
                .map(|descriptor| descriptor.artifact_id),
        )
        .await?;
    provision_descriptors(
        BUILTIN_EXPORTS,
        &artifacts,
        builtin_tool_owner_account_id,
        auth_service,
        application_service,
        environment_service,
        component_service,
        component_write_service,
        deployment_service,
        deployment_write_service,
        tool_release_service,
        tool_middleware_release_service,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn provision_descriptors(
    descriptors: &[BuiltinExportDescriptor],
    artifacts: &BTreeMap<&str, Arc<Vec<u8>>>,
    owner: AccountId,
    auth_service: &Arc<AuthService>,
    applications: &Arc<ApplicationService>,
    environments: &Arc<EnvironmentService>,
    components: &Arc<ComponentService>,
    component_writes: &Arc<ComponentWriteService>,
    deployments: &Arc<DeploymentService>,
    deployment_writes: &Arc<DeploymentWriteService>,
    releases: &Arc<ToolReleaseService>,
    middleware_releases: &Arc<ToolMiddlewareReleaseService>,
) -> anyhow::Result<()> {
    if descriptors.is_empty() {
        return Ok(());
    }
    let mut extracted = Vec::with_capacity(descriptors.len());
    let mut coordinates = std::collections::BTreeSet::new();
    let mut component_hashes = BTreeMap::new();
    for descriptor in descriptors {
        let wasm_bytes = artifacts.get(descriptor.artifact_id).ok_or_else(|| {
            anyhow::anyhow!(
                "built-in export artifact '{}' was not resolved",
                descriptor.artifact_id
            )
        })?;
        if !coordinates.insert((
            descriptor.kind,
            descriptor.export_name,
            descriptor.release_version,
        )) {
            anyhow::bail!(
                "duplicate built-in export release coordinate '{}@{}'",
                descriptor.export_name,
                descriptor.release_version
            );
        }
        let wasm_hash = blake3::hash(wasm_bytes);
        if let Some(existing_hash) = component_hashes.insert(descriptor.component_name, wasm_hash)
            && existing_hash != wasm_hash
        {
            anyhow::bail!(
                "built-in tool component '{}' has conflicting artifacts",
                descriptor.component_name
            );
        }
        let metadata = extract_component_metadata_from_bytes(wasm_bytes, true, true)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to extract built-in export '{}@{}': {error}",
                    descriptor.export_name,
                    descriptor.release_version
                )
            })?;
        let component_name = ComponentName(descriptor.component_name.to_string());
        let wasm_hash = golem_common::model::diff::Hash::new(wasm_hash);
        let (export, already_published) = match descriptor.kind {
            BuiltinExportKind::Tool => {
                let name =
                    ToolName::try_from(descriptor.export_name).map_err(anyhow::Error::msg)?;
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
                let published = releases
                    .preflight_system_component_release(
                        &name,
                        descriptor.release_version,
                        &tool,
                        &component_name,
                        wasm_hash,
                    )
                    .await?;
                (ExtractedBuiltinExport::Tool(tool), published)
            }
            BuiltinExportKind::Middleware => {
                let name = ToolMiddlewareName::try_from(descriptor.export_name)
                    .map_err(anyhow::Error::msg)?;
                let middleware = metadata
                    .tool_middlewares
                    .into_iter()
                    .find(|middleware| middleware.name == name.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "built-in component '{}' does not export tool middleware '{}'",
                            descriptor.component_name,
                            name
                        )
                    })?;
                if middleware.version != descriptor.release_version {
                    anyhow::bail!(
                        "built-in tool middleware '{}' metadata does not match release coordinate {}",
                        name,
                        descriptor.release_version
                    );
                }
                let published = middleware_releases
                    .preflight_system_component_release(
                        &name,
                        descriptor.release_version,
                        &middleware,
                        &component_name,
                        wasm_hash,
                    )
                    .await?;
                (ExtractedBuiltinExport::Middleware(middleware), published)
            }
        };
        extracted.push((export, already_published));
    }
    if extracted
        .iter()
        .all(|(_, already_published)| *already_published)
    {
        tracing::info!("Built-in tools are already provisioned");
        return Ok(());
    }
    let auth = auth_service.builtin_owner_auth(owner).await?;
    let app = get_or_create_application(applications, owner, &auth).await?;
    let environment = get_or_create_environment(environments, app.id, &auth).await?;
    let components_needing_work = descriptors
        .iter()
        .zip(&extracted)
        .filter_map(|(descriptor, (_, already_published))| {
            (!already_published).then_some(descriptor.component_name)
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut component_exports = BTreeMap::<_, ComponentExports>::new();
    for (descriptor, (export, _)) in descriptors.iter().zip(&extracted) {
        if !components_needing_work.contains(descriptor.component_name) {
            continue;
        }
        let component = component_exports
            .entry(descriptor.component_name)
            .or_default();
        match export {
            ExtractedBuiltinExport::Tool(tool) => component.tools.push(tool.clone()),
            ExtractedBuiltinExport::Middleware(middleware) => {
                component.middlewares.push(middleware.clone())
            }
        }
    }
    let mut staged = BTreeMap::new();
    for (component_name, exports) in component_exports {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.component_name == component_name)
            .expect("component group came from descriptors");
        let wasm_bytes = artifacts
            .get(descriptor.artifact_id)
            .expect("all descriptor artifacts were resolved");
        let component = upload_component(
            component_writes,
            components,
            environment.id,
            descriptor,
            wasm_bytes,
            exports.tools,
            exports.middlewares,
            &auth,
        )
        .await?;
        match components.get_deployed_component(component.id, &auth).await {
            Ok(deployed) if deployed.revision == component.revision => {}
            _ => {
                if let Err(error) =
                    deploy(deployments, deployment_writes, environment.id, &auth).await
                {
                    match components.get_deployed_component(component.id, &auth).await {
                        Ok(deployed) if deployed.revision == component.revision => {}
                        _ => return Err(error),
                    }
                }
            }
        }
        staged.insert(component_name, component);
    }
    for (descriptor, (export, already_published)) in descriptors.iter().zip(extracted) {
        if already_published {
            continue;
        }
        let staged_component = &staged[descriptor.component_name];
        let component = components
            .get_deployed_component(staged_component.id, &auth)
            .await?;
        if component.revision != staged_component.revision {
            anyhow::bail!(
                "built-in tool component '{}' deployed revision changed unexpectedly",
                descriptor.component_name
            );
        }
        match export {
            ExtractedBuiltinExport::Tool(tool) => {
                let name =
                    ToolName::try_from(descriptor.export_name).map_err(anyhow::Error::msg)?;
                let metadata = component.metadata.tools().get(&name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "built-in component '{}' does not export tool '{}'",
                        descriptor.component_name,
                        name
                    )
                })?;
                if metadata.definition != tool {
                    anyhow::bail!("deployed built-in tool '{}' metadata changed", name);
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
            ExtractedBuiltinExport::Middleware(middleware) => {
                let name = ToolMiddlewareName::try_from(descriptor.export_name)
                    .map_err(anyhow::Error::msg)?;
                let metadata = component
                    .metadata
                    .tool_middlewares()
                    .get(&name)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "built-in component '{}' does not export tool middleware '{}'",
                            descriptor.component_name,
                            name
                        )
                    })?;
                if metadata.definition != middleware {
                    anyhow::bail!("deployed built-in middleware '{}' metadata changed", name);
                }
                middleware_releases
                    .provision_system_release(SystemToolMiddlewareReleaseProvision {
                        name,
                        version: descriptor.release_version.to_string(),
                        source: ToolMiddlewareSource::Component {
                            component_id: component.id,
                            component_revision: component.revision,
                            component_name: component.component_name.clone(),
                        },
                        definition: metadata.definition.clone(),
                        metadata_version: TOOL_MIDDLEWARE_METADATA_WIT_VERSION.to_string(),
                    })
                    .await?;
            }
        }
    }
    tracing::info!("Built-in tool components provisioned successfully");
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
                    tool_compatibility_mode: Default::default(),
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
    descriptor: &BuiltinExportDescriptor,
    wasm_bytes: &[u8],
    tools: Vec<Tool>,
    middlewares: Vec<ToolMiddleware>,
    auth: &AuthCtx,
) -> anyhow::Result<Component> {
    let name = ComponentName(descriptor.component_name.into());
    let tool_deployment_configs = tools
        .iter()
        .map(|tool| {
            let tool_name = ToolName::try_from(
                tool.name()
                    .expect("built-in tool metadata was matched by a valid name"),
            )
            .expect("built-in tool metadata name was already validated");
            (
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
                    component_bindings: BTreeMap::new(),
                },
            )
        })
        .collect();
    let tool_middleware_provision_configs = middlewares
        .iter()
        .map(|middleware| {
            let name = ToolMiddlewareName::try_from(middleware.name.clone())
                .expect("built-in middleware metadata name was already validated");
            (
                name,
                ToolProvisionConfigCreation {
                    config: serde_json::json!({}).into(),
                    env: BTreeMap::new(),
                    plugin_installations: Vec::new(),
                    files: BTreeMap::new(),
                },
            )
        })
        .collect();
    match writes
        .create(
            env,
            ComponentCreation {
                component_name: name.clone(),
                config_schema: Default::default(),
                component_provision_config: Default::default(),
                agent_types: vec![],
                agent_type_provision_configs: BTreeMap::new(),
                tools: tools.clone(),
                tool_deployment_configs,
                tool_middlewares: middlewares.clone(),
                tool_middleware_provision_configs,
            },
            wasm_bytes.to_vec(),
            None,
            auth,
        )
        .await
    {
        Ok(value) => Ok(value),
        Err(ComponentError::ComponentWithNameAlreadyExists(_)) => loop {
            let existing = reads.get_staged_component_by_name(env, &name, auth).await?;
            let expected = golem_common::model::diff::Hash::new(blake3::hash(wasm_bytes));
            if existing.wasm_hash == expected
                && component_has_intended_exports(&existing, &tools, &middlewares)
            {
                break Ok(existing);
            }
            let tool_deployment_config_updates = tools
                .iter()
                .map(|tool| {
                    let name = ToolName::try_from(
                        tool.name()
                            .expect("built-in tool metadata was matched by a valid name"),
                    )
                    .expect("built-in tool metadata name was already validated");
                    (
                        name,
                        ToolDeploymentConfigUpdate {
                            provision: Some(ToolProvisionConfigUpdate {
                                config: Some(serde_json::json!({}).into()),
                                env: Some(BTreeMap::new()),
                                plugin_updates: Vec::new(),
                                files_to_add_or_update: BTreeMap::new(),
                                files_to_remove: Vec::new(),
                                file_permission_updates: BTreeMap::new(),
                            }),
                            environment_binding: Default::default(),
                            component_bindings: Some(BTreeMap::new()),
                            agent_bindings: Some(BTreeMap::new()),
                        },
                    )
                })
                .collect();
            let tool_middleware_provision_config_updates = middlewares
                .iter()
                .map(|middleware| {
                    let name = ToolMiddlewareName::try_from(middleware.name.clone())
                        .expect("built-in middleware metadata name was already validated");
                    (
                        name,
                        ToolProvisionConfigUpdate {
                            config: Some(serde_json::json!({}).into()),
                            env: Some(BTreeMap::new()),
                            plugin_updates: Vec::new(),
                            files_to_add_or_update: BTreeMap::new(),
                            files_to_remove: Vec::new(),
                            file_permission_updates: BTreeMap::new(),
                        },
                    )
                })
                .collect();
            match writes
                .update(
                    existing.id,
                    ComponentUpdate {
                        current_revision: existing.revision,
                        config_schema: None,
                        component_provision_config: None,
                        agent_types: None,
                        agent_type_provision_config_updates: None,
                        tools: Some(tools.clone()),
                        tool_deployment_config_updates: Some(tool_deployment_config_updates),
                        tool_middlewares: Some(middlewares.clone()),
                        tool_middleware_provision_config_updates: Some(
                            tool_middleware_provision_config_updates,
                        ),
                        allow_incompatible_config: false,
                    },
                    Some(wasm_bytes.to_vec()),
                    None,
                    auth,
                )
                .await
            {
                Ok(component) => break Ok(component),
                Err(ComponentError::ConcurrentUpdate) => continue,
                Err(error) => break Err(error.into()),
            }
        },
        Err(error) => Err(error.into()),
    }
}

fn component_has_intended_exports(
    component: &Component,
    tools: &[Tool],
    middlewares: &[ToolMiddleware],
) -> bool {
    let stored = component.metadata.tools();
    let stored_middlewares = component.metadata.tool_middlewares();
    stored.len() == tools.len()
        && stored_middlewares.len() == middlewares.len()
        && tools.iter().all(|tool| {
            tool.name()
                .and_then(|name| ToolName::try_from(name).ok())
                .and_then(|name| stored.get(&name))
                .is_some_and(|metadata| metadata.definition == *tool)
        })
        && middlewares.iter().all(|middleware| {
            ToolMiddlewareName::try_from(middleware.name.clone())
                .ok()
                .and_then(|name| stored_middlewares.get(&name))
                .is_some_and(|metadata| metadata.definition == *middleware)
        })
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
                mcp_imports: Vec::new(),
                current_revision: plan.current_revision,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion(Uuid::new_v4().to_string()),
                publish_tools: vec![],
                remote_tools: vec![],
                publish_tool_middlewares: vec![],
                remote_tool_middlewares: vec![],
                universal_tool_middlewares: vec![],
                environment_tool_middleware_bindings: Default::default(),
                agent_tool_middleware_bindings: Default::default(),
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
    use test_r::test;

    #[test]
    fn components_use_one_artifact_each() {
        let mut artifacts_by_component = BTreeMap::new();
        for descriptor in BUILTIN_EXPORTS {
            if let Some(previous) =
                artifacts_by_component.insert(descriptor.component_name, descriptor.artifact_id)
            {
                assert_eq!(previous, descriptor.artifact_id);
            }
        }
    }
}
