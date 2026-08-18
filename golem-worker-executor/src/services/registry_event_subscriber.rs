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
use crate::services::card_interest::{CardAuthorityRecoveryEpoch, CardAuthorityRecoveryFinalize};
use crate::services::component::ComponentService;
use crate::services::environment_state::EnvironmentStateService;
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::RegistryInvalidationEvent;
use golem_common::model::card::CardId;
use golem_service_base::clients::registry::{RegistryInvalidationHandler, RegistryService};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub(crate) struct WorkerExecutorRegistryInvalidationHandler<Ctx: WorkerCtx> {
    active_workers: Arc<ActiveWorkers<Ctx>>,
    card_service: Arc<dyn CardService>,
    component_service: Arc<dyn ComponentService>,
    environment_state_service: Arc<dyn EnvironmentStateService>,
    agent_types_service: Arc<dyn AgentTypesService>,
    shutdown_token: CancellationToken,
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
        let handler_shutdown_token = shutdown_token.clone();
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
                    shutdown_token: handler_shutdown_token,
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
    async fn reevaluate_tracked_cards(&self, epoch: CardAuthorityRecoveryEpoch) {
        let mut retry_delay = Duration::from_millis(100);
        loop {
            if !self
                .active_workers
                .is_current_card_authority_recovery(epoch)
            {
                return;
            }

            let (interest_revision, card_ids) =
                self.active_workers.tracked_card_ids_with_revision().await;
            let revoked = if card_ids.is_empty() {
                Vec::new()
            } else {
                let check_result = tokio::select! {
                    _ = self.shutdown_token.cancelled() => return,
                    result = self.card_service.check_cards(card_ids.clone()) => result,
                };
                let states = match check_result {
                    Ok(states) => states,
                    Err(err) => {
                        warn!(
                            error = %err,
                            retry_delay_ms = retry_delay.as_millis(),
                            "Failed re-validating tracked cards after cursor expiry; retrying"
                        );
                        if !sleep_for_recovery_retry(&self.shutdown_token, retry_delay).await {
                            return;
                        }
                        retry_delay = next_retry_delay(retry_delay);
                        continue;
                    }
                };
                let Some(revoked) = revoked_cards_if_fully_revalidated(&card_ids, &states) else {
                    warn!(
                        retry_delay_ms = retry_delay.as_millis(),
                        "Registry omitted or could not verify tracked cards after cursor expiry; retrying"
                    );
                    if !sleep_for_recovery_retry(&self.shutdown_token, retry_delay).await {
                        return;
                    }
                    retry_delay = next_retry_delay(retry_delay);
                    continue;
                };
                revoked
            };

            if !revoked.is_empty() {
                debug!(
                    card_count = revoked.len(),
                    "Cursor expiry re-validation found revoked cards, notifying running workers"
                );
                self.active_workers.notify_revoked_cards(&revoked).await;
            }

            match self
                .active_workers
                .finalize_card_authority_recovery(epoch, interest_revision)
                .await
            {
                CardAuthorityRecoveryFinalize::Reopened => return,
                CardAuthorityRecoveryFinalize::InterestChanged => {
                    debug!(
                        "Card interests changed during cursor-expiry recovery; re-validating the new tracked set"
                    );
                    retry_delay = Duration::from_millis(100);
                }
                CardAuthorityRecoveryFinalize::StaleEpoch => return,
            }
        }
    }
}

fn revoked_cards_if_fully_revalidated(
    card_ids: &[CardId],
    states: &HashMap<CardId, CardState>,
) -> Option<Vec<CardId>> {
    let mut revoked = Vec::new();
    for card_id in card_ids {
        match states.get(card_id) {
            Some(CardState::Live(card)) if card.card_id() == *card_id => {}
            Some(CardState::Live(_)) => return None,
            Some(CardState::Revoked) => revoked.push(*card_id),
            Some(CardState::Unknown) | None => return None,
        }
    }
    Some(revoked)
}

async fn sleep_for_recovery_retry(shutdown_token: &CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        _ = shutdown_token.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

fn next_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(Duration::from_secs(30))
}

#[async_trait::async_trait]
impl<Ctx: WorkerCtx> RegistryInvalidationHandler
    for WorkerExecutorRegistryInvalidationHandler<Ctx>
{
    async fn on_event(&self, event: RegistryInvalidationEvent) {
        match &event {
            RegistryInvalidationEvent::CursorExpired { .. } => {
                warn!("Registry invalidation cursor expired, flushing all caches");
                let recovery_epoch = self.active_workers.close_card_authority();
                self.component_service.invalidate_all().await;
                self.environment_state_service.invalidate_all().await;
                self.agent_types_service.invalidate_all().await;
                self.card_service.invalidate_all().await;
                self.reevaluate_tracked_cards(recovery_epoch).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use golem_common::model::card::{Card, StoredCard};
    use std::collections::HashMap;
    use test_r::test;

    fn live_card(card_id: CardId) -> CardState {
        CardState::Live(Box::new(StoredCard::Concrete(Card {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: Utc::now(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        })))
    }

    #[test]
    fn cursor_recovery_backoff_caps_at_thirty_seconds() {
        let mut delay = Duration::from_millis(100);
        for _ in 0..20 {
            delay = next_retry_delay(delay);
        }
        assert_eq!(delay, Duration::from_secs(30));
        assert_eq!(next_retry_delay(delay), Duration::from_secs(30));
    }

    #[test]
    fn recovery_requires_a_state_for_every_tracked_card() {
        let live = CardId::new();
        let missing = CardId::new();
        let states = HashMap::from([(live, live_card(live))]);

        assert_eq!(
            revoked_cards_if_fully_revalidated(&[live, missing], &states),
            None
        );
    }

    #[test]
    fn recovery_rejects_unknown_card_states() {
        let card_id = CardId::new();
        let states = HashMap::from([(card_id, CardState::Unknown)]);

        assert_eq!(
            revoked_cards_if_fully_revalidated(&[card_id], &states),
            None
        );
    }

    #[test]
    fn recovery_rejects_live_state_with_mismatched_card_id() {
        let requested = CardId::new();
        let returned = CardId::new();
        let states = HashMap::from([(requested, live_card(returned))]);

        assert_eq!(
            revoked_cards_if_fully_revalidated(&[requested], &states),
            None
        );
    }

    #[test]
    fn recovery_collects_revocations_after_full_revalidation() {
        let revoked = CardId::new();
        let live = CardId::new();
        let states = HashMap::from([(revoked, CardState::Revoked), (live, live_card(live))]);

        assert_eq!(
            revoked_cards_if_fully_revalidated(&[revoked, live], &states),
            Some(vec![revoked])
        );
    }
}
