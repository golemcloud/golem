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

use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, ToolDeploymentState, ToolName,
};
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::environment::EnvironmentState;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
pub enum ToolDiscoveryError {
    Retrieval(WorkerExecutorError),
    InconsistentSnapshot { details: String },
}

impl Display for ToolDiscoveryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retrieval(error) => error.fmt(f),
            Self::InconsistentSnapshot { details } => {
                write!(f, "Inconsistent tool deployment snapshot: {details}")
            }
        }
    }
}

impl Error for ToolDiscoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Retrieval(error) => Some(error),
            Self::InconsistentSnapshot { .. } => None,
        }
    }
}

impl From<WorkerExecutorError> for ToolDiscoveryError {
    fn from(value: WorkerExecutorError) -> Self {
        Self::Retrieval(value)
    }
}

pub fn get_accessible_tools_from_snapshot(
    deployment: Option<&ToolDeploymentState>,
    agent_type: &AgentTypeName,
) -> Result<Vec<RegisteredTool>, ToolDiscoveryError> {
    let Some(deployment) = deployment else {
        return Ok(Vec::new());
    };
    let Some(bindings) = deployment.agent_tool_bindings.get(agent_type) else {
        return Ok(Vec::new());
    };

    bindings
        .keys()
        .map(|tool_name| {
            deployment
                .registered_tools
                .get(tool_name)
                .cloned()
                .ok_or_else(|| ToolDiscoveryError::InconsistentSnapshot {
                    details: format!(
                        "binding for agent type '{}' references missing tool '{}'",
                        agent_type.0, tool_name
                    ),
                })
        })
        .collect()
}

pub fn get_accessible_tool_from_snapshot(
    deployment: Option<&ToolDeploymentState>,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
) -> Result<Option<RegisteredTool>, ToolDiscoveryError> {
    let Some(deployment) = deployment else {
        return Ok(None);
    };
    let Some(bindings) = deployment.agent_tool_bindings.get(agent_type) else {
        return Ok(None);
    };
    if !bindings.contains_key(tool_name) {
        return Ok(None);
    }

    deployment
        .registered_tools
        .get(tool_name)
        .cloned()
        .map(Some)
        .ok_or_else(|| ToolDiscoveryError::InconsistentSnapshot {
            details: format!(
                "binding for agent type '{}' references missing tool '{}'",
                agent_type.0, tool_name
            ),
        })
}

#[async_trait]
pub trait EnvironmentStateService: Send + Sync {
    /// Get the current deployment of the agent.
    /// Will return None if there is no current deployment.
    async fn get_agent_deployment(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError>;

    async fn get_agent_secrets(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<HashMap<CanonicalAgentSecretPath, AgentSecret>, WorkerExecutorError>;

    async fn get_agent_secret_revision(
        &self,
        environment_id: EnvironmentId,
        agent_secret_id: AgentSecretId,
        path: CanonicalAgentSecretPath,
        revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, WorkerExecutorError>;

    async fn get_retry_policies(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<NamedRetryPolicy>, WorkerExecutorError>;

    async fn get_registered_tool(
        &self,
        _environment_id: EnvironmentId,
        _tool_name: &ToolName,
    ) -> Result<Option<RegisteredTool>, WorkerExecutorError> {
        Ok(None)
    }

    async fn get_agent_tool_binding(
        &self,
        _environment_id: EnvironmentId,
        _agent_type: &AgentTypeName,
        _tool_name: &ToolName,
    ) -> Result<Option<CompiledToolBinding>, WorkerExecutorError> {
        Ok(None)
    }

    async fn get_accessible_tools(
        &self,
        _environment_id: EnvironmentId,
        _agent_type: &AgentTypeName,
    ) -> Result<Vec<RegisteredTool>, ToolDiscoveryError> {
        Ok(Vec::new())
    }

    async fn get_accessible_tool(
        &self,
        _environment_id: EnvironmentId,
        _agent_type: &AgentTypeName,
        _tool_name: &ToolName,
    ) -> Result<Option<RegisteredTool>, ToolDiscoveryError> {
        Ok(None)
    }

    async fn invalidate_environment(&self, _environment_id: EnvironmentId) {}
    async fn invalidate_all(&self) {}
}

pub struct GrpcEnvironmentStateService {
    client: Arc<dyn RegistryService>,
    cached_environment_state: Cache<EnvironmentId, (), Arc<EnvironmentState>, WorkerExecutorError>,
}

impl GrpcEnvironmentStateService {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        cache_capacity: usize,
        cache_ttl: Duration,
        cache_eviction_interval: Duration,
    ) -> Self {
        Self {
            client: registry_service,
            cached_environment_state: Cache::new(
                Some(cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: cache_ttl,
                    period: cache_eviction_interval,
                },
                "gprc_environment_statue_service_environments",
            ),
        }
    }

    async fn get_environment_state(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Arc<EnvironmentState>, WorkerExecutorError> {
        self.cached_environment_state
            .get_or_insert_simple(&environment_id, || {
                Box::pin(async move {
                    let result = self
                        .client
                        .get_current_environment_state(environment_id)
                        .await
                        .map_err(|e| {
                            WorkerExecutorError::runtime(format!(
                                "Failed to get domains for agent types: {e}"
                            ))
                        })?;

                    Ok(Arc::new(result))
                })
            })
            .await
    }
}

#[async_trait]
impl EnvironmentStateService for GrpcEnvironmentStateService {
    async fn get_agent_deployment(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        Ok(environment_state
            .agent_deployment_details
            .get(agent_type)
            .cloned())
    }

    async fn get_agent_secrets(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<HashMap<CanonicalAgentSecretPath, AgentSecret>, WorkerExecutorError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        Ok(environment_state.agent_secrets.clone())
    }

    async fn get_agent_secret_revision(
        &self,
        environment_id: EnvironmentId,
        agent_secret_id: AgentSecretId,
        path: CanonicalAgentSecretPath,
        revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, WorkerExecutorError> {
        self.client
            .get_agent_secret_revision(environment_id, agent_secret_id, path, revision)
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!("Failed to get agent secret revision: {e}"))
            })
    }

    async fn get_retry_policies(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<NamedRetryPolicy>, WorkerExecutorError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        Ok(environment_state.retry_policies.clone())
    }

    async fn get_registered_tool(
        &self,
        environment_id: EnvironmentId,
        tool_name: &ToolName,
    ) -> Result<Option<RegisteredTool>, WorkerExecutorError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        Ok(environment_state
            .tool_deployment
            .as_ref()
            .and_then(|deployment| deployment.registered_tools.get(tool_name))
            .cloned())
    }

    async fn get_agent_tool_binding(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
    ) -> Result<Option<CompiledToolBinding>, WorkerExecutorError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        Ok(environment_state
            .tool_deployment
            .as_ref()
            .and_then(|deployment| deployment.agent_tool_bindings.get(agent_type))
            .and_then(|bindings| bindings.get(tool_name))
            .cloned())
    }

    async fn get_accessible_tools(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
    ) -> Result<Vec<RegisteredTool>, ToolDiscoveryError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        get_accessible_tools_from_snapshot(environment_state.tool_deployment.as_ref(), agent_type)
    }

    async fn get_accessible_tool(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
    ) -> Result<Option<RegisteredTool>, ToolDiscoveryError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        get_accessible_tool_from_snapshot(
            environment_state.tool_deployment.as_ref(),
            agent_type,
            tool_name,
        )
    }

    async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        self.cached_environment_state.remove(&environment_id).await;
    }

    async fn invalidate_all(&self) {
        let keys = self.cached_environment_state.keys().await;
        for key in keys {
            self.cached_environment_state.remove(&key).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ToolDiscoveryError, get_accessible_tool_from_snapshot, get_accessible_tools_from_snapshot,
    };
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::tool::{
        CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolDeploymentState, ToolName,
        ToolProvisionConfig, ToolSource,
    };
    use golem_common::schema::SchemaGraph;
    use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
    use std::collections::BTreeMap;
    use test_r::test;

    fn registered_tool(name: &str, deployment_revision: DeploymentRevision) -> RegisteredTool {
        RegisteredTool {
            deployment_revision,
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
            source: ToolSource::Component {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::try_from(1_u64).unwrap(),
                component_name: ComponentName(format!("tools:{name}")),
            },
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            metadata_version: "0.1.0".to_string(),
        }
    }

    fn binding(
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
        registered_tool: &RegisteredTool,
    ) -> CompiledToolBinding {
        CompiledToolBinding {
            deployment_revision: registered_tool.deployment_revision,
            agent_type_name: agent_type.clone(),
            tool_name: tool_name.clone(),
            version: registered_tool.definition.version.clone(),
            metadata_version: registered_tool.metadata_version.clone(),
            account_id: registered_tool.owner_account_id,
            account_email: registered_tool.owner_account_email.clone(),
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            source: registered_tool.source.clone(),
        }
    }

    fn deployment_state() -> (ToolDeploymentState, AgentTypeName, AgentTypeName) {
        let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
        let agent_a = AgentTypeName("AgentA".to_string());
        let agent_b = AgentTypeName("AgentB".to_string());
        let alpha_name = ToolName::try_from("alpha").unwrap();
        let beta_name = ToolName::try_from("beta").unwrap();
        let unbound_name = ToolName::try_from("unbound").unwrap();
        let alpha = registered_tool(alpha_name.as_str(), deployment_revision);
        let beta = registered_tool(beta_name.as_str(), deployment_revision);
        let unbound = registered_tool(unbound_name.as_str(), deployment_revision);

        (
            ToolDeploymentState {
                deployment_revision,
                registered_tools: BTreeMap::from([
                    (alpha_name.clone(), alpha.clone()),
                    (beta_name.clone(), beta.clone()),
                    (unbound_name, unbound),
                ]),
                agent_tool_bindings: BTreeMap::from([
                    (
                        agent_a.clone(),
                        BTreeMap::from([
                            (alpha_name.clone(), binding(&agent_a, &alpha_name, &alpha)),
                            (beta_name.clone(), binding(&agent_a, &beta_name, &beta)),
                        ]),
                    ),
                    (
                        agent_b.clone(),
                        BTreeMap::from([(beta_name.clone(), binding(&agent_b, &beta_name, &beta))]),
                    ),
                ]),
            },
            agent_a,
            agent_b,
        )
    }

    #[test]
    fn accessible_tools_join_bindings_and_registrations_in_name_order() {
        let (deployment, agent_a, agent_b) = deployment_state();
        let beta = ToolName::try_from("beta").unwrap();

        let agent_a_tools =
            get_accessible_tools_from_snapshot(Some(&deployment), &agent_a).unwrap();
        let agent_b_tools =
            get_accessible_tools_from_snapshot(Some(&deployment), &agent_b).unwrap();

        assert_eq!(
            agent_a_tools
                .iter()
                .map(|tool| tool.definition.name().unwrap())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(
            agent_b_tools
                .iter()
                .map(|tool| tool.definition.name().unwrap())
                .collect::<Vec<_>>(),
            vec!["beta"]
        );
        assert!(
            agent_a_tools
                .iter()
                .chain(&agent_b_tools)
                .all(|tool| tool.deployment_revision == deployment.deployment_revision)
        );
        assert!(
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_a, &beta)
                .unwrap()
                .is_some()
        );
        assert!(
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_b, &beta)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn accessible_tool_requires_a_binding_for_the_agent() {
        let (deployment, agent_a, agent_b) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        let unbound = ToolName::try_from("unbound").unwrap();

        assert!(
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_a, &alpha)
                .unwrap()
                .is_some()
        );
        assert!(
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_b, &alpha)
                .unwrap()
                .is_none()
        );
        assert!(
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_a, &unbound)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_valid_tool_name_does_not_change_accessible_set() {
        let (deployment, agent_a, _) = deployment_state();
        let unknown = ToolName::try_from("unknown").unwrap();
        let before = get_accessible_tools_from_snapshot(Some(&deployment), &agent_a).unwrap();

        assert!(
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_a, &unknown)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            get_accessible_tools_from_snapshot(Some(&deployment), &agent_a).unwrap(),
            before
        );
    }

    #[test]
    fn missing_deployment_or_agent_bindings_are_empty() {
        let (deployment, _, _) = deployment_state();
        let missing_agent = AgentTypeName("MissingAgent".to_string());
        let alpha = ToolName::try_from("alpha").unwrap();

        assert!(
            get_accessible_tools_from_snapshot(None, &missing_agent)
                .unwrap()
                .is_empty()
        );
        assert!(
            get_accessible_tools_from_snapshot(Some(&deployment), &missing_agent)
                .unwrap()
                .is_empty()
        );
        assert!(
            get_accessible_tool_from_snapshot(None, &missing_agent, &alpha)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn dangling_binding_is_a_permanent_integrity_error() {
        let (mut deployment, agent_a, _) = deployment_state();
        let beta = ToolName::try_from("beta").unwrap();
        deployment.registered_tools.remove(&beta);

        let list_error =
            get_accessible_tools_from_snapshot(Some(&deployment), &agent_a).unwrap_err();
        let get_error =
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_a, &beta).unwrap_err();

        assert!(matches!(
            list_error,
            ToolDiscoveryError::InconsistentSnapshot { .. }
        ));
        assert!(matches!(
            get_error,
            ToolDiscoveryError::InconsistentSnapshot { .. }
        ));
    }

    #[test]
    fn single_lookup_does_not_scan_unrelated_dangling_bindings() {
        let (mut deployment, agent_a, _) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        let beta = ToolName::try_from("beta").unwrap();
        deployment.registered_tools.remove(&beta);

        assert!(
            get_accessible_tool_from_snapshot(Some(&deployment), &agent_a, &alpha)
                .unwrap()
                .is_some()
        );
    }
}
