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
use golem_common::SafeDisplay;
use golem_common::model::card::{CardId, StoredCard};
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait CardService: Send + Sync {
    async fn record_revoked_cards(&self, card_ids: &[CardId]);

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError>;
}

pub struct CardServiceDefault {
    registry_service: Arc<dyn RegistryService>,
    state: RwLock<CardServiceState>,
}

#[derive(Default)]
struct CardServiceState {
    cards: HashMap<CardId, CardData>,
}

#[derive(Default)]
struct CardData {
    state: CardState,
}

#[derive(Default, Clone, PartialEq, Eq, Debug)]
pub enum CardState {
    #[default]
    Unknown,
    Live(Box<StoredCard>),
    Revoked,
}

pub struct NoopCardService;

#[async_trait]
impl CardService for NoopCardService {
    async fn record_revoked_cards(&self, _card_ids: &[CardId]) {}

    async fn check_cards(
        &self,
        _card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        Ok(HashMap::new())
    }
}

impl CardServiceDefault {
    pub fn new(registry_service: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service,
            state: RwLock::new(CardServiceState::default()),
        }
    }

    fn record_revoked_cards_in_state(state: &mut CardServiceState, card_ids: &[CardId]) {
        for card_id in card_ids {
            state.cards.entry(*card_id).or_default().state = CardState::Revoked;
        }
    }

    fn record_live_cards_in_state(state: &mut CardServiceState, cards: Vec<StoredCard>) {
        for card in cards {
            let data = state.cards.entry(card.card_id()).or_default();
            if data.state != CardState::Revoked {
                data.state = CardState::Live(Box::new(card));
            }
        }
    }

    fn cached_card_states(
        state: &CardServiceState,
        card_ids: &[CardId],
    ) -> (HashMap<CardId, CardState>, Vec<CardId>) {
        let mut cached_states = HashMap::new();
        let mut seen_unknown = HashSet::new();

        for card_id in card_ids {
            match state.cards.get(card_id).map(|data| &data.state) {
                Some(CardState::Revoked) => {
                    cached_states.insert(*card_id, CardState::Revoked);
                }
                Some(CardState::Live(card)) => {
                    cached_states.insert(*card_id, CardState::Live(card.clone()));
                }
                Some(CardState::Unknown) | None => {
                    if seen_unknown.insert(*card_id) {
                        cached_states.insert(*card_id, CardState::Unknown);
                    }
                }
            }
        }

        let unknown = seen_unknown.into_iter().collect();
        (cached_states, unknown)
    }

    fn record_fetched_cards(
        state: &mut CardServiceState,
        requested_card_ids: &[CardId],
        fetched_cards: Vec<StoredCard>,
    ) -> Vec<CardId> {
        let fetched_card_ids = fetched_cards
            .iter()
            .map(StoredCard::card_id)
            .collect::<HashSet<_>>();
        let missing = requested_card_ids
            .iter()
            .copied()
            .filter(|card_id| !fetched_card_ids.contains(card_id))
            .collect::<Vec<_>>();

        Self::record_revoked_cards_in_state(state, &missing);
        Self::record_live_cards_in_state(state, fetched_cards);

        missing
    }

    fn card_state(state: &CardServiceState, card_id: CardId) -> Option<CardState> {
        state.cards.get(&card_id).map(|data| data.state.clone())
    }
}

#[async_trait]
impl CardService for CardServiceDefault {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        let mut state = self.state.write().await;
        Self::record_revoked_cards_in_state(&mut state, card_ids);
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        let (mut result, unknown) = {
            let state = self.state.read().await;
            Self::cached_card_states(&state, &card_ids)
        };

        if unknown.is_empty() {
            return Ok(result);
        }

        let live_cards = self
            .registry_service
            .batch_get_cards(unknown.clone())
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "Failed checking card liveness: {}",
                    err.to_safe_string()
                ))
            })?;
        let mut state = self.state.write().await;
        Self::record_fetched_cards(&mut state, &unknown, live_cards);
        for card_id in unknown {
            if let Some(state) = Self::card_state(&state, card_id) {
                result.insert(card_id, state);
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use golem_common::model::AgentId;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentType, ResolvedAgentType};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::auth::TokenSecret;
    use golem_common::model::card::Card;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
    use golem_service_base::clients::registry::{
        RegistryInvalidationHandler, RegistryServiceError, ResourceUsageUpdate,
    };
    use golem_service_base::custom_api::CompiledRoutes;
    use golem_service_base::mcp::CompiledMcp;
    use golem_service_base::model::auth::AuthCtx;
    use golem_service_base::model::component::Component;
    use golem_service_base::model::environment::EnvironmentState;
    use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    struct TestRegistryService {
        existing_cards: Option<HashSet<CardId>>,
        lookup_count: Arc<AtomicUsize>,
    }

    impl TestRegistryService {
        fn all_existing() -> Self {
            Self {
                existing_cards: None,
                lookup_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_existing(existing_cards: HashSet<CardId>, lookup_count: Arc<AtomicUsize>) -> Self {
            Self {
                existing_cards: Some(existing_cards),
                lookup_count,
            }
        }
    }

    #[async_trait]
    impl RegistryService for TestRegistryService {
        async fn authenticate_token(
            &self,
            _token: &TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_limits(
            &self,
            _account_id: golem_common::model::account::AccountId,
        ) -> Result<ResourceLimits, RegistryServiceError> {
            unimplemented!()
        }

        async fn update_worker_connection_limit(
            &self,
            _account_id: golem_common::model::account::AccountId,
            _agent_id: &AgentId,
            _added: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_update_resource_usage(
            &self,
            _updates: HashMap<golem_common::model::account::AccountId, ResourceUsageUpdate>,
        ) -> Result<AccountResourceLimits, RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_get_existing_cards(
            &self,
            card_ids: Vec<CardId>,
        ) -> Result<Vec<CardId>, RegistryServiceError> {
            self.lookup_count.fetch_add(1, Ordering::SeqCst);
            Ok(match &self.existing_cards {
                Some(existing_cards) => card_ids
                    .into_iter()
                    .filter(|card_id| existing_cards.contains(card_id))
                    .collect(),
                None => card_ids,
            })
        }

        async fn batch_get_cards(
            &self,
            card_ids: Vec<CardId>,
        ) -> Result<Vec<StoredCard>, RegistryServiceError> {
            self.lookup_count.fetch_add(1, Ordering::SeqCst);
            Ok(match &self.existing_cards {
                Some(existing_cards) => card_ids
                    .into_iter()
                    .filter(|card_id| existing_cards.contains(card_id))
                    .map(stored_card)
                    .collect(),
                None => card_ids.into_iter().map(stored_card).collect(),
            })
        }

        async fn download_component(
            &self,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<u8>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_component_metadata(
            &self,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_deployed_component_metadata(
            &self,
            _component_id: ComponentId,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_deployed_component_revisions(
            &self,
            _component_id: ComponentId,
        ) -> Result<Vec<Component>, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_component(
            &self,
            _resolving_account_id: golem_common::model::account::AccountId,
            _resolving_application_id: ApplicationId,
            _resolving_environment_id: EnvironmentId,
            _component_slug: &str,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_agent_types(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_agent_type(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
            _name: &AgentTypeName,
        ) -> Result<RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_agent_type_by_names(
            &self,
            _app_name: &ApplicationName,
            _environment_name: &EnvironmentName,
            _agent_type_name: &AgentTypeName,
            _deployment_revision: Option<DeploymentRevision>,
            _owner_account_email: Option<&str>,
            _auth_ctx: &AuthCtx,
        ) -> Result<ResolvedAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_routes_for_domain(
            &self,
            _domain: &Domain,
        ) -> Result<CompiledRoutes, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_compiled_mcps_for_domain(
            &self,
            _domain: &Domain,
        ) -> Result<CompiledMcp, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_current_environment_state(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<EnvironmentState, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_definition_by_id(
            &self,
            _resource_definition_id: ResourceDefinitionId,
        ) -> Result<ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_definition_by_name(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: ResourceName,
        ) -> Result<ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn subscribe_registry_invalidations(
            &self,
            _last_seen_event_id: Option<u64>,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                golem_common::model::agent::RegistryInvalidationEvent,
                                RegistryServiceError,
                            >,
                        > + Send,
                >,
            >,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn run_registry_invalidation_event_subscriber(
            &self,
            _service_name: &'static str,
            _shutdown_token: Option<tokio_util::sync::CancellationToken>,
            _handler: Arc<dyn RegistryInvalidationHandler>,
        ) {
            unimplemented!()
        }
    }

    fn service() -> CardServiceDefault {
        CardServiceDefault::new(Arc::new(TestRegistryService::all_existing()))
    }

    fn stored_card(card_id: CardId) -> StoredCard {
        StoredCard::Concrete(Card {
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
        })
    }

    fn assert_live(states: &HashMap<CardId, CardState>, card_id: CardId) {
        assert!(
            matches!(states.get(&card_id), Some(CardState::Live(card)) if card.card_id() == card_id)
        );
    }

    fn assert_revoked(states: &HashMap<CardId, CardState>, card_id: CardId) {
        assert_eq!(states.get(&card_id), Some(&CardState::Revoked));
    }

    #[test]
    async fn noop_card_service_reports_no_revoked_cards() {
        let service = NoopCardService;
        let revoked = CardId::new();

        assert!(!matches!(
            service
                .check_cards(vec![revoked])
                .await
                .unwrap()
                .get(&revoked),
            Some(CardState::Revoked)
        ));
    }

    #[test]
    async fn record_revoked_cards_caches_revocation() {
        let service = service();
        let card_id = CardId::new();

        service.record_revoked_cards(&[card_id]).await;

        assert_revoked(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
    }

    #[test]
    async fn known_live_card_skips_repeated_registry_lookup() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_existing(
            HashSet::from([card_id]),
            lookup_count.clone(),
        )));

        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);

        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn checked_card_is_cached_as_known_live_without_interested_agents() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_existing(
            HashSet::from([card_id]),
            lookup_count.clone(),
        )));

        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);

        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn check_cards_returns_live_card_data() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_existing(
            HashSet::from([card_id]),
            lookup_count.clone(),
        )));

        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn check_cards_reuses_cached_live_card_data() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_existing(
            HashSet::from([card_id]),
            lookup_count.clone(),
        )));

        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);

        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn revocation_without_interested_agents_invalidates_known_live_card() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_existing(
            HashSet::from([card_id]),
            lookup_count.clone(),
        )));

        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        service.record_revoked_cards(&[card_id]).await;

        assert_revoked(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn revocation_invalidates_known_live_card() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_existing(
            HashSet::from([card_id]),
            lookup_count.clone(),
        )));

        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        service.record_revoked_cards(&[card_id]).await;

        assert_revoked(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);
    }
}
