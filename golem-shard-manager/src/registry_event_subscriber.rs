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
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

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
    use futures::StreamExt;

    let mut last_seen_event_id: Option<u64> = None;
    let mut backoff = Duration::from_millis(100);
    let max_backoff = Duration::from_secs(30);

    loop {
        let connect_result = registry_service
            .subscribe_registry_invalidations(last_seen_event_id)
            .await;

        match connect_result {
            Ok(mut stream) => {
                info!("Connected to registry invalidation stream");
                backoff = Duration::from_millis(100);

                loop {
                    match stream.next().await {
                        Some(Ok(event)) => {
                            last_seen_event_id = Some(event.event_id());
                            dispatch_event(&fetcher, &quota_service, &event).await;
                        }
                        Some(Err(e)) => {
                            warn!("Error receiving registry event: {e}, reconnecting");
                            break;
                        }
                        None => {
                            warn!("Registry invalidation stream ended, reconnecting");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to registry invalidation stream: {e}");
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
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
