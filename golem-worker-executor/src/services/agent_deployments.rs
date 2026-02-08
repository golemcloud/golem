// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use golem_common::model::environment::EnvironmentId;
use golem_common::model::agent::AgentTypeName;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_common::model::domain_registration::Domain;
use std::sync::Arc;
use golem_service_base::clients::registry::RegistryService;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use std::collections::HashMap;
use std::time::Duration;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::custom_api::AgentWebhookId;

#[async_trait]
pub trait AgentDeploymentsService: Send + Sync {
    /// Get the current deployment of the agent.
    /// Will return None if there is no current deployment.
    async fn get_agent_deployment(&self, environment: EnvironmentId, agent_type: &AgentTypeName) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError>;

    async fn get_agent_webhook_url(
        &self,
        environment: EnvironmentId,
        agent_type: &AgentTypeName,
        webhook_id: &AgentWebhookId
    ) -> Result<Option<String>, WorkerExecutorError>;
}

pub struct GrpcAgentDeploymentService {
    client: Arc<dyn RegistryService>,
    cached_environment_agent_deployments: Cache<
        EnvironmentId,
        (),
        HashMap<AgentTypeName, AgentDeploymentDetails>,
        WorkerExecutorError,
    >,
    use_https_for_webhook_url: bool,
}

impl GrpcAgentDeploymentService {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        cache_capacity: usize,
        cache_ttl: Duration,
        cache_eviction_interval: Duration,
        use_https_for_webhook_url: bool,
    ) -> Self {
        Self {
            client: registry_service,
            cached_environment_agent_deployments: Cache::new(
                Some(cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: cache_ttl,
                    period: cache_eviction_interval,
                },
                "gprc_agent_deployed_domains_service",
            ),
            use_https_for_webhook_url
        }
    }

    async fn get_environment_agent_deployments(&self, environment: EnvironmentId) -> Result<HashMap<AgentTypeName, AgentDeploymentDetails>, WorkerExecutorError> {
        self
            .cached_environment_agent_deployments
            .get_or_insert_simple(
                &environment,
                || Box::pin(async move {
                    self.client
                        .get_agent_deployments(environment)
                        .await
                        .map_err(|e| WorkerExecutorError::runtime(format!("Failed to get domains for agent types: {e}")))
                })
            )
            .await
    }
}

#[async_trait]
impl AgentDeploymentsService for GrpcAgentDeploymentService {
    async fn get_agent_deployment(&self, environment: EnvironmentId, agent_type: &AgentTypeName) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        let environment_agent_deployments = self.get_environment_agent_deployments(environment).await?;
        Ok(environment_agent_deployments.get(agent_type).cloned())
    }

    async fn get_agent_webhook_url(
        &self,
        environment: EnvironmentId,
        agent_type: &AgentTypeName,
        webhook_id: &AgentWebhookId
    ) -> Result<Option<String>, WorkerExecutorError> {
        let Some(webhook_prefix_authority_and_path) = self.get_agent_deployment(environment, agent_type).await?.and_then(|ad| ad.webhook_prefix_authority_and_path) else {
            return Ok(None);
        };
        let encoded_webhook_id = webhook_id.to_base64_url();
        let protocol = if self.use_https_for_webhook_url { "https" } else { "http" };
        Ok(Some(format!("{}://{}/{}", protocol, webhook_prefix_authority_and_path, encoded_webhook_id)))
    }
}
