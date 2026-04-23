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

use crate::services::agent_types::AgentTypesService;
use crate::services::component::ComponentService;
use crate::services::environment_state::EnvironmentStateService;
use golem_common::model::agent::RegistryInvalidationEvent;
use golem_service_base::clients::registry::{RegistryInvalidationHandler, RegistryService};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub(crate) struct WorkerExecutorRegistryInvalidationHandler {
    component_service: Arc<dyn ComponentService>,
    environment_state_service: Arc<dyn EnvironmentStateService>,
    agent_types_service: Arc<dyn AgentTypesService>,
}

impl WorkerExecutorRegistryInvalidationHandler {
    pub async fn run(
        registry_service: Arc<dyn RegistryService>,
        component_service: Arc<dyn ComponentService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_types_service: Arc<dyn AgentTypesService>,
        shutdown_token: CancellationToken,
    ) {
        registry_service
            .run_registry_invalidation_event_subscriber(
                "worker-executor",
                Some(shutdown_token),
                Arc::new(Self {
                    component_service,
                    environment_state_service,
                    agent_types_service,
                }),
            )
            .await;
    }
}

#[async_trait::async_trait]
impl RegistryInvalidationHandler for WorkerExecutorRegistryInvalidationHandler {
    async fn on_event(&self, event: RegistryInvalidationEvent) {
        match &event {
            RegistryInvalidationEvent::CursorExpired { .. } => {
                warn!("Registry invalidation cursor expired, flushing all caches");
                self.component_service.invalidate_all().await;
                self.environment_state_service.invalidate_all().await;
                self.agent_types_service.invalidate_all().await;
            }
            RegistryInvalidationEvent::DeploymentChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received deployment changed event, invalidating environment caches"
                );
                self.component_service
                    .invalidate_latest_deployed_metadata_for_environment(*environment_id)
                    .await;
                self.environment_state_service
                    .invalidate_environment(*environment_id)
                    .await;
                self.agent_types_service
                    .invalidate_environment(*environment_id)
                    .await;
            }
            RegistryInvalidationEvent::DomainRegistrationChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received domain registration changed event, ignoring"
                );
            }
            RegistryInvalidationEvent::AccountTokensInvalidated { account_id, .. } => {
                debug!(
                    account_id = %account_id,
                    "Received account tokens invalidated event, ignoring"
                );
            }
            RegistryInvalidationEvent::EnvironmentPermissionsChanged {
                environment_id,
                grantee_account_id,
                ..
            } => {
                debug!(
                    environment_id = %environment_id,
                    grantee_account_id = %grantee_account_id,
                    "Received environment permissions changed event, ignoring"
                );
            }
            RegistryInvalidationEvent::SecuritySchemeChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received security scheme changed event, ignoring"
                );
            }
            RegistryInvalidationEvent::RetryPolicyChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received retry policy changed event, invalidating environment cache"
                );
                self.environment_state_service
                    .invalidate_environment(*environment_id)
                    .await;
            }
            RegistryInvalidationEvent::ResourceDefinitionChanged {
                environment_id,
                resource_definition_id,
                resource_name,
                ..
            } => {
                debug!(
                    environment_id = %environment_id,
                    resource_definition_id = %resource_definition_id,
                    resource_name = %resource_name,
                    "Received resource definition changed event, ignoring"
                );
            }
            RegistryInvalidationEvent::AgentSecretChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received agent secret changed event, invalidating environment cache"
                );
                self.environment_state_service
                    .invalidate_environment(*environment_id)
                    .await;
            }
            RegistryInvalidationEvent::ApplicationDeleted {
                application_id,
                account_id,
                ..
            } => {
                debug!(
                    application_id = %application_id,
                    account_id = %account_id,
                    "Received application deleted event, flushing all caches"
                );
                // Worker-executor caches are keyed per-environment/component/agent-type,
                // none of which carry the application_id. Flush all to guarantee no
                // cached entries for environments under the deleted application
                // survive into a same-name recreation cycle.
                self.component_service.invalidate_all().await;
                self.environment_state_service.invalidate_all().await;
                self.agent_types_service.invalidate_all().await;
            }
            RegistryInvalidationEvent::EnvironmentDeleted { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received environment deleted event, invalidating environment caches"
                );
                self.component_service
                    .invalidate_latest_deployed_metadata_for_environment(*environment_id)
                    .await;
                self.environment_state_service
                    .invalidate_environment(*environment_id)
                    .await;
                self.agent_types_service
                    .invalidate_environment(*environment_id)
                    .await;
            }
        }
    }
}
