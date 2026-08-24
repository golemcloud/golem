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
use golem_common::model::environment::EnvironmentId;
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, ToolDeploymentState, ToolName,
};
use golem_common::schema::tool::DiscoveredTool;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_worker_executor::services::environment_state::{
    EnvironmentStateService, ToolDiscoveryError, ToolDiscoverySnapshot,
    get_accessible_tool_from_snapshot, get_accessible_tools_from_snapshot,
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
}

#[async_trait]
impl EnvironmentStateService for TestEnvironmentStateService {
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

    async fn get_registered_tool(
        &self,
        environment_id: EnvironmentId,
        tool_name: &ToolName,
    ) -> Result<Option<RegisteredTool>, WorkerExecutorError> {
        Ok(self
            .tool_deployments
            .read()
            .unwrap()
            .iter()
            .find(|((candidate, _, _), _)| *candidate == environment_id)
            .and_then(|(_, deployment)| deployment.state.registered_tools.get(tool_name))
            .cloned())
    }

    async fn get_agent_tool_binding(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
    ) -> Result<Option<CompiledToolBinding>, WorkerExecutorError> {
        Ok(self
            .tool_deployments
            .read()
            .unwrap()
            .iter()
            .find(|((candidate, _, _), _)| *candidate == environment_id)
            .and_then(|(_, deployment)| deployment.state.agent_tool_bindings.get(agent_type))
            .and_then(|bindings| bindings.get(tool_name))
            .cloned())
    }

    async fn get_accessible_tools(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type: &AgentTypeName,
    ) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        self.accessible_tools_calls.fetch_add(1, Ordering::SeqCst);
        let snapshot = self
            .tool_deployments
            .read()
            .unwrap()
            .get(&(environment_id, component_id, component_revision))
            .map(|deployment| deployment.discovery.clone());
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
        self.accessible_tool_calls.fetch_add(1, Ordering::SeqCst);
        let snapshot = self
            .tool_deployments
            .read()
            .unwrap()
            .get(&(environment_id, component_id, component_revision))
            .map(|deployment| deployment.discovery.clone());
        get_accessible_tool_from_snapshot(snapshot.as_deref(), agent_type, tool_name)
    }
}
