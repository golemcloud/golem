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
use crate::services::environment_state::EnvironmentStateService;
use golem_common::model::agent::RegistryInvalidationEvent;
use golem_service_base::clients::registry::RegistryService;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub async fn run_registry_event_subscriber(
    registry_service: Arc<dyn RegistryService>,
    environment_state_service: Arc<dyn EnvironmentStateService>,
    agent_types_service: Arc<dyn AgentTypesService>,
    shutdown_token: CancellationToken,
) {
    use futures::StreamExt;

    let mut last_seen_event_id: Option<u64> = None;
    let mut backoff = Duration::from_millis(100);
    let max_backoff = Duration::from_secs(30);

    loop {
        let connect_result = tokio::select! {
            result = registry_service.subscribe_registry_invalidations(last_seen_event_id) => result,
            _ = shutdown_token.cancelled() => {
                info!("Registry event subscriber shutting down");
                return;
            }
        };

        match connect_result {
            Ok(mut stream) => {
                info!("Connected to registry invalidation stream");
                backoff = Duration::from_millis(100);

                loop {
                    let item = tokio::select! {
                        item = stream.next() => item,
                        _ = shutdown_token.cancelled() => {
                            info!("Registry event subscriber shutting down");
                            return;
                        }
                    };

                    match item {
                        Some(Ok(event)) => {
                            last_seen_event_id = Some(event.event_id());
                            dispatch_event(
                                &event,
                                &*environment_state_service,
                                &*agent_types_service,
                            )
                            .await;
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

        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown_token.cancelled() => {
                info!("Registry event subscriber shutting down");
                return;
            }
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn dispatch_event(
    event: &RegistryInvalidationEvent,
    environment_state_service: &dyn EnvironmentStateService,
    agent_types_service: &dyn AgentTypesService,
) {
    match event {
        RegistryInvalidationEvent::CursorExpired { .. } => {
            warn!("Registry invalidation cursor expired, flushing all caches");
            environment_state_service.invalidate_all().await;
            agent_types_service.invalidate_all().await;
        }
        RegistryInvalidationEvent::DeploymentChanged { environment_id, .. } => {
            debug!(
                environment_id = %environment_id,
                "Received deployment changed event, invalidating environment caches"
            );
            environment_state_service
                .invalidate_environment(*environment_id)
                .await;
            agent_types_service
                .invalidate_environment(*environment_id)
                .await;
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
    }
}
