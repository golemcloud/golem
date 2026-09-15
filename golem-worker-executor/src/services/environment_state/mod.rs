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
    CompiledToolBinding, HostToolId, RegisteredTool, ToolDeploymentState, ToolFilesystemAccess,
    ToolName, ToolProvisionConfig, ToolSource,
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

struct CachedToolDeployment {
    state: ToolDeploymentState,
    discovery: ToolDiscoverySnapshot,
}

impl From<ToolDeploymentState> for CachedToolDeployment {
    fn from(state: ToolDeploymentState) -> Self {
        Self {
            discovery: state.clone().into(),
            state,
        }
    }
}

struct ToolDiscoveryCache {
    values: Arc<
        Cache<ToolDiscoveryCacheKey, (), Option<Arc<CachedToolDeployment>>, WorkerExecutorError>,
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
    ) -> Result<Option<Arc<CachedToolDeployment>>, WorkerExecutorError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<Arc<CachedToolDeployment>>, WorkerExecutorError>>
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

#[derive(Clone, Debug, PartialEq)]
pub enum ToolActivationOutcome {
    Ready(Box<ToolActivationSnapshot>),
    NotBound,
    NotRegistered,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolDispatchTarget {
    Component(EntityActivation),
    Host {
        host_tool_id: HostToolId,
        implementation_version: String,
        deployment_revision: golem_common::model::deployment::DeploymentRevision,
        provision: ToolProvisionConfig,
        binding: Box<CompiledToolBinding>,
        filesystem: FilesystemCapability,
    },
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

    pub fn into_dispatch_target(self) -> Result<ToolDispatchTarget, ToolDiscoveryError> {
        match self.registered_tool.source {
            ToolSource::Component {
                component_id,
                component_revision,
                ..
            } => EntityActivation::new(
                ExecutableTarget::new(component_id, component_revision),
                self.registered_tool.deployment_revision,
                EntityActivationPolicy::Tool {
                    provision: self.registered_tool.provision,
                    binding: Box::new(self.binding),
                },
                self.filesystem,
            )
            .map(ToolDispatchTarget::Component)
            .map_err(|details| ToolDiscoveryError::InconsistentSnapshot { details }),
            ToolSource::Host {
                host_tool_id,
                implementation_version,
            } => Ok(ToolDispatchTarget::Host {
                host_tool_id,
                implementation_version,
                deployment_revision: self.registered_tool.deployment_revision,
                provision: self.registered_tool.provision,
                binding: Box::new(self.binding),
                filesystem: self.filesystem,
            }),
        }
    }
}

pub fn get_tool_activation_from_deployment(
    deployment: Option<&ToolDeploymentState>,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
    let Some(deployment) = deployment else {
        return Ok(ToolActivationOutcome::NotRegistered);
    };
    let binding = deployment
        .agent_tool_bindings
        .get(agent_type)
        .and_then(|bindings| bindings.get(tool_name));
    let registered_tool = deployment.registered_tools.get(tool_name);

    let Some(registered_tool) = registered_tool else {
        return match binding {
            Some(_) => Err(ToolDiscoveryError::dangling_binding(agent_type, tool_name)),
            None => Ok(ToolActivationOutcome::NotRegistered),
        };
    };
    let Some(binding) = binding else {
        return Ok(ToolActivationOutcome::NotBound);
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
        && binding.release_id == registered_tool.release_id
        && binding.metadata_digest == registered_tool.metadata_digest
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

    Ok(ToolActivationOutcome::Ready(Box::new(
        ToolActivationSnapshot {
            filesystem,
            registered_tool: registered_tool.clone(),
            binding: binding.clone(),
        },
    )))
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

    async fn get_tool_activation(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _agent_type: &AgentTypeName,
        _tool_name: &ToolName,
    ) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
        Ok(ToolActivationOutcome::NotRegistered)
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

    async fn get_tool_deployment_snapshot(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<Arc<CachedToolDeployment>>, WorkerExecutorError> {
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

    async fn get_tool_activation(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
    ) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
        let snapshot = self
            .get_tool_deployment_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_tool_activation_from_deployment(
            snapshot.as_deref().map(|snapshot| &snapshot.state),
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
            .get_tool_deployment_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_accessible_tools_from_snapshot(
            snapshot.as_deref().map(|snapshot| &snapshot.discovery),
            agent_type,
        )
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
            .get_tool_deployment_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_accessible_tool_from_snapshot(
            snapshot.as_deref().map(|snapshot| &snapshot.discovery),
            agent_type,
            tool_name,
        )
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
mod tests;
