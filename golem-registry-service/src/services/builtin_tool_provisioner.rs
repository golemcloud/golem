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
use crate::services::component::{ComponentError, ComponentService, ComponentWriteService};
use crate::services::deployment::{
    DeploymentService, DeploymentWriteError, DeploymentWriteService,
};
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::tool_release::{SystemReleaseState, ToolReleaseService};
use golem_common::model::account::AccountId;
use golem_common::model::agent::extraction::extract_component_metadata_from_bytes;
use golem_common::model::application::{
    Application, ApplicationCreation, ApplicationId, ApplicationName,
};
use golem_common::model::component::{
    ComponentCreation, ComponentId, ComponentName, ComponentUpdate,
};
use golem_common::model::component::{
    ToolDeploymentConfigCreation, ToolDeploymentConfigUpdate, ToolProvisionConfigCreation,
    ToolProvisionConfigUpdate,
};
use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName,
};
use golem_common::model::tool::{TOOL_METADATA_WIT_VERSION, ToolName, ToolSource};
use golem_common::model::tool_release::{
    SystemToolAvailability, SystemToolReleaseProvision, ToolRelease,
};
use golem_common::schema::tool::Tool;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use uuid::Uuid;

const SYSTEM_APP_NAME: &str = "golem-system";
const SYSTEM_ENV_NAME: &str = "builtin-tools";

/// A built-in tool release embedded in this build. Descriptors that name the same component share
/// one embedded artifact, and that component implements all of their tools.
pub struct BuiltinToolDescriptor {
    pub component_name: &'static str,
    pub tool_name: &'static str,
    pub release_version: &'static str,
    pub wasm_bytes: &'static [u8],
}

impl BuiltinToolDescriptor {
    fn coordinate(&self) -> String {
        format!("{}@{}", self.tool_name, self.release_version)
    }
}

/// What one provisioning pass did, by release coordinate (`bash@0.2.0`) or component name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProvisionReport {
    /// Releases whose embedded component was compiled and instantiated to extract its metadata.
    /// A release already provisioned from the same bytes is not.
    pub extracted: Vec<String>,
    /// Older components that stopped implementing a tool a new release now implements.
    pub retired: Vec<String>,
    /// Whether the built-in environment was deployed.
    pub deployed: bool,
    /// Releases newly published.
    pub published: Vec<String>,
    /// Older releases marked superseded by a newly published version.
    pub superseded: Vec<String>,
}

static BUILTIN_TOOLS: &[BuiltinToolDescriptor] = &[
    BuiltinToolDescriptor {
        component_name: "filesystem-tools",
        tool_name: "read-file",
        release_version: "0.3.0",
        wasm_bytes: include_bytes!("../../../builtin-tools/filesystem-tools.wasm"),
    },
    BuiltinToolDescriptor {
        component_name: "filesystem-tools",
        tool_name: "write-file",
        release_version: "0.3.0",
        wasm_bytes: include_bytes!("../../../builtin-tools/filesystem-tools.wasm"),
    },
    BuiltinToolDescriptor {
        component_name: "filesystem-tools",
        tool_name: "edit-file",
        release_version: "0.3.0",
        wasm_bytes: include_bytes!("../../../builtin-tools/filesystem-tools.wasm"),
    },
    // Each Bash version has its own component (`golem:bash-0-2-0`), so a new version never
    // replaces the revision an older release, and every grant of it, is pinned to; provisioning
    // retires the older component's implementation of the tool instead.
    BuiltinToolDescriptor {
        component_name: "golem:bash-0-2-0",
        tool_name: "bash",
        release_version: "0.2.0",
        wasm_bytes: include_bytes!("../../../builtin-tools/bash.wasm"),
    },
];

#[allow(clippy::too_many_arguments)]
pub async fn provision_builtin_tools(
    builtin_tool_owner_account_id: AccountId,
    auth_service: &Arc<AuthService>,
    application_service: &Arc<ApplicationService>,
    environment_service: &Arc<EnvironmentService>,
    component_service: &Arc<ComponentService>,
    component_write_service: &Arc<ComponentWriteService>,
    deployment_service: &Arc<DeploymentService>,
    deployment_write_service: &Arc<DeploymentWriteService>,
    tool_release_service: &Arc<ToolReleaseService>,
) -> anyhow::Result<ProvisionReport> {
    provision_descriptors(
        BUILTIN_TOOLS,
        builtin_tool_owner_account_id,
        auth_service,
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

/// Provisions each descriptor's release into the `golem-system/builtin-tools` environment.
///
/// A release already provisioned from the same bytes costs a few lookups: its metadata is not
/// extracted again (extraction compiles and instantiates the component) and nothing is deployed.
/// A component that needs work is staged with every tool its descriptors name. When the
/// component already exists (the filesystem tools), the new artifact becomes its next revision.
/// When a new version names a component of its own (Bash's per-version components), every older
/// component that implements the same tool stops implementing it in the same deployment that
/// adds the new one, so the environment never has two implementors and never none. Older
/// releases stay pinned to the revisions they were provisioned from, and are superseded only once
/// the new release is published.
#[allow(clippy::too_many_arguments)]
pub async fn provision_descriptors(
    descriptors: &[BuiltinToolDescriptor],
    owner: AccountId,
    auth_service: &Arc<AuthService>,
    applications: &Arc<ApplicationService>,
    environments: &Arc<EnvironmentService>,
    components: &Arc<ComponentService>,
    component_writes: &Arc<ComponentWriteService>,
    deployments: &Arc<DeploymentService>,
    deployment_writes: &Arc<DeploymentWriteService>,
    releases: &Arc<ToolReleaseService>,
) -> anyhow::Result<ProvisionReport> {
    let mut report = ProvisionReport::default();
    if descriptors.is_empty() {
        return Ok(report);
    }
    let mut tool_names = std::collections::BTreeSet::new();
    let mut component_hashes = BTreeMap::new();
    for descriptor in descriptors {
        if !tool_names.insert(descriptor.tool_name) {
            anyhow::bail!(
                "duplicate built-in tool '{}': one release of a tool is provisioned at a time",
                descriptor.tool_name
            );
        }
        let wasm_hash = blake3::hash(descriptor.wasm_bytes);
        if let Some(existing_hash) = component_hashes.insert(descriptor.component_name, wasm_hash)
            && existing_hash != wasm_hash
        {
            anyhow::bail!(
                "built-in tool component '{}' has conflicting embedded artifacts",
                descriptor.component_name
            );
        }
    }
    let auth = auth_service.builtin_owner_auth(owner).await?;
    let mut current = Vec::new();
    let mut superseded = BTreeSet::new();
    let mut needs_work = BTreeSet::new();
    for descriptor in descriptors {
        let name = ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg)?;
        let component_name = ComponentName(descriptor.component_name.to_string());
        let wasm_hash = golem_common::model::diff::Hash::new(blake3::hash(descriptor.wasm_bytes));
        // Identical bytes extract to an identical definition (extraction runs the component's
        // own discovery with no inputs but its bytes), so a release recorded from
        // these bytes, under this build's metadata version, needs no extraction.
        let state = releases
            .system_component_release_state(
                &name,
                descriptor.release_version,
                &component_name,
                wasm_hash,
            )
            .await?;
        match state {
            SystemReleaseState::Current(release)
                if release_is_deployed(components, &release, wasm_hash, &auth).await =>
            {
                current.push(descriptor);
            }
            SystemReleaseState::Superseded => {
                tracing::warn!(
                    "built-in tool release '{}' was superseded by a newer release, which stays",
                    descriptor.coordinate()
                );
                superseded.insert(descriptor.tool_name);
            }
            SystemReleaseState::Current(_)
            | SystemReleaseState::Absent
            | SystemReleaseState::Differs => {
                needs_work.insert(descriptor.component_name);
            }
        }
    }
    // A component is staged whole: each revision declares every tool its descriptors name, so
    // one tool needing work re-stages its siblings too rather than dropping them.
    let (staging, mut published): (Vec<_>, Vec<_>) = descriptors
        .iter()
        .filter(|descriptor| !superseded.contains(descriptor.tool_name))
        .partition(|descriptor| needs_work.contains(descriptor.component_name));
    published.retain(|descriptor| current.iter().any(|kept| std::ptr::eq(*kept, *descriptor)));
    let mut pending = Vec::new();
    for descriptor in staging {
        let name = ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg)?;
        let component_name = ComponentName(descriptor.component_name.to_string());
        let wasm_hash = golem_common::model::diff::Hash::new(blake3::hash(descriptor.wasm_bytes));
        report.extracted.push(descriptor.coordinate());
        let metadata = extract_component_metadata_from_bytes(descriptor.wasm_bytes, true, true)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to extract built-in tool '{}': {error}",
                    descriptor.coordinate()
                )
            })?;
        let tool = metadata
            .tools
            .into_iter()
            .find(|tool| tool.name() == Some(name.as_str()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "built-in component '{}' does not export tool '{}'",
                    component_name,
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
        let already_published = releases
            .preflight_system_component_release(
                &name,
                descriptor.release_version,
                &tool,
                &component_name,
                wasm_hash,
            )
            .await?;
        pending.push((descriptor, tool, already_published));
    }
    if !pending.is_empty() {
        let app = get_or_create_application(applications, owner, &auth).await?;
        let env = get_or_create_environment(environments, app.id, &auth).await?;
        let mut component_tools = BTreeMap::<&str, Vec<Tool>>::new();
        for (descriptor, tool, _) in &pending {
            component_tools
                .entry(descriptor.component_name)
                .or_default()
                .push(tool.clone());
        }
        let mut uploaded = BTreeMap::new();
        for (component_name, tools) in component_tools {
            let descriptor = pending
                .iter()
                .map(|(descriptor, _, _)| *descriptor)
                .find(|descriptor| descriptor.component_name == component_name)
                .expect("component group came from pending descriptors");
            let component = upload_component(
                component_writes,
                components,
                env.id,
                descriptor,
                tools,
                &auth,
            )
            .await?;
            uploaded.insert(component_name, component);
        }
        let staged: Vec<_> = pending
            .iter()
            .map(|(descriptor, _, _)| (*descriptor, uploaded[descriptor.component_name].clone()))
            .collect();
        // Releases already published from these bytes stay pinned to the revision they were
        // published from; a sibling tool joining their component does not repoint them.
        let already_published: BTreeSet<&str> = pending
            .iter()
            .filter(|(_, _, already)| *already)
            .map(|(descriptor, _, _)| descriptor.tool_name)
            .collect();
        let retired =
            retire_other_implementors(components, component_writes, &env, &staged, &auth).await?;
        let mut deployment_current = retired.is_empty();
        report.retired = retired;
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
            report.deployed = true;
        }
        for (descriptor, staged_component) in staged {
            let component = components
                .get_deployed_component(staged_component.id, &auth)
                .await?;
            if component.revision != staged_component.revision {
                anyhow::bail!(
                    "built-in tool component '{}' deployed revision changed unexpectedly",
                    component.component_name
                );
            }
            let name = ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg)?;
            let metadata = component.metadata.tools().get(&name).ok_or_else(|| {
                anyhow::anyhow!(
                    "built-in component '{}' does not export tool '{}'",
                    component.component_name,
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
            if already_published.contains(descriptor.tool_name) {
                published.push(descriptor);
                continue;
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
            report.published.push(descriptor.coordinate());
            published.push(descriptor);
        }
    }
    // Only now that each release is published do the ones it replaces stop resolving by
    // coordinate; the tool always has a published release.
    for descriptor in published {
        let name = ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg)?;
        for release in releases
            .supersede_older_system_releases(&name, descriptor.release_version)
            .await?
        {
            report
                .superseded
                .push(format!("{}@{}", release.name, release.version));
        }
    }
    tracing::info!(?report, "Built-in tools provisioned successfully");
    Ok(report)
}

/// Whether the component a current release comes from is deployed with the same bytes and still
/// implements the release's tool as published. A later revision that only adds a sibling tool
/// keeps it current.
async fn release_is_deployed(
    components: &Arc<ComponentService>,
    release: &ToolRelease,
    wasm_hash: golem_common::model::diff::Hash,
    auth: &AuthCtx,
) -> bool {
    let ToolSource::Component { component_id, .. } = &release.source else {
        return false;
    };
    components
        .get_deployed_component(*component_id, auth)
        .await
        .is_ok_and(|deployed| {
            deployed.wasm_hash == wasm_hash
                && deployed
                    .metadata
                    .tools()
                    .get(&release.name)
                    .is_some_and(|metadata| metadata.definition == release.definition)
        })
}

/// Makes every other staged component of the environment stop implementing the tools the
/// staged components now implement, so the next deployment has one implementor of each.
///
/// A component cannot be deleted while a release or a deployment refers to it, and an older
/// release always does; instead its next revision declares no such tool. The older releases
/// stay pinned to the revisions they were provisioned from, which remain in storage and in the
/// deployment history, so every grant and agent of them keeps working. Returns the components
/// it changed.
async fn retire_other_implementors(
    components: &Arc<ComponentService>,
    writes: &Arc<ComponentWriteService>,
    env: &Environment,
    staged: &[(&BuiltinToolDescriptor, Component)],
    auth: &AuthCtx,
) -> anyhow::Result<Vec<String>> {
    let keep: BTreeSet<ComponentId> = staged.iter().map(|(_, component)| component.id).collect();
    let tools: BTreeSet<ToolName> = staged
        .iter()
        .map(|(descriptor, _)| ToolName::try_from(descriptor.tool_name).map_err(anyhow::Error::msg))
        .collect::<anyhow::Result<_>>()?;
    let mut retired = Vec::new();
    for component in components
        .list_staged_components_for_environment(env, auth)
        .await?
    {
        if keep.contains(&component.id)
            || !component
                .metadata
                .tools()
                .keys()
                .any(|name| tools.contains(name))
        {
            continue;
        }
        let remaining: Vec<Tool> = component
            .metadata
            .tools()
            .iter()
            .filter(|(name, _)| !tools.contains(*name))
            .map(|(_, metadata)| metadata.definition.clone())
            .collect();
        let update = ComponentUpdate {
            current_revision: component.revision,
            config_schema: None,
            component_provision_config: None,
            agent_types: None,
            agent_type_provision_config_updates: None,
            tools: Some(remaining),
            tool_deployment_config_updates: None,
            tool_middlewares: None,
            tool_middleware_provision_config_updates: None,
            allow_incompatible_config: false,
        };
        match writes.update(component.id, update, None, None, auth).await {
            Ok(_) => {}
            // Another replica retired it first.
            Err(ComponentError::ConcurrentUpdate) => {
                let current = components.get_staged_component(component.id, auth).await?;
                if current
                    .metadata
                    .tools()
                    .keys()
                    .any(|name| tools.contains(name))
                {
                    anyhow::bail!(
                        "built-in tool component '{}' changed while it was being retired",
                        component.component_name
                    );
                }
            }
            Err(error) => return Err(error.into()),
        }
        retired.push(component.component_name.to_string());
    }
    Ok(retired)
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
    descriptor: &BuiltinToolDescriptor,
    tools: Vec<Tool>,
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
                tool_middlewares: vec![],
                tool_middleware_provision_configs: BTreeMap::new(),
            },
            descriptor.wasm_bytes.to_vec(),
            None,
            auth,
        )
        .await
    {
        Ok(value) => Ok(value),
        Err(ComponentError::ComponentWithNameAlreadyExists(_)) => loop {
            let existing = reads.get_staged_component_by_name(env, &name, auth).await?;
            let expected =
                golem_common::model::diff::Hash::new(blake3::hash(descriptor.wasm_bytes));
            if existing.wasm_hash == expected && component_has_intended_tools(&existing, &tools) {
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
                        tool_middlewares: None,
                        tool_middleware_provision_config_updates: None,
                        allow_incompatible_config: false,
                    },
                    Some(descriptor.wasm_bytes.to_vec()),
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

fn component_has_intended_tools(component: &Component, tools: &[Tool]) -> bool {
    let stored = component.metadata.tools();
    stored.len() == tools.len()
        && tools.iter().all(|tool| {
            tool.name()
                .and_then(|name| ToolName::try_from(name).ok())
                .and_then(|name| stored.get(&name))
                .is_some_and(|metadata| metadata.definition == *tool)
        })
}

async fn deploy(
    reads: &Arc<DeploymentService>,
    writes: &Arc<DeploymentWriteService>,
    env: EnvironmentId,
    auth: &AuthCtx,
) -> anyhow::Result<()> {
    let plan = reads.get_current_deployment_plan(env, auth).await?;
    match writes
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
        .await
    {
        Ok(_) => Ok(()),
        // Another replica's own boot-time provisioning may have deployed this exact revision
        // between our plan and our create_deployment call. The caller (provision_descriptors)
        // always re-reads the deployed component right after deploy() returns and bails with a
        // clear error if it doesn't match what we staged, so losing this race is not itself a
        // failure -- only actually failing to converge on our staged revision is.
        Err(DeploymentWriteError::ConcurrentDeployment) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
