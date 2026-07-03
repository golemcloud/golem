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

    /// Drops cached live card state so the next `check_cards` re-validates
    /// against the registry. Called during registry invalidation cursor-expiry
    /// recovery, when card revocations may have been missed.
    async fn invalidate_all(&self) {}
}

pub struct CardServiceDefault {
    registry_service: Arc<dyn RegistryService>,
    state: RwLock<CardServiceState>,
}

#[derive(Default)]
struct CardServiceState {
    cards: HashMap<CardId, CardData>,
    /// Bumped by `invalidate_all`. A `check_cards` lookup captures this before
    /// its registry fetch and refuses to write results back if it changed in
    /// the meantime, so an in-flight lookup cannot repopulate a just-flushed
    /// live cache with stale data.
    generation: u64,
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
        // Optimistic re-validation: capture the cache generation before fetching
        // from the registry. If an `invalidate_all` races with the fetch, retry
        // with a fresh snapshot so cached hits and freshly fetched cards are all
        // reported from a single consistent post-invalidation cache state. The
        // bound guards against pathological repeated invalidation; on exhaustion
        // the fetch is served without repopulating the cache, so the next call
        // still re-validates.
        const MAX_ATTEMPTS: usize = 8;

        for attempt in 0..MAX_ATTEMPTS {
            let (result, unknown, generation) = {
                let state = self.state.read().await;
                let (result, unknown) = Self::cached_card_states(&state, &card_ids);
                (result, unknown, state.generation)
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
            if state.generation == generation {
                Self::record_fetched_cards(&mut state, &unknown, live_cards);
                let (result, _) = Self::cached_card_states(&state, &card_ids);
                return Ok(result);
            }

            if attempt == MAX_ATTEMPTS - 1 {
                // Exhausted retries under repeated invalidation. Every value
                // fetched this attempt predates the detected invalidation, so it
                // cannot be trusted as fresh liveness. Report only what the
                // current cache knows (post-invalidation live entries and
                // monotonic revocations) plus registry-confirmed removals as
                // revoked. Anything else is omitted and re-validated on the next
                // call, so a card is never reported live on stale evidence.
                let fetched_live_ids = live_cards
                    .iter()
                    .map(StoredCard::card_id)
                    .collect::<HashSet<_>>();
                let mut result = HashMap::new();
                for card_id in &card_ids {
                    if let Some(card_state) = Self::card_state(&state, *card_id) {
                        result.insert(*card_id, card_state);
                    } else if unknown.contains(card_id) && !fetched_live_ids.contains(card_id) {
                        result.insert(*card_id, CardState::Revoked);
                    }
                }
                return Ok(result);
            }
        }

        unreachable!("check_cards returns on the final attempt")
    }

    async fn invalidate_all(&self) {
        let mut state = self.state.write().await;
        state
            .cards
            .retain(|_, data| data.state == CardState::Revoked);
        state.generation = state.generation.wrapping_add(1);
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
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    struct BlockedCardLookup {
        requested: Vec<CardId>,
        release: Arc<tokio::sync::Notify>,
    }

    struct BlockingRegistryService {
        existing_cards: Arc<RwLock<HashSet<CardId>>>,
        lookup_started: tokio::sync::mpsc::UnboundedSender<BlockedCardLookup>,
    }

    enum ExistingCards {
        All,
        Fixed(HashSet<CardId>),
        Shared(Arc<RwLock<HashSet<CardId>>>),
        DelayedFirstShared {
            delayed_call_index: usize,
            existing_cards: Arc<RwLock<HashSet<CardId>>>,
            first_lookup_started: Arc<tokio::sync::Notify>,
            release_first_lookup: Arc<tokio::sync::Notify>,
        },
    }

    struct TestRegistryService {
        existing_cards: ExistingCards,
        lookup_count: Arc<AtomicUsize>,
    }

    impl TestRegistryService {
        fn all_existing() -> Self {
            Self {
                existing_cards: ExistingCards::All,
                lookup_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_existing(existing_cards: HashSet<CardId>, lookup_count: Arc<AtomicUsize>) -> Self {
            Self {
                existing_cards: ExistingCards::Fixed(existing_cards),
                lookup_count,
            }
        }

        fn with_shared_existing(
            existing_cards: Arc<RwLock<HashSet<CardId>>>,
            lookup_count: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                existing_cards: ExistingCards::Shared(existing_cards),
                lookup_count,
            }
        }

        fn with_delayed_first_shared_existing(
            existing_cards: Arc<RwLock<HashSet<CardId>>>,
            lookup_count: Arc<AtomicUsize>,
            first_lookup_started: Arc<tokio::sync::Notify>,
            release_first_lookup: Arc<tokio::sync::Notify>,
        ) -> Self {
            Self::with_delayed_shared_existing(
                existing_cards,
                lookup_count,
                first_lookup_started,
                release_first_lookup,
                0,
            )
        }

        fn with_delayed_shared_existing(
            existing_cards: Arc<RwLock<HashSet<CardId>>>,
            lookup_count: Arc<AtomicUsize>,
            first_lookup_started: Arc<tokio::sync::Notify>,
            release_first_lookup: Arc<tokio::sync::Notify>,
            delayed_call_index: usize,
        ) -> Self {
            Self {
                existing_cards: ExistingCards::DelayedFirstShared {
                    delayed_call_index,
                    existing_cards,
                    first_lookup_started,
                    release_first_lookup,
                },
                lookup_count,
            }
        }

        async fn keep_existing(&self, card_ids: Vec<CardId>) -> Vec<CardId> {
            match &self.existing_cards {
                ExistingCards::All => card_ids,
                ExistingCards::Fixed(existing_cards) => card_ids
                    .into_iter()
                    .filter(|card_id| existing_cards.contains(card_id))
                    .collect(),
                ExistingCards::Shared(existing_cards) => {
                    let existing_cards = existing_cards.read().await;
                    card_ids
                        .into_iter()
                        .filter(|card_id| existing_cards.contains(card_id))
                        .collect()
                }
                ExistingCards::DelayedFirstShared { existing_cards, .. } => {
                    let existing_cards = existing_cards.read().await;
                    card_ids
                        .into_iter()
                        .filter(|card_id| existing_cards.contains(card_id))
                        .collect()
                }
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
            Ok(self.keep_existing(card_ids).await)
        }

        async fn batch_get_cards(
            &self,
            card_ids: Vec<CardId>,
        ) -> Result<Vec<StoredCard>, RegistryServiceError> {
            let call_index = self.lookup_count.fetch_add(1, Ordering::SeqCst);
            if let ExistingCards::DelayedFirstShared {
                delayed_call_index,
                existing_cards,
                first_lookup_started,
                release_first_lookup,
            } = &self.existing_cards
                && call_index == *delayed_call_index
            {
                let existing_cards = existing_cards.read().await;
                let snapshot = card_ids
                    .into_iter()
                    .filter(|card_id| existing_cards.contains(card_id))
                    .collect::<Vec<_>>();
                drop(existing_cards);

                first_lookup_started.notify_one();
                release_first_lookup.notified().await;

                return Ok(snapshot.into_iter().map(stored_card).collect());
            }

            Ok(self
                .keep_existing(card_ids)
                .await
                .into_iter()
                .map(stored_card)
                .collect())
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

        async fn get_agent_secret_revision(
            &self,
            _environment_id: EnvironmentId,
            _agent_secret_id: golem_common::model::agent_secret::AgentSecretId,
            _path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
            _revision: golem_common::model::agent_secret::AgentSecretRevision,
        ) -> Result<
            Option<golem_service_base::model::agent_secret::AgentSecret>,
            RegistryServiceError,
        > {
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

    #[async_trait]
    impl RegistryService for BlockingRegistryService {
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
            _card_ids: Vec<CardId>,
        ) -> Result<Vec<CardId>, RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_get_cards(
            &self,
            card_ids: Vec<CardId>,
        ) -> Result<Vec<StoredCard>, RegistryServiceError> {
            let snapshot = {
                let existing_cards = self.existing_cards.read().await;
                card_ids
                    .iter()
                    .copied()
                    .filter(|card_id| existing_cards.contains(card_id))
                    .map(stored_card)
                    .collect::<Vec<_>>()
            };
            let release = Arc::new(tokio::sync::Notify::new());
            self.lookup_started
                .send(BlockedCardLookup {
                    requested: card_ids,
                    release: release.clone(),
                })
                .unwrap();
            release.notified().await;
            Ok(snapshot)
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

        async fn get_agent_secret_revision(
            &self,
            _environment_id: EnvironmentId,
            _agent_secret_id: golem_common::model::agent_secret::AgentSecretId,
            _path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
            _revision: golem_common::model::agent_secret::AgentSecretRevision,
        ) -> Result<
            Option<golem_service_base::model::agent_secret::AgentSecret>,
            RegistryServiceError,
        > {
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

    async fn expect_blocked_lookup(
        lookup_started: &mut tokio::sync::mpsc::UnboundedReceiver<BlockedCardLookup>,
        expected: &[CardId],
    ) -> Arc<tokio::sync::Notify> {
        let lookup = lookup_started.recv().await.unwrap();
        assert_eq!(
            lookup.requested.into_iter().collect::<BTreeSet<_>>(),
            expected.iter().copied().collect::<BTreeSet<_>>()
        );
        lookup.release
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

    #[test]
    async fn invalidate_all_drops_live_cache_and_rediscovers_revocation() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let existing_cards = Arc::new(RwLock::new(HashSet::from([card_id])));
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_shared_existing(
            existing_cards.clone(),
            lookup_count.clone(),
        )));

        assert_live(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);

        // The card is revoked at the registry during a cursor gap.
        existing_cards.write().await.remove(&card_id);

        service.invalidate_all().await;

        // The flushed live cache forces a re-fetch, which rediscovers the revocation.
        assert_revoked(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_eq!(lookup_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    async fn invalidate_all_keeps_revoked_entries_without_refetch() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let service = CardServiceDefault::new(Arc::new(TestRegistryService::with_existing(
            HashSet::from([card_id]),
            lookup_count.clone(),
        )));

        service.record_revoked_cards(&[card_id]).await;
        service.invalidate_all().await;

        assert_revoked(&service.check_cards(vec![card_id]).await.unwrap(), card_id);
        assert_eq!(lookup_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    async fn invalidate_all_is_not_undone_by_in_flight_live_lookup() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let card_id = CardId::new();
        let existing_cards = Arc::new(RwLock::new(HashSet::from([card_id])));
        let first_lookup_started = Arc::new(tokio::sync::Notify::new());
        let release_first_lookup = Arc::new(tokio::sync::Notify::new());
        let service = Arc::new(CardServiceDefault::new(Arc::new(
            TestRegistryService::with_delayed_first_shared_existing(
                existing_cards.clone(),
                lookup_count.clone(),
                first_lookup_started.clone(),
                release_first_lookup.clone(),
            ),
        )));

        let in_flight_lookup = tokio::spawn({
            let service = service.clone();
            async move { service.check_cards(vec![card_id]).await.unwrap() }
        });
        first_lookup_started.notified().await;

        service.invalidate_all().await;
        existing_cards.write().await.remove(&card_id);

        release_first_lookup.notify_one();
        let _ = in_flight_lookup.await.unwrap();

        let states = service.check_cards(vec![card_id]).await.unwrap();
        assert_eq!(
            lookup_count.load(Ordering::SeqCst),
            2,
            "check_cards after invalidate_all must re-fetch even if a stale live lookup completed after invalidation"
        );
        assert_revoked(&states, card_id);
    }

    #[test]
    async fn check_cards_honors_revocation_recorded_during_invalidated_mixed_fetch() {
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let cached_card_id = CardId::new();
        let fetched_card_id = CardId::new();
        let existing_cards = Arc::new(RwLock::new(HashSet::from([
            cached_card_id,
            fetched_card_id,
        ])));
        let first_lookup_started = Arc::new(tokio::sync::Notify::new());
        let release_first_lookup = Arc::new(tokio::sync::Notify::new());
        let service = Arc::new(CardServiceDefault::new(Arc::new(
            TestRegistryService::with_delayed_shared_existing(
                existing_cards.clone(),
                lookup_count.clone(),
                first_lookup_started.clone(),
                release_first_lookup.clone(),
                1,
            ),
        )));

        assert_live(
            &service.check_cards(vec![cached_card_id]).await.unwrap(),
            cached_card_id,
        );

        let in_flight_lookup = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .check_cards(vec![cached_card_id, fetched_card_id])
                    .await
                    .unwrap()
            }
        });
        first_lookup_started.notified().await;

        service.invalidate_all().await;
        service.record_revoked_cards(&[cached_card_id]).await;

        release_first_lookup.notify_one();
        let states = in_flight_lookup.await.unwrap();

        assert_revoked(&states, cached_card_id);
    }

    #[test]
    async fn exhausted_generation_retries_do_not_return_invalidated_cached_live_hit() {
        let cached_card_id = CardId::new();
        let fetched_card_id = CardId::new();
        let existing_cards = Arc::new(RwLock::new(HashSet::from([
            cached_card_id,
            fetched_card_id,
        ])));
        let (lookup_started, mut lookup_started_rx) = tokio::sync::mpsc::unbounded_channel();
        let service = Arc::new(CardServiceDefault::new(Arc::new(BlockingRegistryService {
            existing_cards: existing_cards.clone(),
            lookup_started,
        })));

        let precache = tokio::spawn({
            let service = service.clone();
            async move { service.check_cards(vec![cached_card_id]).await.unwrap() }
        });
        expect_blocked_lookup(&mut lookup_started_rx, &[cached_card_id])
            .await
            .notify_one();
        assert_live(&precache.await.unwrap(), cached_card_id);

        let mixed_lookup = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .check_cards(vec![cached_card_id, fetched_card_id])
                    .await
                    .unwrap()
            }
        });

        for _ in 0..7 {
            let blocked_mixed_lookup =
                expect_blocked_lookup(&mut lookup_started_rx, &[fetched_card_id]).await;

            service.invalidate_all().await;

            let repopulate_cached_card = tokio::spawn({
                let service = service.clone();
                async move { service.check_cards(vec![cached_card_id]).await.unwrap() }
            });
            expect_blocked_lookup(&mut lookup_started_rx, &[cached_card_id])
                .await
                .notify_one();
            assert_live(&repopulate_cached_card.await.unwrap(), cached_card_id);

            blocked_mixed_lookup.notify_one();
        }

        let final_blocked_mixed_lookup =
            expect_blocked_lookup(&mut lookup_started_rx, &[fetched_card_id]).await;
        service.invalidate_all().await;
        existing_cards.write().await.remove(&cached_card_id);
        final_blocked_mixed_lookup.notify_one();

        let states = mixed_lookup.await.unwrap();
        assert!(
            !matches!(states.get(&cached_card_id), Some(CardState::Live(_))),
            "invalidated cached live hit was returned without being revalidated: {states:?}"
        );
    }

    #[test]
    async fn exhausted_generation_retries_do_not_return_final_fetch_live_from_before_invalidation()
    {
        let card_id = CardId::new();
        let existing_cards = Arc::new(RwLock::new(HashSet::from([card_id])));
        let (lookup_started, mut lookup_started_rx) = tokio::sync::mpsc::unbounded_channel();
        let service = Arc::new(CardServiceDefault::new(Arc::new(BlockingRegistryService {
            existing_cards: existing_cards.clone(),
            lookup_started,
        })));

        let lookup = tokio::spawn({
            let service = service.clone();
            async move { service.check_cards(vec![card_id]).await.unwrap() }
        });

        for _ in 0..7 {
            let blocked_lookup = expect_blocked_lookup(&mut lookup_started_rx, &[card_id]).await;
            service.invalidate_all().await;
            blocked_lookup.notify_one();
        }

        let final_blocked_lookup = expect_blocked_lookup(&mut lookup_started_rx, &[card_id]).await;
        existing_cards.write().await.remove(&card_id);
        service.invalidate_all().await;
        final_blocked_lookup.notify_one();

        let states = lookup.await.unwrap();
        assert!(
            !matches!(states.get(&card_id), Some(CardState::Live(_))),
            "final fetch taken before invalidate_all was returned as live after invalidation: {states:?}"
        );
    }
}
