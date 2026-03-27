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
use golem_service_base::clients::registry::RegistryService;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{debug, warn};

pub fn start(
    registry_service: Arc<dyn RegistryService>,
    fetcher: Arc<dyn ResourceDefinitionFetcher>,
    quota_service: Arc<QuotaService>,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) {
    join_set.spawn(run_loop(registry_service, fetcher, quota_service));
}

async fn run_loop(
    registry_service: Arc<dyn RegistryService>,
    fetcher: Arc<dyn ResourceDefinitionFetcher>,
    quota_service: Arc<QuotaService>,
) -> anyhow::Result<()> {
    registry_service
        .run_registry_invalidation_event_subscriber(
            "shard-manager",
            None,
            Box::new(move |event| {
                let fetcher = fetcher.clone();
                let quota_service = quota_service.clone();
                Box::pin(async move {
                    dispatch_event(&fetcher, &quota_service, &event).await;
                })
            }),
        )
        .await;

    Ok(())
}

async fn dispatch_event(
    fetcher: &Arc<dyn ResourceDefinitionFetcher>,
    quota_service: &QuotaService,
    event: &RegistryInvalidationEvent,
) {
    match event {
        RegistryInvalidationEvent::CursorExpired { .. } => {
            warn!("Registry invalidation cursor expired, refreshing all entries");
            fetcher.invalidate_all().await;
            quota_service.on_cursor_expired().await;
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
            fetcher
                .invalidate(*environment_id, resource_name.clone())
                .await;
            quota_service
                .on_resource_definition_changed(*resource_definition_id)
                .await;
        }
        RegistryInvalidationEvent::DeploymentChanged { .. }
        | RegistryInvalidationEvent::DomainRegistrationChanged { .. }
        | RegistryInvalidationEvent::AccountTokensInvalidated { .. }
        | RegistryInvalidationEvent::EnvironmentPermissionsChanged { .. }
        | RegistryInvalidationEvent::SecuritySchemeChanged { .. } => {}
    }
}
