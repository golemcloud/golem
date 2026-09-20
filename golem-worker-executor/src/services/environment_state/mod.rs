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
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::FilesystemCapability;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_import::McpImportSource;
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, ToolBindingOwner, ToolDeploymentState,
    ToolFilesystemAccess, ToolName, ToolProvisionConfig,
};
pub use golem_common::model::tool::{ToolActivationSnapshot, ToolDispatchTarget};
use golem_common::schema::tool::DiscoveredTool;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::environment::EnvironmentState;
use golem_service_base::model::mcp_import::McpImportObservation;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

type ToolDiscoveryCacheKey = (EnvironmentId, ComponentId, ComponentRevision);
type ToolDeploymentRevisionCacheKey = (EnvironmentId, DeploymentRevision);

struct CachedToolDeployment {
    state: Arc<ToolDeploymentState>,
    discovery: ToolDiscoverySnapshot,
}

impl From<ToolDeploymentState> for CachedToolDeployment {
    fn from(state: ToolDeploymentState) -> Self {
        Self {
            discovery: state.clone().into(),
            state: Arc::new(state),
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

#[derive(Clone, Debug)]
pub enum ToolDiscoveryError {
    Retrieval(WorkerExecutorError),
    Mcp(RegistryServiceError),
    AgentContextRequired,
    MissingDeploymentRevision {
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    },
    InconsistentSnapshot {
        details: String,
    },
}

impl ToolDiscoveryError {
    fn dangling_binding(owner: &ToolBindingOwner, tool_name: &ToolName) -> Self {
        Self::InconsistentSnapshot {
            details: format!("binding for {owner:?} references missing tool '{tool_name}'"),
        }
    }
}

impl Display for ToolDiscoveryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retrieval(error) => error.fmt(f),
            Self::Mcp(error) => error.fmt(f),
            Self::AgentContextRequired => write!(f, "Tool discovery requires an agent context"),
            Self::MissingDeploymentRevision {
                environment_id,
                deployment_revision,
            } => write!(
                f,
                "Tool deployment revision {deployment_revision} does not exist in environment {environment_id}"
            ),
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
            Self::Mcp(error) => Some(error),
            Self::AgentContextRequired
            | Self::MissingDeploymentRevision { .. }
            | Self::InconsistentSnapshot { .. } => None,
        }
    }
}

impl From<WorkerExecutorError> for ToolDiscoveryError {
    fn from(value: WorkerExecutorError) -> Self {
        Self::Retrieval(value)
    }
}

pub struct ToolDiscoverySnapshot {
    owner_tools: BTreeMap<ToolBindingOwner, BTreeMap<ToolName, Arc<DiscoveredTool>>>,
    dangling_bindings: BTreeMap<ToolBindingOwner, BTreeSet<ToolName>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolActivationOutcome {
    Ready(Box<ToolActivationSnapshot>),
    NotBound,
    NotRegistered,
}

pub fn tool_activation_from_mcp(
    mut source: McpImportSource,
    protocol_version: String,
    tool: golem_mcp_import::tool::ProjectedTool,
    deployment: &ToolDeploymentState,
    owner: &golem_service_base::model::component::Component,
    binding_owner: &ToolBindingOwner,
) -> Result<ToolActivationSnapshot, ToolDiscoveryError> {
    use golem_common::model::entity::McpImportActivation;
    use golem_common::model::mcp_import::mcp_import_bridge_source;
    use golem_common::model::tool::{ConfigKeyScope, SecretKeyScope, TOOL_METADATA_WIT_VERSION};
    let invalid = |details: String| ToolDiscoveryError::InconsistentSnapshot { details };
    let tool_name = ToolName::try_from(
        tool.definition
            .name()
            .ok_or_else(|| invalid("unnamed MCP projection".into()))?,
    )
    .map_err(invalid)?;
    let metadata_digest = tool
        .digest
        .parse()
        .map_err(|error| invalid(format!("invalid MCP projection digest: {error}")))?;
    let projected_tool = tool.to_json().map_err(|error| invalid(error.to_string()))?;
    source.upstream_tool_name = tool.upstream_name;
    let registered_tool = RegisteredTool {
        deployment_revision: source.deployment_revision,
        release_id: None,
        definition: tool.definition,
        provision: ToolProvisionConfig::default(),
        component_bindings: BTreeMap::new(),
        source: mcp_import_bridge_source(),
        owner_account_id: owner.account_id,
        owner_account_email: owner.account_email.clone(),
        metadata_version: TOOL_METADATA_WIT_VERSION.into(),
        metadata_digest,
    };
    let binding = CompiledToolBinding {
        deployment_revision: source.deployment_revision,
        release_id: None,
        owner: binding_owner.clone(),
        tool_name: tool_name.clone(),
        version: registered_tool.definition.version.clone(),
        metadata_version: registered_tool.metadata_version.clone(),
        metadata_digest,
        account_id: owner.account_id,
        account_email: owner.account_email.clone(),
        parameters: golem_common::model::json::NormalizedJsonValue::new(serde_json::json!({})),
        config_keys_readable: ConfigKeyScope::Keys(BTreeSet::new()),
        secret_keys_readable: SecretKeyScope::Keys(BTreeSet::new()),
        secret_keys_revealable: SecretKeyScope::Keys(BTreeSet::new()),
        filesystem_access: ToolFilesystemAccess::Denied,
        source: mcp_import_bridge_source(),
    };
    let configuration = &deployment.tool_middleware_configuration;
    let environment_binding = configuration.environment_bindings.get(&tool_name);
    let owner_binding = match binding_owner {
        ToolBindingOwner::AgentType { agent_type_name } => configuration
            .agent_bindings
            .get(agent_type_name)
            .and_then(|bindings| bindings.get(&tool_name)),
        ToolBindingOwner::ComponentBaseline { .. } => None,
    };
    let (config, readable, revealable) = mcp_binding_scopes(environment_binding, owner_binding);
    let mut binding = binding;
    binding.config_keys_readable = config;
    binding.secret_keys_readable = readable;
    binding.secret_keys_revealable = revealable;
    let compiled = golem_common::model::tool_middleware::compile::compile_tool_middleware_chain(
        deployment.deployment_revision,
        &registered_tool.definition,
        &binding,
        &deployment
            .registered_tool_middlewares
            .values()
            .cloned()
            .collect::<Vec<_>>(),
        &configuration.universal,
        environment_binding,
        owner_binding,
        configuration.compatibility_mode,
    );
    if !compiled.errors.is_empty() {
        return Err(invalid(format!(
            "middleware for dynamic tool '{tool_name}' is incompatible: {}",
            compiled
                .errors
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    Ok(ToolActivationSnapshot {
        registered_tool,
        binding,
        middleware_chain: compiled.chains.into_iter().next(),
        filesystem: FilesystemCapability::Incapable,
        mcp_import: Some(Box::new(McpImportActivation {
            source,
            protocol_version,
            projected_tool,
        })),
    })
}

pub(crate) fn mcp_binding_scopes(
    environment: Option<&golem_common::model::tool::ToolBindingInput>,
    agent: Option<&golem_common::model::tool::ToolBindingInput>,
) -> (
    golem_common::model::tool::ConfigKeyScope,
    golem_common::model::tool::SecretKeyScope,
    golem_common::model::tool::SecretKeyScope,
) {
    use golem_common::model::tool::{ConfigKeyScope, SecretKeyScope};
    let (config, readable, revealable) = match (environment, agent) {
        (None, None) => (
            ConfigKeyScope::Keys(BTreeSet::new()),
            SecretKeyScope::Keys(BTreeSet::new()),
            SecretKeyScope::Keys(BTreeSet::new()),
        ),
        (Some(binding), None) | (None, Some(binding)) => (
            binding.config_keys_readable.clone(),
            binding.secret_keys_readable.clone(),
            binding.secret_keys_revealable.clone(),
        ),
        (Some(environment), Some(agent)) => (
            environment
                .config_keys_readable
                .intersection(&agent.config_keys_readable),
            environment
                .secret_keys_readable
                .intersection(&agent.secret_keys_readable),
            environment
                .secret_keys_revealable
                .intersection(&agent.secret_keys_revealable),
        ),
    };
    let revealable = revealable.intersection(&readable);
    (config, readable, revealable)
}

pub fn get_tool_activation_from_deployment(
    deployment: Option<&ToolDeploymentState>,
    owner: &ToolBindingOwner,
    tool_name: &ToolName,
) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
    let Some(deployment) = deployment else {
        return Ok(ToolActivationOutcome::NotRegistered);
    };
    let binding = deployment
        .tool_bindings
        .get(owner)
        .and_then(|bindings| bindings.get(tool_name));
    let registered_tool = deployment.registered_tools.get(tool_name);

    let Some(registered_tool) = registered_tool else {
        return match binding {
            Some(_) => Err(ToolDiscoveryError::dangling_binding(owner, tool_name)),
            None => Ok(ToolActivationOutcome::NotRegistered),
        };
    };
    let Some(binding) = binding else {
        return Ok(ToolActivationOutcome::NotBound);
    };
    let middleware_chain = deployment
        .tool_middleware_chains
        .get(owner)
        .and_then(|chains| chains.get(tool_name));

    let consistent = registered_tool.deployment_revision == deployment.deployment_revision
        && registered_tool
            .definition
            .name()
            .is_some_and(|name| name == tool_name.as_str())
        && binding.deployment_revision == deployment.deployment_revision
        && binding.owner == *owner
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
                "registration and binding for agent type '{owner:?}' and tool '{}' do not describe one deployment activation",
                tool_name
            ),
        });
    }

    if let Some(chain) = middleware_chain {
        let chain_consistent = chain.deployment_revision == deployment.deployment_revision
            && chain.owner == *owner
            && chain.tool_name == *tool_name
            && chain.occurrences.iter().all(|occurrence| {
                occurrence.middleware.deployment_revision == deployment.deployment_revision
                    && golem_common::model::tool_middleware::ToolMiddlewareName::try_from(
                        occurrence.middleware.definition.name.as_str(),
                    )
                    .ok()
                    .and_then(|name| deployment.registered_tool_middlewares.get(&name))
                        == Some(&occurrence.middleware)
            });
        if !chain_consistent {
            return Err(ToolDiscoveryError::InconsistentSnapshot {
                details: format!(
                    "middleware chain for agent type '{owner:?}' and tool '{}' does not describe one deployment activation",
                    tool_name
                ),
            });
        }
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
            mcp_import: None,
            middleware_chain: middleware_chain.cloned(),
        },
    )))
}

impl From<ToolDeploymentState> for ToolDiscoverySnapshot {
    fn from(value: ToolDeploymentState) -> Self {
        let ToolDeploymentState {
            registered_tools,
            tool_bindings,
            tool_middleware_chains,
            ..
        } = value;

        let dangling_bindings = tool_bindings
            .iter()
            .filter_map(|(owner, bindings)| {
                let missing = bindings
                    .keys()
                    .filter(|name| !registered_tools.contains_key(*name))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                (!missing.is_empty()).then(|| (owner.clone(), missing))
            })
            .collect();
        let owner_tools = tool_bindings
            .into_iter()
            .map(|(owner, bindings)| {
                let tools = bindings
                    .into_keys()
                    .filter_map(|name| {
                        registered_tools.get(&name).map(|registered| {
                            let mut discovered: DiscoveredTool = registered.clone().into();
                            discovered.lookup_name = name.to_string();
                            if let Some(chain) = tool_middleware_chains
                                .get(&owner)
                                .and_then(|chains| chains.get(&name))
                            {
                                discovered.definition = chain.effective_definition.clone();
                            }
                            (name, Arc::new(discovered))
                        })
                    })
                    .collect();
                (owner, tools)
            })
            .collect();
        Self {
            owner_tools,
            dangling_bindings,
        }
    }
}

pub fn get_accessible_tools_from_snapshot(
    snapshot: Option<&ToolDiscoverySnapshot>,
    owner: &ToolBindingOwner,
) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let Some(snapshot) = snapshot else {
        return Ok(Vec::new());
    };
    if let Some(missing) = snapshot.dangling_bindings.get(owner)
        && let Some(tool_name) = missing.first()
    {
        return Err(ToolDiscoveryError::dangling_binding(owner, tool_name));
    }
    let Some(tools) = snapshot.owner_tools.get(owner) else {
        return Ok(Vec::new());
    };
    Ok(tools.values().cloned().collect())
}

pub fn get_accessible_tool_from_snapshot(
    snapshot: Option<&ToolDiscoverySnapshot>,
    owner: &ToolBindingOwner,
    tool_name: &ToolName,
) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    if snapshot
        .dangling_bindings
        .get(owner)
        .is_some_and(|missing| missing.contains(tool_name))
    {
        return Err(ToolDiscoveryError::dangling_binding(owner, tool_name));
    }
    Ok(snapshot
        .owner_tools
        .get(owner)
        .and_then(|tools| tools.get(tool_name))
        .cloned())
}

pub fn get_accessible_tools_from_deployment(
    deployment: Option<&ToolDeploymentState>,
    owner: &ToolBindingOwner,
) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let snapshot = deployment.cloned().map(ToolDiscoverySnapshot::from);
    get_accessible_tools_from_snapshot(snapshot.as_ref(), owner)
}

pub fn get_accessible_tool_from_deployment(
    deployment: Option<&ToolDeploymentState>,
    owner: &ToolBindingOwner,
    tool_name: &ToolName,
) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let snapshot = deployment.cloned().map(ToolDiscoverySnapshot::from);
    get_accessible_tool_from_snapshot(snapshot.as_ref(), owner, tool_name)
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

    async fn get_live_tool_deployment_state(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<Option<Arc<ToolDeploymentState>>, ToolDiscoveryError> {
        Ok(None)
    }

    async fn resolve_mcp_import(
        &self,
        _source: &McpImportSource,
        _auth: &AuthCtx,
        _refresh: bool,
    ) -> Result<McpImportObservation, RegistryServiceError> {
        Err(RegistryServiceError::internal_client_error(
            "MCP resolution is unavailable",
        ))
    }

    async fn get_mcp_runtime_credential(
        &self,
        _source: &McpImportSource,
        _auth: &AuthCtx,
    ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
    {
        Err(RegistryServiceError::internal_client_error(
            "MCP credentials are unavailable",
        ))
    }

    async fn report_mcp_resource_unauthorized(
        &self,
        _source: &McpImportSource,
        _auth: &AuthCtx,
        _generation: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        Err(RegistryServiceError::internal_client_error(
            "MCP authorization feedback is unavailable",
        ))
    }

    async fn get_tool_deployment_state_at_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    ) -> Result<Arc<ToolDeploymentState>, ToolDiscoveryError> {
        Err(ToolDiscoveryError::MissingDeploymentRevision {
            environment_id,
            deployment_revision,
        })
    }

    async fn get_tool_activation(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _owner: &ToolBindingOwner,
        _tool_name: &ToolName,
    ) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
        Ok(ToolActivationOutcome::NotRegistered)
    }

    async fn get_accessible_tools(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _owner: &ToolBindingOwner,
    ) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        Ok(Vec::new())
    }

    async fn get_accessible_tool(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _owner: &ToolBindingOwner,
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
    cached_tool_deployment_revisions:
        Cache<ToolDeploymentRevisionCacheKey, (), Arc<ToolDeploymentState>, ToolDiscoveryError>,
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
            cached_tool_deployment_revisions: Cache::new(
                Some(cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: cache_ttl,
                    period: cache_eviction_interval,
                },
                "grpc_environment_state_service_tool_deployment_revisions",
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
    async fn get_mcp_runtime_credential(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
    ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
    {
        self.client.get_mcp_runtime_credential(source, auth).await
    }

    async fn report_mcp_resource_unauthorized(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
        generation: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        self.client
            .report_mcp_resource_unauthorized(source, auth, generation)
            .await
    }

    async fn resolve_mcp_import(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
        refresh: bool,
    ) -> Result<McpImportObservation, RegistryServiceError> {
        let client = self.client.clone();
        let source = source.clone();
        let auth = auth.clone();
        tokio::spawn(async move { client.resolve_mcp_import(&source, &auth, refresh).await })
            .await
            .map_err(|_| {
                RegistryServiceError::internal_client_error("MCP resolution task failed")
            })?
    }

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

    async fn get_live_tool_deployment_state(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<Arc<ToolDeploymentState>>, ToolDiscoveryError> {
        Ok(self
            .get_tool_deployment_snapshot(environment_id, component_id, component_revision)
            .await?
            .map(|deployment| deployment.state.clone()))
    }

    async fn get_tool_deployment_state_at_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    ) -> Result<Arc<ToolDeploymentState>, ToolDiscoveryError> {
        let client = self.client.clone();
        self.cached_tool_deployment_revisions
            .get_or_insert_simple_spawned(
                &(environment_id, deployment_revision),
                move || async move {
                    let state = client
                        .get_tool_deployment_state_at_revision(environment_id, deployment_revision)
                        .await
                        .map_err(|error| {
                            ToolDiscoveryError::Retrieval(WorkerExecutorError::runtime(format!(
                                "Failed to get tool deployment state at revision: {error}"
                            )))
                        })?
                        .ok_or(ToolDiscoveryError::MissingDeploymentRevision {
                            environment_id,
                            deployment_revision,
                        })?;
                    if state.deployment_revision != deployment_revision {
                        return Err(ToolDiscoveryError::InconsistentSnapshot {
                            details: format!(
                                "registry returned tool deployment revision {} when revision {deployment_revision} was requested",
                                state.deployment_revision
                            ),
                        });
                    }
                    Ok(Arc::new(state))
                },
            )
            .await
    }

    async fn get_tool_activation(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
        tool_name: &ToolName,
    ) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
        let snapshot = self
            .get_tool_deployment_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_tool_activation_from_deployment(
            snapshot.as_deref().map(|snapshot| snapshot.state.as_ref()),
            owner,
            tool_name,
        )
    }

    async fn get_accessible_tools(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
    ) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        let snapshot = self
            .get_tool_deployment_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_accessible_tools_from_snapshot(
            snapshot.as_deref().map(|snapshot| &snapshot.discovery),
            owner,
        )
    }

    async fn get_accessible_tool(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
        tool_name: &ToolName,
    ) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        let snapshot = self
            .get_tool_deployment_snapshot(environment_id, component_id, component_revision)
            .await?;
        get_accessible_tool_from_snapshot(
            snapshot.as_deref().map(|snapshot| &snapshot.discovery),
            owner,
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
