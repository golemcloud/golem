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
use crate::model::component::{ComponentId, ComponentRevision};
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
use crate::model::tool_release::ToolReleaseId;
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
    let (parameters, readable, requested_revealable) = match (environment, agent) {
        (None, None) => return None,
        (Some(binding), None) | (None, Some(binding)) => (
            binding.parameters.clone(),
            binding.secret_keys_readable.clone(),
            binding.secret_keys_revealable.clone(),
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
            filesystem_access: ToolFilesystemAccess::Unset,
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
    local_component_revisions: &BTreeSet<(ComponentId, ComponentRevision)>,
) -> BTreeMap<String, HashOf<RemoteToolDeployment>> {
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
            let is_local = match &tool.source {
                ToolSource::Component {
                    component_id,
                    component_revision,
                    ..
                } => local_component_revisions.contains(&(*component_id, *component_revision)),
                ToolSource::Host { .. } => false,
            };
            if is_local {
                return None;
            }
            let release_id = tool.release_id?;
            let name = ToolName::try_from(tool.definition.name()?).ok()?;
            let bindings = bindings_by_tool.remove(&name).unwrap_or_default();
            Some((
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
            ))
        })
        .collect()
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

        Ok(
            if components.is_some()
                || http_api_deployments.is_some()
                || mcp_deployments.is_some()
                || remote_tools.is_some()
                || published_tools.is_some()
            {
                Some(DeploymentDiff {
                    components: components.unwrap_or_default(),
                    http_api_deployments: http_api_deployments.unwrap_or_default(),
                    mcp_deployments: mcp_deployments.unwrap_or_default(),
                    remote_tools: remote_tools.unwrap_or_default(),
                    published_tools: published_tools.unwrap_or_default(),
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
        hash_from_serialized_value(&deployment)
    }
}

#[cfg(test)]
mod tests {
    use super::{Deployment, EffectiveToolBinding, RemoteToolDeployment};
    use crate::model::account::{AccountEmail, AccountId};
    use crate::model::agent::AgentTypeName;
    use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
    use crate::model::diff::{Hash, Hashable};
    use crate::model::json::NormalizedJsonValue;
    use crate::model::tool::{
        HostToolId, SecretKeyScope, ToolFilesystemAccess, ToolProvisionConfig, ToolSource,
    };
    use crate::model::tool_release::ToolReleaseId;
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
