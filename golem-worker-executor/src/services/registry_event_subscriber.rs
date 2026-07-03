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

use crate::services::active_workers::ActiveWorkers;
use crate::services::agent_types::AgentTypesService;
use crate::services::card::{CardService, CardState};
use crate::services::component::ComponentService;
use crate::services::environment_state::EnvironmentStateService;
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::RegistryInvalidationEvent;
use golem_common::model::card::CardId;
use golem_service_base::clients::registry::{RegistryInvalidationHandler, RegistryService};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub(crate) struct WorkerExecutorRegistryInvalidationHandler<Ctx: WorkerCtx> {
    active_workers: Arc<ActiveWorkers<Ctx>>,
    card_service: Arc<dyn CardService>,
    component_service: Arc<dyn ComponentService>,
    environment_state_service: Arc<dyn EnvironmentStateService>,
    agent_types_service: Arc<dyn AgentTypesService>,
}

impl<Ctx: WorkerCtx> WorkerExecutorRegistryInvalidationHandler<Ctx> {
    pub async fn run(
        registry_service: Arc<dyn RegistryService>,
        active_workers: Arc<ActiveWorkers<Ctx>>,
        card_service: Arc<dyn CardService>,
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
                    active_workers,
                    card_service,
                    component_service,
                    environment_state_service,
                    agent_types_service,
                }),
            )
            .await;
    }

    /// Re-validates every card currently depended on by a running worker and
    /// propagates any revocations discovered. A `CursorExpired` event means card
    /// revocations may have been missed, so the flushed card cache is not enough
    /// on its own: already-running workers cached their permission as live and
    /// would only re-check on their next replay. This re-fetches the tracked
    /// cards (the card cache was just flushed, so `check_cards` hits the
    /// registry) and reuses the standard revocation propagation path for any
    /// card that is no longer live.
    async fn reevaluate_tracked_cards(&self) {
        let card_ids = self.active_workers.tracked_card_ids().await;
        if card_ids.is_empty() {
            return;
        }

        let states = match self.card_service.check_cards(card_ids).await {
            Ok(states) => states,
            Err(err) => {
                warn!(
                    error = %err,
                    "Failed re-validating tracked cards after cursor expiry; \
                     running workers will re-check on their next replay"
                );
                return;
            }
        };

        let revoked = states
            .into_iter()
            .filter(|(_, state)| *state == CardState::Revoked)
            .map(|(card_id, _)| card_id)
            .collect::<Vec<_>>();

        if !revoked.is_empty() {
            debug!(
                card_count = revoked.len(),
                "Cursor expiry re-validation found revoked cards, notifying running workers"
            );
            self.active_workers.notify_revoked_cards(&revoked).await;
        }
    }
}

#[async_trait::async_trait]
impl<Ctx: WorkerCtx> RegistryInvalidationHandler
    for WorkerExecutorRegistryInvalidationHandler<Ctx>
{
    async fn on_event(&self, event: RegistryInvalidationEvent) {
        match &event {
            RegistryInvalidationEvent::CursorExpired { .. } => {
                warn!("Registry invalidation cursor expired, flushing all caches");
                self.component_service.invalidate_all().await;
                self.environment_state_service.invalidate_all().await;
                self.agent_types_service.invalidate_all().await;
                self.card_service.invalidate_all().await;
                self.reevaluate_tracked_cards().await;
            }
            RegistryInvalidationEvent::DeploymentChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received deployment changed event, invalidating environment caches"
                );
                self.component_service
                    .invalidate_current_deployed_metadata_for_environment(*environment_id)
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
            RegistryInvalidationEvent::CardRevoked { card_ids, .. } => {
                let card_ids = card_ids.iter().copied().map(CardId).collect::<Vec<_>>();
                debug!(
                    card_count = card_ids.len(),
                    "Received card revocation event, recording revoked card ids"
                );
                self.card_service.record_revoked_cards(&card_ids).await;
                self.active_workers.notify_revoked_cards(&card_ids).await;
            }
            RegistryInvalidationEvent::ApplicationDeleted {
                application_id,
                account_id,
                app_name,
                environment_ids,
                ..
            } => {
                debug!(
                    application_id = %application_id,
                    account_id = %account_id,
                    app_name,
                    environment_count = environment_ids.len(),
                    "Received application deleted event, invalidating per-environment caches"
                );
                // Invalidate each environment individually using the provided UUIDs
                // rather than flushing all caches.
                for env_id in environment_ids {
                    self.active_workers.unload_environment(*env_id).await;
                    self.component_service
                        .invalidate_all_metadata_for_environment(*env_id)
                        .await;
                    self.environment_state_service
                        .invalidate_environment(*env_id)
                        .await;
                    self.agent_types_service
                        .invalidate_environment(*env_id)
                        .await;
                }
            }
            RegistryInvalidationEvent::EnvironmentDeleted {
                environment_id,
                app_name,
                env_name,
                ..
            } => {
                debug!(
                    environment_id = %environment_id,
                    app_name,
                    env_name,
                    "Received environment deleted event, invalidating environment caches"
                );
                self.active_workers
                    .unload_environment(*environment_id)
                    .await;
                self.component_service
                    .invalidate_all_metadata_for_environment(*environment_id)
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
