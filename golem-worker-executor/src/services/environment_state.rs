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
use golem_common::model::environment::EnvironmentId;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::environment::EnvironmentState;
use golem_service_base::model::AgentDeploymentDetails;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
    ) -> Result<HashMap<Vec<String>, AgentSecret>, WorkerExecutorError>;
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
    ) -> Result<HashMap<Vec<String>, AgentSecret>, WorkerExecutorError> {
        let environment_state = self.get_environment_state(environment_id).await?;
        Ok(environment_state.agent_secrets.clone())
    }
}
