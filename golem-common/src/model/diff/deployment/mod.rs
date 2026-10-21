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

use super::{BTreeSetDiff, HttpApiDeployment, McpDeployment};
use crate::model::account::{AccountEmail, AccountId};
use crate::model::agent::AgentTypeName;
use crate::model::component::{ComponentId, ComponentName};
use crate::model::diff::DiffError;
use crate::model::diff::component::Component;
use crate::model::diff::hash::{Hash, HashOf, Hashable, hash_from_serialized_value};
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use crate::model::json::NormalizedJsonValue;
use crate::model::mcp_import::McpImport;
use crate::model::tool::{
    CompiledToolBinding, ConfigKeyScope, RegisteredTool, SecretKeyScope, ToolBindingInput,
    ToolFilesystemAccess, ToolName, ToolProvisionConfig, ToolSource,
};
use crate::model::tool_middleware::{
    RegisteredToolMiddleware, ToolMiddlewareInstallation, ToolMiddlewareMergeMode,
};
use crate::model::tool_middleware_release::{
    ToolMiddlewareReleaseId, ToolMiddlewareReleaseMetadata, tool_middleware_source_digest,
};
use crate::model::tool_release::ToolReleaseId;
use crate::schema::tool::compatibility::ToolCompatibilityMode;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveToolBinding {
    pub parameters: NormalizedJsonValue,
    pub config_keys_readable: ConfigKeyScope,
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
    pub filesystem_access: ToolFilesystemAccess,
}

pub fn effective_tool_binding(
    environment: Option<&ToolBindingInput>,
    agent: Option<&ToolBindingInput>,
) -> Option<(EffectiveToolBinding, bool)> {
    let (parameters, config_keys_readable, readable, requested_revealable, filesystem_access) =
        match (environment, agent) {
            (None, None) => return None,
            (Some(binding), None) | (None, Some(binding)) => (
                binding.parameters.clone(),
                binding.config_keys_readable.clone(),
                binding.secret_keys_readable.clone(),
                binding.secret_keys_revealable.clone(),
                binding.filesystem_access,
            ),
            (Some(environment), Some(agent)) => {
                let mut parameters = environment
                    .parameters
                    .0
                    .as_object()
                    .expect("validated tool binding parameters are objects")
                    .clone();
                parameters.extend(
                    agent
                        .parameters
                        .0
                        .as_object()
                        .expect("validated tool binding parameters are objects")
                        .clone(),
                );
                (
                    NormalizedJsonValue::new(serde_json::Value::Object(parameters)),
                    environment
                        .config_keys_readable
                        .intersection(&agent.config_keys_readable),
                    environment
                        .secret_keys_readable
                        .intersection(&agent.secret_keys_readable),
                    environment
                        .secret_keys_revealable
                        .intersection(&agent.secret_keys_revealable),
                    match (environment.filesystem_access, agent.filesystem_access) {
                        (ToolFilesystemAccess::Denied, _) | (_, ToolFilesystemAccess::Denied) => {
                            ToolFilesystemAccess::Denied
                        }
                        (ToolFilesystemAccess::Allowed, _) | (_, ToolFilesystemAccess::Allowed) => {
                            ToolFilesystemAccess::Allowed
                        }
                        _ => ToolFilesystemAccess::Unset,
                    },
                )
            }
        };
    let revealable = requested_revealable.intersection(&readable);
    let revealable_scope_narrowed = revealable != requested_revealable;
    Some((
        EffectiveToolBinding {
            parameters,
            config_keys_readable,
            secret_keys_readable: readable,
            secret_keys_revealable: revealable,
            filesystem_access,
        },
        revealable_scope_narrowed,
    ))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteToolDeployment {
    pub release_id: ToolReleaseId,
    pub version: String,
    pub source_digest: Hash,
    pub owner_account_id: AccountId,
    pub owner_account_email: AccountEmail,
    pub metadata_version: String,
    pub metadata_digest: Hash,
    pub provision: ToolProvisionConfig,
    pub component_bindings: BTreeMap<String, EffectiveToolBinding>,
    pub bindings: BTreeMap<AgentTypeName, EffectiveToolBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_middleware_binding: Option<ToolMiddlewareBindingInput>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub component_middleware_bindings: BTreeMap<String, ToolMiddlewareBindingInput>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_middleware_bindings: BTreeMap<AgentTypeName, ToolMiddlewareBindingInput>,
}

impl Hashable for RemoteToolDeployment {
    fn hash(&self) -> Result<Hash, DiffError> {
        hash_from_serialized_value(self)
    }
}

impl Diffable for RemoteToolDeployment {
    type DiffResult = RemoteToolDeployment;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        Ok((new != current).then(|| new.clone()))
    }
}

impl Hashable for McpImport {
    fn hash(&self) -> Result<Hash, DiffError> {
        hash_from_serialized_value(self)
    }
}

impl Diffable for McpImport {
    type DiffResult = McpImport;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        Ok((new != current).then(|| new.clone()))
    }
}

pub fn remote_tool_deployments(
    registered_tools: impl IntoIterator<Item = RegisteredTool>,
    bindings: impl IntoIterator<Item = CompiledToolBinding>,
    environment_tool_bindings: &BTreeMap<ToolName, ToolBindingInput>,
    agent_tool_bindings: &BTreeMap<AgentTypeName, BTreeMap<ToolName, ToolBindingInput>>,
    component_names: &BTreeMap<ComponentId, ComponentName>,
    published_tools: &BTreeSet<String>,
) -> Result<BTreeMap<String, HashOf<RemoteToolDeployment>>, DiffError> {
    let mut bindings_by_tool =
        BTreeMap::<ToolName, BTreeMap<AgentTypeName, EffectiveToolBinding>>::new();
    let mut component_bindings_by_tool =
        BTreeMap::<ToolName, BTreeMap<String, EffectiveToolBinding>>::new();
    for binding in bindings {
        let effective = EffectiveToolBinding {
            parameters: binding.parameters,
            config_keys_readable: binding.config_keys_readable,
            secret_keys_readable: binding.secret_keys_readable,
            secret_keys_revealable: binding.secret_keys_revealable,
            filesystem_access: binding.filesystem_access,
        };
        match binding.owner {
            crate::model::tool::ToolBindingOwner::AgentType { agent_type_name } => {
                bindings_by_tool
                    .entry(binding.tool_name)
                    .or_default()
                    .insert(agent_type_name, effective);
            }
            crate::model::tool::ToolBindingOwner::ComponentBaseline { component_id } => {
                if let Some(component_name) = component_names.get(&component_id) {
                    component_bindings_by_tool
                        .entry(binding.tool_name)
                        .or_default()
                        .insert(component_name.0.clone(), effective);
                }
            }
        }
    }

    registered_tools
        .into_iter()
        .filter_map(|tool| {
            let name = match tool.definition.name() {
                Some(name) => match ToolName::try_from(name) {
                    Ok(name) => name,
                    Err(reason) => {
                        return Some(Err(DiffError::RemoteToolIdentityInvariantViolation {
                            reason,
                        }));
                    }
                },
                None => {
                    return Some(Err(DiffError::RemoteToolIdentityInvariantViolation {
                        reason: "registered tool definition has no root name".to_string(),
                    }));
                }
            };
            if published_tools.contains(name.as_str()) {
                return tool.release_id.is_none().then(|| {
                    Err(DiffError::RemoteToolIdentityInvariantViolation {
                        reason: format!("published tool {name} has no release id"),
                    })
                });
            }
            let Some(release_id) = tool.release_id else {
                return matches!(tool.source, ToolSource::Host { .. }).then(|| {
                    Err(DiffError::RemoteToolIdentityInvariantViolation {
                        reason: format!("non-local tool {name} has no release id"),
                    })
                });
            };
            let bindings = bindings_by_tool.remove(&name).unwrap_or_default();
            let component_bindings = component_bindings_by_tool.remove(&name).unwrap_or_default();
            let environment_middleware_binding = environment_tool_bindings
                .get(&name)
                .map(ToolMiddlewareBindingInput::from);
            let component_middleware_bindings = tool
                .component_bindings
                .iter()
                .map(|(component, binding)| {
                    (
                        component.0.clone(),
                        ToolMiddlewareBindingInput::from(binding),
                    )
                })
                .collect();
            let agent_middleware_bindings = agent_tool_bindings
                .iter()
                .filter_map(|(agent, bindings)| {
                    bindings
                        .get(&name)
                        .map(|binding| (agent.clone(), ToolMiddlewareBindingInput::from(binding)))
                })
                .collect();
            Some(Ok((
                name.to_string(),
                RemoteToolDeployment {
                    release_id,
                    version: tool.definition.version,
                    source_digest: crate::model::tool_release::tool_source_digest(&tool.source),
                    owner_account_id: tool.owner_account_id,
                    owner_account_email: tool.owner_account_email,
                    metadata_version: tool.metadata_version,
                    metadata_digest: tool.metadata_digest,
                    provision: tool.provision,
                    component_bindings,
                    bindings,
                    environment_middleware_binding,
                    component_middleware_bindings,
                    agent_middleware_bindings,
                }
                .into(),
            )))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteToolMiddlewareDeployment {
    pub release_id: ToolMiddlewareReleaseId,
    pub version: String,
    pub source_digest: Hash,
    pub owner_account_id: AccountId,
    pub owner_account_email: AccountEmail,
    pub metadata_version: String,
    pub metadata_digest: Hash,
    pub provision: ToolProvisionConfig,
}

impl RemoteToolMiddlewareDeployment {
    pub fn from_metadata(
        metadata: &ToolMiddlewareReleaseMetadata,
        owner_account_id: AccountId,
        owner_account_email: AccountEmail,
        provision: ToolProvisionConfig,
    ) -> Self {
        Self {
            release_id: metadata.id,
            version: metadata.version.clone(),
            source_digest: metadata.source_digest,
            owner_account_id,
            owner_account_email,
            metadata_version: metadata.metadata_version.clone(),
            metadata_digest: metadata.metadata_digest,
            provision,
        }
    }
}

impl Hashable for RemoteToolMiddlewareDeployment {
    fn hash(&self) -> Result<Hash, DiffError> {
        hash_from_serialized_value(self)
    }
}

impl Diffable for RemoteToolMiddlewareDeployment {
    type DiffResult = RemoteToolMiddlewareDeployment;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        Ok((new != current).then(|| new.clone()))
    }
}

pub fn remote_tool_middleware_deployments(
    registered_middlewares: impl IntoIterator<Item = RegisteredToolMiddleware>,
    published_middlewares: &BTreeSet<String>,
) -> Result<BTreeMap<String, HashOf<RemoteToolMiddlewareDeployment>>, DiffError> {
    registered_middlewares
        .into_iter()
        .filter_map(|middleware| {
            let name = middleware.definition.name.clone();
            if published_middlewares.contains(&name) {
                return middleware.release_id.is_none().then(|| {
                    Err(DiffError::RemoteToolIdentityInvariantViolation {
                        reason: format!("published tool middleware {name} has no release id"),
                    })
                });
            }
            let release_id = middleware.release_id?;
            Some(Ok((
                name,
                RemoteToolMiddlewareDeployment {
                    release_id,
                    version: middleware.definition.version.clone(),
                    source_digest: tool_middleware_source_digest(&middleware.source),
                    owner_account_id: middleware.owner_account_id,
                    owner_account_email: middleware.owner_account_email,
                    metadata_version: middleware.metadata_version,
                    metadata_digest: middleware.metadata_digest,
                    provision: middleware.provision,
                }
                .into(),
            )))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMiddlewareBindingInput {
    pub config_keys_readable: ConfigKeyScope,
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
    pub middleware: Option<Vec<ToolMiddlewareInstallation>>,
    pub middleware_merge_mode: Option<ToolMiddlewareMergeMode>,
}

impl From<&ToolBindingInput> for ToolMiddlewareBindingInput {
    fn from(value: &ToolBindingInput) -> Self {
        Self {
            config_keys_readable: value.config_keys_readable.clone(),
            secret_keys_readable: value.secret_keys_readable.clone(),
            secret_keys_revealable: value.secret_keys_revealable.clone(),
            middleware: value.middleware.clone(),
            middleware_merge_mode: value.middleware_merge_mode,
        }
    }
}

impl Diffable for ToolMiddlewareBindingInput {
    type DiffResult = ToolMiddlewareBindingInput;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        Ok((new != current).then(|| new.clone()))
    }
}

pub fn tool_middleware_binding_inputs(
    environment: &BTreeMap<ToolName, ToolBindingInput>,
    agents: &BTreeMap<AgentTypeName, BTreeMap<ToolName, ToolBindingInput>>,
) -> (
    BTreeMap<String, ToolMiddlewareBindingInput>,
    BTreeMap<String, BTreeMap<String, ToolMiddlewareBindingInput>>,
) {
    let environment = environment
        .iter()
        .map(|(name, binding)| (name.to_string(), binding.into()))
        .collect();
    let agents = agents
        .iter()
        .filter_map(|(agent, bindings)| {
            let bindings = bindings
                .iter()
                .map(|(name, binding)| (name.to_string(), binding.into()))
                .collect::<BTreeMap<_, _>>();
            (!bindings.is_empty()).then(|| (agent.0.clone(), bindings))
        })
        .collect();
    (environment, agents)
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub components: BTreeMap<String, HashOf<Component>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub http_api_deployments: BTreeMap<String, HashOf<HttpApiDeployment>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub mcp_deployments: BTreeMap<String, HashOf<McpDeployment>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub mcp_imports: BTreeMap<String, HashOf<McpImport>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub remote_tools: BTreeMap<String, HashOf<RemoteToolDeployment>>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub published_tools: BTreeSet<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub remote_tool_middleware_deployments:
        BTreeMap<String, HashOf<RemoteToolMiddlewareDeployment>>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub published_tool_middlewares: BTreeSet<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub universal_tool_middlewares: Vec<ToolMiddlewareInstallation>,
    pub tool_compatibility_mode: ToolCompatibilityMode,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub environment_tool_middleware_bindings: BTreeMap<String, ToolMiddlewareBindingInput>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_tool_middleware_bindings:
        BTreeMap<String, BTreeMap<String, ToolMiddlewareBindingInput>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDiff {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMapDiff<String, HashOf<Component>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub http_api_deployments: BTreeMapDiff<String, HashOf<HttpApiDeployment>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_deployments: BTreeMapDiff<String, HashOf<McpDeployment>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_imports: BTreeMapDiff<String, HashOf<McpImport>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub remote_tools: BTreeMapDiff<String, HashOf<RemoteToolDeployment>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub published_tools: BTreeSetDiff<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub remote_tool_middleware_deployments:
        BTreeMapDiff<String, HashOf<RemoteToolMiddlewareDeployment>>,
    pub published_tool_middlewares: BTreeSetDiff<String>,
    pub universal_tool_middlewares_changed: bool,
    pub tool_compatibility_mode_changed: bool,
    pub environment_tool_middleware_bindings: BTreeMapDiff<String, ToolMiddlewareBindingInput>,
    pub agent_tool_middleware_bindings:
        BTreeMapDiff<String, BTreeMap<String, ToolMiddlewareBindingInput>>,
}

impl Diffable for Deployment {
    type DiffResult = DeploymentDiff;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        let components = new.components.diff_with_current(&current.components)?;
        let http_api_deployments = new
            .http_api_deployments
            .diff_with_current(&current.http_api_deployments)?;
        let mcp_deployments = new
            .mcp_deployments
            .diff_with_current(&current.mcp_deployments)?;
        let mcp_imports = new.mcp_imports.diff_with_current(&current.mcp_imports)?;
        let remote_tools = new.remote_tools.diff_with_current(&current.remote_tools)?;
        let published_tools = new
            .published_tools
            .diff_with_current(&current.published_tools)?;
        let remote_tool_middleware_deployments = new
            .remote_tool_middleware_deployments
            .diff_with_current(&current.remote_tool_middleware_deployments)?;
        let published_tool_middlewares = new
            .published_tool_middlewares
            .diff_with_current(&current.published_tool_middlewares)?;
        let environment_tool_middleware_bindings = new
            .environment_tool_middleware_bindings
            .diff_with_current(&current.environment_tool_middleware_bindings)?;
        let agent_tool_middleware_bindings = new
            .agent_tool_middleware_bindings
            .diff_with_current(&current.agent_tool_middleware_bindings)?;
        let universal_tool_middlewares_changed =
            new.universal_tool_middlewares != current.universal_tool_middlewares;
        let tool_compatibility_mode_changed =
            new.tool_compatibility_mode != current.tool_compatibility_mode;

        Ok(
            if components.is_some()
                || http_api_deployments.is_some()
                || mcp_deployments.is_some()
                || mcp_imports.is_some()
                || remote_tools.is_some()
                || published_tools.is_some()
                || remote_tool_middleware_deployments.is_some()
                || published_tool_middlewares.is_some()
                || universal_tool_middlewares_changed
                || tool_compatibility_mode_changed
                || environment_tool_middleware_bindings.is_some()
                || agent_tool_middleware_bindings.is_some()
            {
                Some(DeploymentDiff {
                    components: components.unwrap_or_default(),
                    http_api_deployments: http_api_deployments.unwrap_or_default(),
                    mcp_deployments: mcp_deployments.unwrap_or_default(),
                    mcp_imports: mcp_imports.unwrap_or_default(),
                    remote_tools: remote_tools.unwrap_or_default(),
                    published_tools: published_tools.unwrap_or_default(),
                    remote_tool_middleware_deployments: remote_tool_middleware_deployments
                        .unwrap_or_default(),
                    published_tool_middlewares: published_tool_middlewares.unwrap_or_default(),
                    universal_tool_middlewares_changed,
                    tool_compatibility_mode_changed,
                    environment_tool_middleware_bindings: environment_tool_middleware_bindings
                        .unwrap_or_default(),
                    agent_tool_middleware_bindings: agent_tool_middleware_bindings
                        .unwrap_or_default(),
                })
            } else {
                None
            },
        )
    }
}

impl Hashable for Deployment {
    fn hash(&self) -> Result<Hash, DiffError> {
        let mut deployment = self.clone();
        deployment.published_tools.clear();
        deployment.published_tool_middlewares.clear();
        hash_from_serialized_value(&deployment)
    }
}

#[cfg(test)]
mod tests;
