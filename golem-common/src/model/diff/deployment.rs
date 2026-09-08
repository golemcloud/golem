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
use crate::model::diff::DiffError;
use crate::model::diff::component::Component;
use crate::model::diff::hash::{Hash, HashOf, Hashable, hash_from_serialized_value};
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use crate::model::json::NormalizedJsonValue;
use crate::model::tool::{
    CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolBindingInput, ToolFilesystemAccess,
    ToolName, ToolProvisionConfig, ToolSource,
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
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
    pub filesystem_access: ToolFilesystemAccess,
}

pub fn effective_tool_binding(
    environment: Option<&ToolBindingInput>,
    agent: Option<&ToolBindingInput>,
) -> Option<(EffectiveToolBinding, bool)> {
    let (parameters, readable, requested_revealable, filesystem_access) = match (environment, agent)
    {
        (None, None) => return None,
        (Some(binding), None) | (None, Some(binding)) => (
            binding.parameters.clone(),
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
    pub bindings: BTreeMap<AgentTypeName, EffectiveToolBinding>,
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

pub fn remote_tool_deployments(
    registered_tools: impl IntoIterator<Item = RegisteredTool>,
    bindings: impl IntoIterator<Item = CompiledToolBinding>,
    published_tools: &BTreeSet<String>,
) -> Result<BTreeMap<String, HashOf<RemoteToolDeployment>>, DiffError> {
    let mut bindings_by_tool =
        BTreeMap::<ToolName, BTreeMap<AgentTypeName, EffectiveToolBinding>>::new();
    for binding in bindings {
        bindings_by_tool
            .entry(binding.tool_name)
            .or_default()
            .insert(
                binding.agent_type_name,
                EffectiveToolBinding {
                    parameters: binding.parameters,
                    secret_keys_readable: binding.secret_keys_readable,
                    secret_keys_revealable: binding.secret_keys_revealable,
                    filesystem_access: binding.filesystem_access,
                },
            );
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
                    bindings,
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
    pub middleware: Option<Vec<ToolMiddlewareInstallation>>,
    pub middleware_merge_mode: ToolMiddlewareMergeMode,
}

impl From<&ToolBindingInput> for ToolMiddlewareBindingInput {
    fn from(value: &ToolBindingInput) -> Self {
        Self {
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
        .filter(|(_, binding)| binding.middleware.is_some())
        .map(|(name, binding)| (name.to_string(), binding.into()))
        .collect();
    let agents = agents
        .iter()
        .filter_map(|(agent, bindings)| {
            let bindings = bindings
                .iter()
                .filter(|(_, binding)| binding.middleware.is_some())
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
mod tests {
    use super::{
        Deployment, EffectiveToolBinding, RemoteToolDeployment, remote_tool_deployments,
        remote_tool_middleware_deployments,
    };
    use crate::model::account::{AccountEmail, AccountId};
    use crate::model::agent::AgentTypeName;
    use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
    use crate::model::deployment::DeploymentRevision;
    use crate::model::diff::{Hash, Hashable};
    use crate::model::json::NormalizedJsonValue;
    use crate::model::tool::{
        HostToolId, RegisteredTool, SecretKeyScope, ToolFilesystemAccess, ToolProvisionConfig,
        ToolSource,
    };
    use crate::model::tool_middleware::{RegisteredToolMiddleware, ToolMiddlewareSource};
    use crate::model::tool_middleware_release::ToolMiddlewareReleaseId;
    use crate::model::tool_release::ToolReleaseId;
    use crate::schema::SchemaGraph;
    use crate::schema::tool::{
        CommandNode, CommandTree, Doc, Globals, Tool, ToolMiddleware, ToolMiddlewareScope,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use test_r::test;

    fn remote_tool() -> RemoteToolDeployment {
        RemoteToolDeployment {
            release_id: ToolReleaseId::new(),
            version: "1.0.0".to_string(),
            source_digest: crate::model::tool_release::tool_source_digest(&ToolSource::Component {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                component_name: ComponentName("publisher-tools".to_string()),
            }),
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("publisher@example.com"),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Hash::new(blake3::hash(b"metadata-a")),
            provision: ToolProvisionConfig::default(),
            bindings: BTreeMap::new(),
        }
    }

    fn deployment_hash(tool: RemoteToolDeployment, published: bool) -> Hash {
        Deployment {
            remote_tools: BTreeMap::from([("grep".to_string(), tool.into())]),
            published_tools: if published {
                BTreeSet::from(["local-tool".to_string()])
            } else {
                BTreeSet::new()
            },
            ..Deployment::default()
        }
        .hash()
        .unwrap()
    }

    fn registered_middleware(
        deployment_revision: DeploymentRevision,
        release_id: ToolMiddlewareReleaseId,
    ) -> RegisteredToolMiddleware {
        RegisteredToolMiddleware {
            deployment_revision,
            release_id: Some(release_id),
            definition: ToolMiddleware {
                name: "audit".to_string(),
                version: "1.0.0".to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                scope: ToolMiddlewareScope::Universal,
            },
            provision: ToolProvisionConfig::default(),
            source: ToolMiddlewareSource::Component {
                component_id: ComponentId(uuid::Uuid::from_u128(1)),
                component_revision: ComponentRevision::INITIAL,
                component_name: ComponentName("middleware".to_string()),
            },
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Hash::new(blake3::hash(b"metadata")),
        }
    }

    #[test]
    fn middleware_registration_hash_ignores_deployment_revision_and_local_publication_id() {
        let release_a = ToolMiddlewareReleaseId::new();
        let release_b = ToolMiddlewareReleaseId::new();
        let first = registered_middleware(DeploymentRevision::INITIAL, release_a);
        let mut later = first.clone();
        later.deployment_revision = DeploymentRevision::try_from(42_u64).unwrap();
        let remote_a = remote_tool_middleware_deployments([first], &BTreeSet::new()).unwrap();
        let remote_b = remote_tool_middleware_deployments([later], &BTreeSet::new()).unwrap();
        assert_eq!(
            remote_a["audit"].hash().unwrap(),
            remote_b["audit"].hash().unwrap()
        );

        let published = BTreeSet::from(["audit".to_string()]);
        let local_a = remote_tool_middleware_deployments(
            [registered_middleware(
                DeploymentRevision::INITIAL,
                release_a,
            )],
            &published,
        )
        .unwrap();
        let local_b = remote_tool_middleware_deployments(
            [registered_middleware(
                DeploymentRevision::INITIAL,
                release_b,
            )],
            &published,
        )
        .unwrap();
        assert!(local_a.is_empty());
        assert!(local_b.is_empty());
    }

    #[test]
    fn remote_middleware_metadata_changes_hash() {
        let release = ToolMiddlewareReleaseId::new();
        let first = registered_middleware(DeploymentRevision::INITIAL, release);
        let mut changed = first.clone();
        changed.metadata_digest = Hash::new(blake3::hash(b"changed metadata"));
        let first = remote_tool_middleware_deployments([first], &BTreeSet::new()).unwrap();
        let changed = remote_tool_middleware_deployments([changed], &BTreeSet::new()).unwrap();
        assert_ne!(
            first["audit"].hash().unwrap(),
            changed["audit"].hash().unwrap()
        );
    }

    #[test]
    fn middleware_snapshot_changes_deployment_hash() {
        let base = Deployment::default().hash().unwrap();
        let with_middleware = Deployment {
            universal_tool_middlewares: vec![
                crate::model::tool_middleware::ToolMiddlewareInstallation {
                    name: "audit".try_into().unwrap(),
                    version: Some("1.0.0".to_string()),
                    parameters: NormalizedJsonValue::new(serde_json::json!({})),
                    account: None,
                    filesystem_access: ToolFilesystemAccess::Denied,
                },
            ],
            ..Deployment::default()
        }
        .hash()
        .unwrap();

        assert_ne!(base, with_middleware);
    }

    #[test]
    fn middleware_filesystem_policy_changes_deployment_hash() {
        let installation =
            |filesystem_access| crate::model::tool_middleware::ToolMiddlewareInstallation {
                name: "audit".try_into().unwrap(),
                version: None,
                parameters: NormalizedJsonValue::new(serde_json::json!({})),
                account: None,
                filesystem_access,
            };
        let hash = |filesystem_access| {
            Deployment {
                universal_tool_middlewares: vec![installation(filesystem_access)],
                ..Deployment::default()
            }
            .hash()
            .unwrap()
        };

        assert_ne!(
            hash(ToolFilesystemAccess::Allowed),
            hash(ToolFilesystemAccess::Denied)
        );
    }

    fn registered_tool(
        name: &str,
        source: ToolSource,
        release_id: Option<ToolReleaseId>,
    ) -> RegisteredTool {
        RegisteredTool {
            deployment_revision: DeploymentRevision::INITIAL,
            release_id,
            definition: Tool {
                version: "1.0.0".to_string(),
                commands: CommandTree {
                    nodes: vec![CommandNode {
                        name: name.to_string(),
                        aliases: Vec::new(),
                        doc: Doc::default(),
                        globals: Globals::default(),
                        subcommands: Vec::new(),
                        body: None,
                    }],
                },
                schema: SchemaGraph::empty(),
            },
            provision: ToolProvisionConfig::default(),
            source,
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Hash::new(blake3::hash(name.as_bytes())),
        }
    }

    #[test]
    fn remote_tool_classification_uses_release_and_publication_identity() {
        let local = registered_tool(
            "local",
            ToolSource::Component {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                component_name: ComponentName("local-component".to_string()),
            },
            None,
        );
        let published_release_id = ToolReleaseId::new();
        let published = registered_tool(
            "published",
            ToolSource::Component {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                component_name: ComponentName("publisher".to_string()),
            },
            Some(published_release_id),
        );
        let remote_release_id = ToolReleaseId::new();
        let remote = registered_tool(
            "remote",
            ToolSource::Host {
                host_tool_id: HostToolId::try_from("remote-host".to_string()).unwrap(),
                implementation_version: "1".to_string(),
            },
            Some(remote_release_id),
        );

        let classified = remote_tool_deployments(
            [local, published, remote],
            Vec::new(),
            &BTreeSet::from(["published".to_string()]),
        )
        .unwrap();

        assert_eq!(
            classified.keys().map(String::as_str).collect::<Vec<_>>(),
            ["remote"]
        );
        assert_eq!(
            classified["remote"].as_value().unwrap().release_id,
            remote_release_id
        );
    }

    #[test]
    fn remote_host_tool_without_release_identity_is_rejected() {
        let tool = registered_tool(
            "remote",
            ToolSource::Host {
                host_tool_id: HostToolId::try_from("remote-host".to_string()).unwrap(),
                implementation_version: "1".to_string(),
            },
            None,
        );

        let error = remote_tool_deployments([tool], Vec::new(), &BTreeSet::new()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("non-local tool remote has no release id")
        );
    }

    #[test]
    fn remote_tool_identity_hash_covers_release_source_metadata_provision_and_bindings() {
        let base = remote_tool();
        let base_hash = deployment_hash(base.clone(), false);

        let mut changed_release = base.clone();
        changed_release.release_id = ToolReleaseId::new();
        assert_ne!(base_hash, deployment_hash(changed_release, false));

        let mut changed_source = base.clone();
        changed_source.source_digest =
            crate::model::tool_release::tool_source_digest(&ToolSource::Host {
                host_tool_id: HostToolId::try_from("native-grep".to_string()).unwrap(),
                implementation_version: "2026.08".to_string(),
            });
        assert_ne!(base_hash, deployment_hash(changed_source, false));

        let mut changed_metadata = base.clone();
        changed_metadata.metadata_digest = Hash::new(blake3::hash(b"metadata-b"));
        assert_ne!(base_hash, deployment_hash(changed_metadata, false));

        let mut changed_provision = base.clone();
        changed_provision.provision.config =
            NormalizedJsonValue::new(serde_json::json!({ "consumer": true }));
        assert_ne!(base_hash, deployment_hash(changed_provision, false));

        let mut changed_binding = base.clone();
        changed_binding.bindings.insert(
            AgentTypeName("Agent".to_string()),
            EffectiveToolBinding {
                parameters: NormalizedJsonValue::new(serde_json::json!({ "limit": 5 })),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
            },
        );
        assert_ne!(base_hash, deployment_hash(changed_binding, false));

        assert_eq!(base_hash, deployment_hash(base, true));
    }
}
