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

use crate::quota::{QuotaService, ResourceDefinitionFetcher};
use golem_common::model::agent::RegistryInvalidationEvent;
use golem_service_base::clients::registry::{RegistryInvalidationHandler, RegistryService};
use std::sync::Arc;
use tracing::{debug, warn};

pub(crate) struct ShardManagerRegistryInvalidationHandler {
    fetcher: Arc<dyn ResourceDefinitionFetcher>,
    quota_service: Arc<QuotaService>,
}

impl ShardManagerRegistryInvalidationHandler {
    pub async fn run(
        registry_service: Arc<dyn RegistryService>,
        fetcher: Arc<dyn ResourceDefinitionFetcher>,
        quota_service: Arc<QuotaService>,
    ) {
        registry_service
            .run_registry_invalidation_event_subscriber(
                "shard-manager",
                None,
                Arc::new(Self {
                    fetcher,
                    quota_service,
                }),
            )
            .await
    }
}

#[async_trait::async_trait]
impl RegistryInvalidationHandler for ShardManagerRegistryInvalidationHandler {
    async fn on_event(&self, event: RegistryInvalidationEvent) {
        match &event {
            RegistryInvalidationEvent::CursorExpired { .. } => {
                warn!("Registry invalidation cursor expired, refreshing all entries");
                self.fetcher.invalidate_all().await;
                self.quota_service.on_cursor_expired().await;
            }
            RegistryInvalidationEvent::ResourceDefinitionChanged {
                environment_id,
                resource_definition_id,
                resource_name,
                ..
            } => {
                debug!(
                    %environment_id,
                    %resource_definition_id,
                    %resource_name,
                    "resource definition changed, refreshing cached entry"
                );
                self.quota_service
                    .on_resource_definition_changed(*resource_definition_id)
                    .await;
                self.fetcher
                    .invalidate(*environment_id, resource_name.clone())
                    .await;
            }
            RegistryInvalidationEvent::DeploymentChanged { .. }
            | RegistryInvalidationEvent::DomainRegistrationChanged { .. }
            | RegistryInvalidationEvent::AccountTokensInvalidated { .. }
            | RegistryInvalidationEvent::EnvironmentPermissionsChanged { .. }
            | RegistryInvalidationEvent::SecuritySchemeChanged { .. }
            | RegistryInvalidationEvent::RetryPolicyChanged { .. }
            | RegistryInvalidationEvent::AgentSecretChanged { .. }
            | RegistryInvalidationEvent::ApplicationDeleted { .. }
            | RegistryInvalidationEvent::EnvironmentDeleted { .. } => {}
        }
    }
}
