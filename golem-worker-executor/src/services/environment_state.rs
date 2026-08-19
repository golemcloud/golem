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
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::entity::{
    EntityActivation, EntityActivationPolicy, ExecutableTarget, FilesystemCapability,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, ToolDeploymentState, ToolFilesystemAccess, ToolName,
    ToolSource,
};
use golem_common::schema::tool::DiscoveredTool;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::environment::EnvironmentState;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

type ToolDiscoveryCacheKey = (EnvironmentId, ComponentId, ComponentRevision);

struct ToolDiscoveryCache {
    values: Arc<
        Cache<ToolDiscoveryCacheKey, (), Option<Arc<ToolDiscoverySnapshot>>, WorkerExecutorError>,
    >,
    invalidation_guard: Arc<tokio::sync::RwLock<()>>,
}

impl ToolDiscoveryCache {
    fn new(capacity: usize, ttl: Duration, eviction_interval: Duration) -> ToolDiscoveryCache {
        Self {
            values: Arc::new(Cache::new(
                Some(capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl,
                    period: eviction_interval,
                },
                "grpc_environment_state_service_tool_discovery",
            )),
            invalidation_guard: Arc::new(tokio::sync::RwLock::new(())),
        }
    }

    async fn get_or_insert<F, Fut>(
        &self,
        key: &ToolDiscoveryCacheKey,
        load: F,
    ) -> Result<Option<Arc<ToolDiscoverySnapshot>>, WorkerExecutorError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<Arc<ToolDiscoverySnapshot>>, WorkerExecutorError>>
            + Send
            + 'static,
    {
        let guard = self.invalidation_guard.clone().read_owned().await;
        let values = self.values.clone();
        let key = *key;
        tokio::spawn(async move {
            let _guard = guard;
            values
                .get_or_insert_simple(&key, async move || load().await)
                .await
        })
        .await
        .map_err(|error| {
            WorkerExecutorError::runtime(format!("Tool discovery cache task failed: {error}"))
        })?
    }

    async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        let _guard = self.invalidation_guard.write().await;
        let keys = self.values.keys().await;
        for key in keys {
            if key.0 == environment_id {
                self.values.remove(&key).await;
            }
        }
    }

    async fn invalidate_all(&self) {
        let _guard = self.invalidation_guard.write().await;
        let keys = self.values.keys().await;
        for key in keys {
            self.values.remove(&key).await;
        }
    }
}

#[derive(Debug)]
pub enum ToolDiscoveryError {
    Retrieval(WorkerExecutorError),
    AgentContextRequired,
    InconsistentSnapshot { details: String },
}

impl ToolDiscoveryError {
    fn dangling_binding(agent_type: &AgentTypeName, tool_name: &ToolName) -> Self {
        Self::InconsistentSnapshot {
            details: format!(
                "binding for agent type '{}' references missing tool '{}'",
                agent_type.0, tool_name
            ),
        }
    }
}

impl Display for ToolDiscoveryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retrieval(error) => error.fmt(f),
            Self::AgentContextRequired => write!(f, "Tool discovery requires an agent context"),
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
            Self::AgentContextRequired | Self::InconsistentSnapshot { .. } => None,
        }
    }
}

impl From<WorkerExecutorError> for ToolDiscoveryError {
    fn from(value: WorkerExecutorError) -> Self {
        Self::Retrieval(value)
    }
}

pub struct ToolDiscoverySnapshot {
    registered_tools: BTreeMap<ToolName, Arc<DiscoveredTool>>,
    agent_tool_bindings: BTreeMap<AgentTypeName, BTreeSet<ToolName>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolActivationSnapshot {
    registered_tool: RegisteredTool,
    binding: CompiledToolBinding,
    filesystem: FilesystemCapability,
}

impl ToolActivationSnapshot {
    pub fn registered_tool(&self) -> &RegisteredTool {
        &self.registered_tool
    }

    pub fn binding(&self) -> &CompiledToolBinding {
        &self.binding
    }

    pub fn filesystem(&self) -> FilesystemCapability {
        self.filesystem
    }

    pub fn into_entity_activation(self) -> Result<EntityActivation, ToolDiscoveryError> {
        let ToolSource::Component {
            component_id,
            component_revision,
            ..
        } = self.registered_tool.source;
        EntityActivation::new(
            ExecutableTarget::new(component_id, component_revision),
            self.registered_tool.deployment_revision,
            EntityActivationPolicy::Tool {
                provision: self.registered_tool.provision,
                binding: self.binding,
            },
            self.filesystem,
        )
        .map_err(|details| ToolDiscoveryError::InconsistentSnapshot { details })
    }
}

pub fn get_tool_activation_from_deployment(
    deployment: Option<&ToolDeploymentState>,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
) -> Result<Option<ToolActivationSnapshot>, ToolDiscoveryError> {
    let Some(deployment) = deployment else {
        return Ok(None);
    };
    let binding = deployment
        .agent_tool_bindings
        .get(agent_type)
        .and_then(|bindings| bindings.get(tool_name));
    let registered_tool = deployment.registered_tools.get(tool_name);

    let Some(binding) = binding else {
        return Ok(None);
    };
    let Some(registered_tool) = registered_tool else {
        return Err(ToolDiscoveryError::dangling_binding(agent_type, tool_name));
    };

    let consistent = registered_tool.deployment_revision == deployment.deployment_revision
        && registered_tool
            .definition
            .name()
            .is_some_and(|name| name == tool_name.as_str())
        && binding.deployment_revision == deployment.deployment_revision
        && binding.agent_type_name == *agent_type
        && binding.tool_name == *tool_name
        && binding.version == registered_tool.definition.version
        && binding.metadata_version == registered_tool.metadata_version
        && binding.account_id == registered_tool.owner_account_id
        && binding.account_email == registered_tool.owner_account_email
        && binding.source == registered_tool.source
        && binding
            .secret_keys_revealable
            .is_subset_of(&binding.secret_keys_readable);

    if !consistent {
        return Err(ToolDiscoveryError::InconsistentSnapshot {
            details: format!(
                "registration and binding for agent type '{}' and tool '{}' do not describe one deployment activation",
                agent_type.0, tool_name
            ),
        });
    }

    let filesystem = match (
        binding.filesystem_access,
        registered_tool.provision.files.is_empty(),
    ) {
        (ToolFilesystemAccess::Allowed, _) | (ToolFilesystemAccess::Unset, false) => {
            FilesystemCapability::Capable
        }
        (ToolFilesystemAccess::Denied, false) => {
            return Err(ToolDiscoveryError::InconsistentSnapshot {
                details: format!(
                    "tool '{}' denies filesystem access but declares provisioned files",
                    tool_name
                ),
            });
        }
        (ToolFilesystemAccess::Denied | ToolFilesystemAccess::Unset, true) => {
            FilesystemCapability::Incapable
        }
    };

    Ok(Some(ToolActivationSnapshot {
        filesystem,
        registered_tool: registered_tool.clone(),
        binding: binding.clone(),
    }))
}

impl From<ToolDeploymentState> for ToolDiscoverySnapshot {
    fn from(value: ToolDeploymentState) -> Self {
        let ToolDeploymentState {
            registered_tools,
            agent_tool_bindings,
            ..
        } = value;

        Self {
            registered_tools: registered_tools
                .into_iter()
                .map(|(name, tool)| (name, Arc::new(tool.into())))
                .collect(),
            agent_tool_bindings: agent_tool_bindings
                .into_iter()
                .map(|(agent_type, bindings)| (agent_type, bindings.into_keys().collect()))
                .collect(),
        }
    }
}

pub fn get_accessible_tools_from_snapshot(
    snapshot: Option<&ToolDiscoverySnapshot>,
    agent_type: &AgentTypeName,
) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let Some(snapshot) = snapshot else {
        return Ok(Vec::new());
    };
    let Some(bindings) = snapshot.agent_tool_bindings.get(agent_type) else {
        return Ok(Vec::new());
    };

    bindings
        .iter()
        .map(|tool_name| {
            snapshot
                .registered_tools
                .get(tool_name)
                .cloned()
                .ok_or_else(|| ToolDiscoveryError::dangling_binding(agent_type, tool_name))
        })
        .collect()
}

pub fn get_accessible_tool_from_snapshot(
    snapshot: Option<&ToolDiscoverySnapshot>,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    let Some(bindings) = snapshot.agent_tool_bindings.get(agent_type) else {
        return Ok(None);
    };
    if !bindings.contains(tool_name) {
        return Ok(None);
    }

    snapshot
        .registered_tools
        .get(tool_name)
        .cloned()
        .map(Some)
        .ok_or_else(|| ToolDiscoveryError::dangling_binding(agent_type, tool_name))
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

    async fn get_tool_activation(
        &self,
        _environment_id: EnvironmentId,
        _agent_type: &AgentTypeName,
        _tool_name: &ToolName,
    ) -> Result<Option<ToolActivationSnapshot>, ToolDiscoveryError> {
        Ok(None)
    }

    async fn get_accessible_tools(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _agent_type: &AgentTypeName,
    ) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        Ok(Vec::new())
    }

    async fn get_accessible_tool(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _agent_type: &AgentTypeName,
        _tool_name: &ToolName,
    ) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        Ok(None)
    }

    async fn invalidate_environment(&self, _environment_id: EnvironmentId) {}
    async fn invalidate_all(&self) {}
}

pub struct GrpcEnvironmentStateService {
    client: Arc<dyn RegistryService>,
    cached_environment_state: Cache<EnvironmentId, (), Arc<EnvironmentState>, WorkerExecutorError>,
    cached_tool_discovery: ToolDiscoveryCache,
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
            cached_tool_discovery: ToolDiscoveryCache::new(
                cache_capacity,
                cache_ttl,
                cache_eviction_interval,
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

    async fn get_tool_discovery_snapshot(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<Arc<ToolDiscoverySnapshot>>, WorkerExecutorError> {
        let key = (environment_id, component_id, component_revision);
        let client = self.client.clone();
        self.cached_tool_discovery
            .get_or_insert(&key, move || async move {
                client
                    .get_tool_deployment_state(environment_id, component_id, component_revision)
                    .await
                    .map(|deployment| deployment.map(|deployment| Arc::new(deployment.into())))
                    .map_err(|error| {
                        WorkerExecutorError::runtime(format!(
                            "Failed to get tool deployment state: {error}"
                        ))
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

    async fn get_tool_activation(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
    ) -> Result<Option<ToolActivationSnapshot>, ToolDiscoveryError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        get_tool_activation_from_deployment(
            environment_state.tool_deployment.as_ref(),
            agent_type,
            tool_name,
        )
    }

    async fn get_accessible_tools(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type: &AgentTypeName,
    ) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        let snapshot = self
            .get_tool_discovery_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_accessible_tools_from_snapshot(snapshot.as_deref(), agent_type)
    }

    async fn get_accessible_tool(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
    ) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        let snapshot = self
            .get_tool_discovery_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_accessible_tool_from_snapshot(snapshot.as_deref(), agent_type, tool_name)
    }

    async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        self.cached_environment_state.remove(&environment_id).await;
        self.cached_tool_discovery
            .invalidate_environment(environment_id)
            .await;
    }

    async fn invalidate_all(&self) {
        let keys = self.cached_environment_state.keys().await;
        for key in keys {
            self.cached_environment_state.remove(&key).await;
        }
        self.cached_tool_discovery.invalidate_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ToolDiscoveryCache, ToolDiscoveryError, ToolDiscoverySnapshot,
        get_accessible_tool_from_snapshot, get_accessible_tools_from_snapshot,
        get_tool_activation_from_deployment,
    };
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{AgentFileContentHash, AgentTypeName};
    use golem_common::model::component::{
        AgentFilePath, AgentFilePermissions, ComponentId, ComponentName, ComponentRevision,
        InitialAgentFile,
    };
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::entity::FilesystemCapability;
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::tool::{
        CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolDeploymentState,
        ToolFilesystemAccess, ToolName, ToolProvisionConfig, ToolSource,
    };
    use golem_common::schema::SchemaGraph;
    use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::{test, timeout};

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
            filesystem_access: ToolFilesystemAccess::Unset,
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
        let alpha = ToolName::try_from("alpha").unwrap();
        let beta = ToolName::try_from("beta").unwrap();
        let ToolSource::Component { component_id, .. } =
            &deployment.registered_tools[&alpha].source;
        let expected_alpha_component = *component_id;
        let snapshot = ToolDiscoverySnapshot::from(deployment);

        let agent_a_tools = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap();
        let agent_b_tools = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_b).unwrap();

        assert_eq!(
            agent_a_tools
                .iter()
                .map(|tool| tool.definition.name().unwrap())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(agent_a_tools[0].implemented_by, expected_alpha_component);
        assert_eq!(
            agent_b_tools
                .iter()
                .map(|tool| tool.definition.name().unwrap())
                .collect::<Vec<_>>(),
            vec!["beta"]
        );
        let beta_for_agent_a = get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &beta)
            .unwrap()
            .unwrap();
        let beta_for_agent_b = get_accessible_tool_from_snapshot(Some(&snapshot), &agent_b, &beta)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&agent_a_tools[1], &beta_for_agent_a));
        assert!(Arc::ptr_eq(&beta_for_agent_a, &beta_for_agent_b));
    }

    #[test]
    fn accessible_tool_requires_a_binding_for_the_agent() {
        let (deployment, agent_a, agent_b) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        let unbound = ToolName::try_from("unbound").unwrap();
        let snapshot = ToolDiscoverySnapshot::from(deployment);

        assert!(
            get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &alpha)
                .unwrap()
                .is_some()
        );
        assert!(
            get_accessible_tool_from_snapshot(Some(&snapshot), &agent_b, &alpha)
                .unwrap()
                .is_none()
        );
        assert!(
            get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &unbound)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_valid_tool_name_does_not_change_accessible_set() {
        let (deployment, agent_a, _) = deployment_state();
        let unknown = ToolName::try_from("unknown").unwrap();
        let snapshot = ToolDiscoverySnapshot::from(deployment);
        let before = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap();

        assert!(
            get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &unknown)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap(),
            before
        );
    }

    #[test]
    fn missing_deployment_or_agent_bindings_are_empty() {
        let (deployment, _, _) = deployment_state();
        let missing_agent = AgentTypeName("MissingAgent".to_string());
        let alpha = ToolName::try_from("alpha").unwrap();
        let snapshot = ToolDiscoverySnapshot::from(deployment);

        assert!(
            get_accessible_tools_from_snapshot(None, &missing_agent)
                .unwrap()
                .is_empty()
        );
        assert!(
            get_accessible_tools_from_snapshot(Some(&snapshot), &missing_agent)
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
        let snapshot = ToolDiscoverySnapshot::from(deployment);

        let list_error = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap_err();
        let get_error =
            get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &beta).unwrap_err();

        let expected_message = concat!(
            "Inconsistent tool deployment snapshot: binding for agent type ",
            "'AgentA' references missing tool 'beta'"
        );
        assert_eq!(list_error.to_string(), expected_message);
        assert_eq!(get_error.to_string(), expected_message);
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
    fn activation_lookup_returns_one_coherent_registration_and_binding() {
        let (deployment, agent_a, _) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();

        let activation = get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha)
            .unwrap()
            .unwrap();

        assert_eq!(
            activation.registered_tool,
            deployment.registered_tools[&alpha]
        );
        assert_eq!(
            activation.binding,
            deployment.agent_tool_bindings[&agent_a][&alpha]
        );
        assert_eq!(activation.filesystem, FilesystemCapability::Incapable);
        assert_eq!(
            activation.into_entity_activation().unwrap().filesystem(),
            FilesystemCapability::Incapable
        );
    }

    #[test]
    fn activation_lookup_uses_explicit_filesystem_verdict() {
        let (mut deployment, agent_a, _) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        deployment
            .agent_tool_bindings
            .get_mut(&agent_a)
            .unwrap()
            .get_mut(&alpha)
            .unwrap()
            .filesystem_access = ToolFilesystemAccess::Allowed;

        let activation = get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha)
            .unwrap()
            .unwrap();

        assert_eq!(activation.filesystem(), FilesystemCapability::Capable);
    }

    #[test]
    fn activation_lookup_rejects_files_with_explicit_filesystem_denial() {
        let (mut deployment, agent_a, _) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        deployment
            .agent_tool_bindings
            .get_mut(&agent_a)
            .unwrap()
            .get_mut(&alpha)
            .unwrap()
            .filesystem_access = ToolFilesystemAccess::Denied;
        deployment
            .registered_tools
            .get_mut(&alpha)
            .unwrap()
            .provision
            .files
            .push(InitialAgentFile {
                content_hash: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
                path: AgentFilePath::from_rel_str("fixture").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
                size: 0,
            });

        let result = get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha);

        assert!(matches!(
            result,
            Err(ToolDiscoveryError::InconsistentSnapshot { .. })
        ));
    }

    #[test]
    fn activation_lookup_rejects_cross_revision_pairs() {
        let (mut deployment, agent_a, _) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        deployment
            .agent_tool_bindings
            .get_mut(&agent_a)
            .unwrap()
            .get_mut(&alpha)
            .unwrap()
            .deployment_revision = DeploymentRevision::try_from(2_u64).unwrap();

        let error =
            get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha).unwrap_err();

        assert!(matches!(
            error,
            ToolDiscoveryError::InconsistentSnapshot { .. }
        ));
    }

    #[test]
    fn activation_lookup_rejects_registration_under_the_wrong_name() {
        let (mut deployment, agent_a, _) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        deployment
            .registered_tools
            .get_mut(&alpha)
            .unwrap()
            .definition
            .commands
            .nodes[0]
            .name = "other".to_string();

        let error =
            get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha).unwrap_err();

        assert!(matches!(
            error,
            ToolDiscoveryError::InconsistentSnapshot { .. }
        ));
    }

    #[test]
    fn single_lookup_does_not_scan_unrelated_dangling_bindings() {
        let (mut deployment, agent_a, _) = deployment_state();
        let alpha = ToolName::try_from("alpha").unwrap();
        let beta = ToolName::try_from("beta").unwrap();
        deployment.registered_tools.remove(&beta);
        let snapshot = ToolDiscoverySnapshot::from(deployment);

        assert!(
            get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &alpha)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    #[timeout("30s")]
    async fn tool_discovery_invalidation_cannot_be_undone_by_an_in_flight_fill() {
        let cache = Arc::new(ToolDiscoveryCache::new(
            8,
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        let environment_id = golem_common::model::environment::EnvironmentId::new();
        let key = (
            environment_id,
            ComponentId::new(),
            ComponentRevision::try_from(1_u64).unwrap(),
        );
        let stale_snapshot = Arc::new(ToolDiscoverySnapshot::from(deployment_state().0));
        let fresh_snapshot = Arc::new(ToolDiscoverySnapshot::from(deployment_state().0));
        let lookup_started = Arc::new(tokio::sync::Notify::new());
        let release_lookup = Arc::new(tokio::sync::Notify::new());

        let lookup = tokio::spawn({
            let cache = cache.clone();
            let stale_snapshot = stale_snapshot.clone();
            let lookup_started = lookup_started.clone();
            let release_lookup = release_lookup.clone();
            async move {
                cache
                    .get_or_insert(&key, move || async move {
                        lookup_started.notify_one();
                        release_lookup.notified().await;
                        Ok(Some(stale_snapshot))
                    })
                    .await
                    .unwrap()
                    .unwrap()
            }
        });
        lookup_started.notified().await;

        let invalidation_started = Arc::new(tokio::sync::Notify::new());
        let invalidation = tokio::spawn({
            let cache = cache.clone();
            let invalidation_started = invalidation_started.clone();
            async move {
                invalidation_started.notify_one();
                cache.invalidate_environment(environment_id).await;
            }
        });
        invalidation_started.notified().await;
        for _ in 0..100 {
            if cache.invalidation_guard.try_read().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(cache.invalidation_guard.try_read().is_err());

        release_lookup.notify_one();
        let loaded_stale_snapshot = lookup.await.unwrap();
        assert!(Arc::ptr_eq(&loaded_stale_snapshot, &stale_snapshot));
        invalidation.await.unwrap();

        let loaded_fresh_snapshot = cache
            .get_or_insert(&key, {
                let fresh_snapshot = fresh_snapshot.clone();
                move || async move { Ok(Some(fresh_snapshot)) }
            })
            .await
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&loaded_fresh_snapshot, &fresh_snapshot));
    }

    #[test]
    #[timeout("30s")]
    async fn cancelled_tool_discovery_lookup_does_not_wedge_invalidation() {
        let cache = Arc::new(ToolDiscoveryCache::new(
            8,
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        let environment_id = golem_common::model::environment::EnvironmentId::new();
        let key = (
            environment_id,
            ComponentId::new(),
            ComponentRevision::try_from(1_u64).unwrap(),
        );
        let stale_snapshot = Arc::new(ToolDiscoverySnapshot::from(deployment_state().0));
        let fresh_snapshot = Arc::new(ToolDiscoverySnapshot::from(deployment_state().0));
        let lookup_started = Arc::new(tokio::sync::Notify::new());
        let release_lookup = Arc::new(tokio::sync::Notify::new());

        let lookup = tokio::spawn({
            let cache = cache.clone();
            let lookup_started = lookup_started.clone();
            let release_lookup = release_lookup.clone();
            async move {
                cache
                    .get_or_insert(&key, move || async move {
                        lookup_started.notify_one();
                        release_lookup.notified().await;
                        Ok(Some(stale_snapshot))
                    })
                    .await
            }
        });
        lookup_started.notified().await;
        lookup.abort();
        let cancellation = match lookup.await {
            Err(error) => error,
            Ok(_) => panic!("aborted lookup completed successfully"),
        };
        assert!(cancellation.is_cancelled());

        let invalidation_started = Arc::new(tokio::sync::Notify::new());
        let invalidation = tokio::spawn({
            let cache = cache.clone();
            let invalidation_started = invalidation_started.clone();
            async move {
                invalidation_started.notify_one();
                cache.invalidate_environment(environment_id).await;
            }
        });
        invalidation_started.notified().await;
        for _ in 0..100 {
            if cache.invalidation_guard.try_read().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(cache.invalidation_guard.try_read().is_err());

        release_lookup.notify_one();
        invalidation.await.unwrap();

        let loaded_fresh_snapshot = cache
            .get_or_insert(&key, {
                let fresh_snapshot = fresh_snapshot.clone();
                move || async move { Ok(Some(fresh_snapshot)) }
            })
            .await
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&loaded_fresh_snapshot, &fresh_snapshot));
    }

    #[test]
    #[timeout("30s")]
    async fn tool_discovery_cache_retains_background_ttl_eviction() {
        let cache = ToolDiscoveryCache::new(8, Duration::from_millis(20), Duration::from_millis(5));
        let key = (
            golem_common::model::environment::EnvironmentId::new(),
            ComponentId::new(),
            ComponentRevision::try_from(1_u64).unwrap(),
        );
        let stale_snapshot = Arc::new(ToolDiscoverySnapshot::from(deployment_state().0));
        let fresh_snapshot = Arc::new(ToolDiscoverySnapshot::from(deployment_state().0));

        let loaded_stale_snapshot = cache
            .get_or_insert(&key, {
                let stale_snapshot = stale_snapshot.clone();
                move || async move { Ok(Some(stale_snapshot)) }
            })
            .await
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&loaded_stale_snapshot, &stale_snapshot));

        tokio::time::sleep(Duration::from_millis(100)).await;

        let loaded_fresh_snapshot = cache
            .get_or_insert(&key, {
                let fresh_snapshot = fresh_snapshot.clone();
                move || async move { Ok(Some(fresh_snapshot)) }
            })
            .await
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&loaded_fresh_snapshot, &fresh_snapshot));
    }
}
