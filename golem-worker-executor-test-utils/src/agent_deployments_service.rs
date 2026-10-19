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
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_import::McpImportSource;
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{ToolBindingOwner, ToolDeploymentState, ToolName};
use golem_common::schema::tool::DiscoveredTool;
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::mcp_import::McpImportObservation;
use golem_worker_executor::services::environment_state::{
    EnvironmentStateService, ToolActivationOutcome, ToolDiscoveryError, ToolDiscoverySnapshot,
    get_accessible_tool_from_snapshot, get_accessible_tools_from_snapshot,
    get_tool_activation_from_deployment,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

pub struct DisabledEnvironmentStateService;

#[async_trait]
impl EnvironmentStateService for DisabledEnvironmentStateService {
    async fn get_agent_deployment(
        &self,
        _environment: EnvironmentId,
        _agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn get_agent_secrets(
        &self,
        _environment_id: EnvironmentId,
    ) -> Result<HashMap<CanonicalAgentSecretPath, AgentSecret>, WorkerExecutorError> {
        Ok(HashMap::new())
    }

    async fn get_agent_secret_revision(
        &self,
        _environment_id: EnvironmentId,
        _agent_secret_id: AgentSecretId,
        _path: CanonicalAgentSecretPath,
        _revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, WorkerExecutorError> {
        Ok(None)
    }

    async fn get_retry_policies(
        &self,
        _environment_id: EnvironmentId,
    ) -> Result<Vec<NamedRetryPolicy>, WorkerExecutorError> {
        Ok(vec![])
    }
}

/// Test-only `EnvironmentStateService` that returns a fixed list of
/// named retry policies regardless of environment.  Used by integration
/// tests that exercise manifest-defined retry policies (e.g. status-code
/// retries via `retryPolicyDefaults`).
pub struct ConfiguredRetryPoliciesEnvironmentStateService {
    pub policies: Vec<NamedRetryPolicy>,
}

#[async_trait]
impl EnvironmentStateService for ConfiguredRetryPoliciesEnvironmentStateService {
    async fn get_agent_deployment(
        &self,
        _environment: EnvironmentId,
        _agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn get_agent_secrets(
        &self,
        _environment_id: EnvironmentId,
    ) -> Result<HashMap<CanonicalAgentSecretPath, AgentSecret>, WorkerExecutorError> {
        Ok(HashMap::new())
    }

    async fn get_agent_secret_revision(
        &self,
        _environment_id: EnvironmentId,
        _agent_secret_id: AgentSecretId,
        _path: CanonicalAgentSecretPath,
        _revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, WorkerExecutorError> {
        Ok(None)
    }

    async fn get_retry_policies(
        &self,
        _environment_id: EnvironmentId,
    ) -> Result<Vec<NamedRetryPolicy>, WorkerExecutorError> {
        Ok(self.policies.clone())
    }
}

struct TestToolDeployment {
    state: ToolDeploymentState,
    discovery: Arc<ToolDiscoverySnapshot>,
}

#[derive(Default)]
pub struct TestEnvironmentStateService {
    agent_secrets: RwLock<HashMap<EnvironmentId, HashMap<CanonicalAgentSecretPath, AgentSecret>>>,
    tool_deployments:
        RwLock<HashMap<(EnvironmentId, ComponentId, ComponentRevision), TestToolDeployment>>,
    agent_secret_revision_calls: AtomicUsize,
    accessible_tools_calls: AtomicUsize,
    accessible_tool_calls: AtomicUsize,
    tool_activation_lookups: RwLock<Vec<(EnvironmentId, ComponentId, ComponentRevision)>>,
    tool_deployment_calls: AtomicUsize,
    tool_deployment_lookups: RwLock<Vec<(EnvironmentId, ComponentId, ComponentRevision)>>,
    tool_deployment_revisions:
        RwLock<HashMap<(EnvironmentId, DeploymentRevision), Arc<ToolDeploymentState>>>,
    tool_deployment_revision_lookups: RwLock<Vec<(EnvironmentId, DeploymentRevision)>>,
    mcp_observations: RwLock<
        Vec<(
            McpImportSource,
            Result<McpImportObservation, RegistryServiceError>,
        )>,
    >,
    mcp_observation_requests: RwLock<Vec<(McpImportSource, AuthCtx)>>,
    mcp_observation_refreshes: RwLock<Vec<bool>>,
    mcp_refresh_gate: RwLock<Option<Arc<tokio::sync::Notify>>>,
    mcp_credentials: RwLock<
        Vec<(
            McpImportSource,
            golem_service_base::clients::registry::McpRuntimeCredential,
        )>,
    >,
    mcp_credential_requests: RwLock<Vec<(McpImportSource, AuthCtx)>>,
    mcp_unauthorized_reports: RwLock<Vec<(McpImportSource, Option<uuid::Uuid>)>>,
}

impl TestEnvironmentStateService {
    pub fn set_agent_secret(&self, secret: AgentSecret) {
        self.agent_secrets
            .write()
            .unwrap()
            .entry(secret.environment_id)
            .or_default()
            .insert(secret.path.clone(), secret);
    }

    pub fn agent_secret_revision_calls(&self) -> usize {
        self.agent_secret_revision_calls.load(Ordering::SeqCst)
    }

    pub fn set_tool_deployment(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        deployment: Option<ToolDeploymentState>,
    ) {
        let mut deployments = self.tool_deployments.write().unwrap();
        let key = (environment_id, component_id, component_revision);
        match deployment {
            Some(deployment) => {
                self.tool_deployment_revisions
                    .write()
                    .unwrap()
                    .entry((environment_id, deployment.deployment_revision))
                    .or_insert_with(|| Arc::new(deployment.clone()));
                deployments.insert(
                    key,
                    TestToolDeployment {
                        discovery: Arc::new(deployment.clone().into()),
                        state: deployment,
                    },
                );
            }
            None => {
                deployments.remove(&key);
            }
        }
    }

    pub fn accessible_tools_calls(&self) -> usize {
        self.accessible_tools_calls.load(Ordering::SeqCst)
    }

    pub fn accessible_tool_calls(&self) -> usize {
        self.accessible_tool_calls.load(Ordering::SeqCst)
    }

    pub fn tool_activation_calls(&self) -> usize {
        self.tool_activation_lookups.read().unwrap().len()
    }

    pub fn tool_activation_lookups(&self) -> Vec<(EnvironmentId, ComponentId, ComponentRevision)> {
        self.tool_activation_lookups.read().unwrap().clone()
    }

    pub fn tool_deployment_calls(&self) -> usize {
        self.tool_deployment_calls.load(Ordering::SeqCst)
    }

    pub fn tool_deployment_lookups(&self) -> Vec<(EnvironmentId, ComponentId, ComponentRevision)> {
        self.tool_deployment_lookups.read().unwrap().clone()
    }

    pub fn tool_deployment_revision_lookups(&self) -> Vec<(EnvironmentId, DeploymentRevision)> {
        self.tool_deployment_revision_lookups
            .read()
            .unwrap()
            .clone()
    }

    pub fn remove_tool_deployment_revision(
        &self,
        environment: EnvironmentId,
        revision: DeploymentRevision,
    ) {
        self.tool_deployment_revisions
            .write()
            .unwrap()
            .remove(&(environment, revision));
    }

    pub fn set_mcp_observation(
        &self,
        source: McpImportSource,
        observation: Result<McpImportObservation, RegistryServiceError>,
    ) {
        let mut observations = self.mcp_observations.write().unwrap();
        if let Some((_, current)) = observations
            .iter_mut()
            .find(|(current, _)| current == &source)
        {
            *current = observation;
        } else {
            observations.push((source, observation));
        }
    }

    pub fn clear_mcp_observations(&self) {
        self.mcp_observations.write().unwrap().clear();
    }

    pub fn mcp_observation_requests(&self) -> Vec<(McpImportSource, AuthCtx)> {
        self.mcp_observation_requests.read().unwrap().clone()
    }

    pub fn mcp_observation_refreshes(&self) -> Vec<bool> {
        self.mcp_observation_refreshes.read().unwrap().clone()
    }

    pub fn set_mcp_refresh_gate(&self, gate: Arc<tokio::sync::Notify>) {
        *self.mcp_refresh_gate.write().unwrap() = Some(gate);
    }

    pub fn set_mcp_credential(
        &self,
        source: McpImportSource,
        credential: golem_service_base::clients::registry::McpRuntimeCredential,
    ) {
        self.mcp_credentials
            .write()
            .unwrap()
            .push((source, credential));
    }

    pub fn clear_mcp_credentials(&self) {
        self.mcp_credentials.write().unwrap().clear();
    }

    pub fn mcp_credential_requests(&self) -> Vec<(McpImportSource, AuthCtx)> {
        self.mcp_credential_requests.read().unwrap().clone()
    }

    pub fn mcp_unauthorized_reports(&self) -> Vec<(McpImportSource, Option<uuid::Uuid>)> {
        self.mcp_unauthorized_reports.read().unwrap().clone()
    }
}

#[async_trait]
impl EnvironmentStateService for TestEnvironmentStateService {
    async fn get_mcp_runtime_credential(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
    ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
    {
        self.mcp_credential_requests
            .write()
            .unwrap()
            .push((source.clone(), auth.clone()));
        self.mcp_credentials
            .read()
            .unwrap()
            .iter()
            .rev()
            .find(|(current, _)| current == source)
            .map(|(_, credential)| credential.clone())
            .ok_or_else(|| {
                RegistryServiceError::BadRequest(vec!["test MCP grant unavailable".into()])
            })
    }

    async fn report_mcp_resource_unauthorized(
        &self,
        source: &McpImportSource,
        _auth: &AuthCtx,
        generation: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        self.mcp_unauthorized_reports
            .write()
            .unwrap()
            .push((source.clone(), generation));
        Ok(())
    }

    async fn get_agent_deployment(
        &self,
        _environment_id: EnvironmentId,
        _agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        Ok(None)
    }

    async fn get_agent_secrets(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<HashMap<CanonicalAgentSecretPath, AgentSecret>, WorkerExecutorError> {
        Ok(self
            .agent_secrets
            .read()
            .unwrap()
            .get(&environment_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_agent_secret_revision(
        &self,
        environment_id: EnvironmentId,
        agent_secret_id: AgentSecretId,
        path: CanonicalAgentSecretPath,
        revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, WorkerExecutorError> {
        self.agent_secret_revision_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(self
            .agent_secrets
            .read()
            .unwrap()
            .get(&environment_id)
            .and_then(|secrets| secrets.get(&path))
            .filter(|secret| secret.id == agent_secret_id && secret.revision == revision)
            .cloned())
    }

    async fn get_retry_policies(
        &self,
        _environment_id: EnvironmentId,
    ) -> Result<Vec<NamedRetryPolicy>, WorkerExecutorError> {
        Ok(Vec::new())
    }

    async fn get_live_tool_deployment_state(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<Arc<ToolDeploymentState>>, ToolDiscoveryError> {
        self.tool_deployment_calls.fetch_add(1, Ordering::SeqCst);
        self.tool_deployment_lookups.write().unwrap().push((
            environment_id,
            component_id,
            component_revision,
        ));
        Ok(self
            .tool_deployments
            .read()
            .unwrap()
            .get(&(environment_id, component_id, component_revision))
            .map(|deployment| Arc::new(deployment.state.clone())))
    }

    async fn get_tool_deployment_state_at_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    ) -> Result<Arc<ToolDeploymentState>, ToolDiscoveryError> {
        self.tool_deployment_revision_lookups
            .write()
            .unwrap()
            .push((environment_id, deployment_revision));
        self.tool_deployment_revisions
            .read()
            .unwrap()
            .get(&(environment_id, deployment_revision))
            .cloned()
            .ok_or(ToolDiscoveryError::MissingDeploymentRevision {
                environment_id,
                deployment_revision,
            })
    }

    async fn resolve_mcp_import(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
        refresh: bool,
    ) -> Result<McpImportObservation, RegistryServiceError> {
        self.mcp_observation_requests
            .write()
            .unwrap()
            .push((source.clone(), auth.clone()));
        self.mcp_observation_refreshes
            .write()
            .unwrap()
            .push(refresh);
        let gate = self.mcp_refresh_gate.read().unwrap().clone();
        if refresh && let Some(gate) = gate {
            gate.notified().await;
        }
        self.mcp_observations
            .read()
            .unwrap()
            .iter()
            .find(|(current, _)| current == source)
            .map(|(_, result)| result.clone())
            .unwrap_or_else(|| {
                Err(RegistryServiceError::internal_client_error(format!(
                    "no test MCP observation configured for {source:?}"
                )))
            })
    }

    async fn get_tool_activation(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
        tool_name: &ToolName,
    ) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
        self.tool_activation_lookups.write().unwrap().push((
            environment_id,
            component_id,
            component_revision,
        ));
        let deployments = self.tool_deployments.read().unwrap();
        let deployment = deployments
            .get(&(environment_id, component_id, component_revision))
            .map(|deployment| &deployment.state);
        get_tool_activation_from_deployment(deployment, owner, tool_name)
    }

    async fn get_accessible_tools(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
    ) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        self.accessible_tools_calls.fetch_add(1, Ordering::SeqCst);
        let snapshot = self
            .tool_deployments
            .read()
            .unwrap()
            .get(&(environment_id, component_id, component_revision))
            .map(|deployment| deployment.discovery.clone());
        get_accessible_tools_from_snapshot(snapshot.as_deref(), owner)
    }

    async fn get_accessible_tool(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
        tool_name: &ToolName,
    ) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        self.accessible_tool_calls.fetch_add(1, Ordering::SeqCst);
        let snapshot = self
            .tool_deployments
            .read()
            .unwrap()
            .get(&(environment_id, component_id, component_revision))
            .map(|deployment| deployment.discovery.clone());
        get_accessible_tool_from_snapshot(snapshot.as_deref(), owner, tool_name)
    }
}
