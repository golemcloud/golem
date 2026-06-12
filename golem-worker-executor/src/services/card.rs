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
use golem_common::model::OwnedAgentId;
use golem_common::model::card::CardId;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLiveness {
    Live,
    Revoked { newly_detected: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveCardEvent {
    CardRevoked(CardId),
}

impl CardLiveness {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }

    pub fn newly_detected_revocation(self) -> bool {
        matches!(
            self,
            Self::Revoked {
                newly_detected: true
            }
        )
    }
}

#[async_trait]
pub trait CardService: Send + Sync {
    fn register_agent_cards(&self, agent_id: OwnedAgentId, card_ids: &[CardId]);

    fn unregister_agent(&self, agent_id: &OwnedAgentId);

    fn enqueue_revoked_cards_for_agent(&self, agent_id: &OwnedAgentId, card_ids: &[CardId]);

    fn record_revoked_cards(&self, card_ids: &[CardId]) -> Vec<OwnedAgentId>;

    fn drain_live_card_events(&self, agent_id: &OwnedAgentId) -> Vec<LiveCardEvent>;

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardLiveness>, WorkerExecutorError>;
}

pub struct CardServiceDefault {
    registry_service: Arc<dyn RegistryService>,
    negative_index: RwLock<HashSet<CardId>>,
    reverse_index: RwLock<HashMap<CardId, HashSet<OwnedAgentId>>>,
    live_card_events: RwLock<HashMap<OwnedAgentId, VecDeque<LiveCardEvent>>>,
}

pub struct NoopCardService;

#[async_trait]
impl CardService for NoopCardService {
    fn register_agent_cards(&self, _agent_id: OwnedAgentId, _card_ids: &[CardId]) {}

    fn unregister_agent(&self, _agent_id: &OwnedAgentId) {}

    fn enqueue_revoked_cards_for_agent(&self, _agent_id: &OwnedAgentId, _card_ids: &[CardId]) {}

    fn record_revoked_cards(&self, _card_ids: &[CardId]) -> Vec<OwnedAgentId> {
        Vec::new()
    }

    fn drain_live_card_events(&self, _agent_id: &OwnedAgentId) -> Vec<LiveCardEvent> {
        Vec::new()
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardLiveness>, WorkerExecutorError> {
        Ok(card_ids
            .into_iter()
            .map(|card_id| (card_id, CardLiveness::Live))
            .collect())
    }
}

impl CardServiceDefault {
    pub fn new(registry_service: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service,
            negative_index: RwLock::new(HashSet::new()),
            reverse_index: RwLock::new(HashMap::new()),
            live_card_events: RwLock::new(HashMap::new()),
        }
    }

    fn cache_revoked_cards(&self, card_ids: &[CardId]) {
        let mut negative_index = self.negative_index.write().unwrap();
        negative_index.extend(card_ids.iter().copied());
    }

    fn remove_revoked_cards_from_reverse_index(&self, card_ids: &[CardId]) {
        let mut reverse_index = self.reverse_index.write().unwrap();
        for card_id in card_ids {
            reverse_index.remove(card_id);
        }
    }

    fn queue_revoked_cards_for_agent(&self, agent_id: &OwnedAgentId, card_ids: &[CardId]) {
        if card_ids.is_empty() {
            return;
        }

        let mut live_card_events = self.live_card_events.write().unwrap();
        let queue = live_card_events.entry(agent_id.clone()).or_default();
        let mut existing_revocations = queue
            .iter()
            .map(|event| match event {
                LiveCardEvent::CardRevoked(card_id) => *card_id,
            })
            .collect::<HashSet<_>>();

        for card_id in card_ids {
            if existing_revocations.insert(*card_id) {
                queue.push_back(LiveCardEvent::CardRevoked(*card_id));
            }
        }
    }
}

#[async_trait]
impl CardService for CardServiceDefault {
    fn register_agent_cards(&self, agent_id: OwnedAgentId, card_ids: &[CardId]) {
        if card_ids.is_empty() {
            return;
        }

        let negative_index = self.negative_index.read().unwrap();
        let live_card_ids = card_ids
            .iter()
            .copied()
            .filter(|card_id| !negative_index.contains(card_id))
            .collect::<Vec<_>>();
        drop(negative_index);

        if live_card_ids.is_empty() {
            return;
        }

        let mut reverse_index = self.reverse_index.write().unwrap();
        for card_id in live_card_ids {
            reverse_index
                .entry(card_id)
                .or_default()
                .insert(agent_id.clone());
        }
    }

    fn unregister_agent(&self, agent_id: &OwnedAgentId) {
        let mut reverse_index = self.reverse_index.write().unwrap();
        reverse_index.retain(|_, agents| {
            agents.remove(agent_id);
            !agents.is_empty()
        });
        drop(reverse_index);

        self.live_card_events.write().unwrap().remove(agent_id);
    }

    fn enqueue_revoked_cards_for_agent(&self, agent_id: &OwnedAgentId, card_ids: &[CardId]) {
        self.cache_revoked_cards(card_ids);
        self.remove_revoked_cards_from_reverse_index(card_ids);
        self.queue_revoked_cards_for_agent(agent_id, card_ids);
    }

    fn record_revoked_cards(&self, card_ids: &[CardId]) -> Vec<OwnedAgentId> {
        self.cache_revoked_cards(card_ids);

        let affected_agents = {
            let reverse_index = self.reverse_index.read().unwrap();
            card_ids
                .iter()
                .filter_map(|card_id| reverse_index.get(card_id))
                .flat_map(|agents| agents.iter().cloned())
                .collect::<HashSet<_>>()
        };

        for agent_id in &affected_agents {
            let affected_card_ids = {
                let reverse_index = self.reverse_index.read().unwrap();
                card_ids
                    .iter()
                    .copied()
                    .filter(|card_id| {
                        reverse_index
                            .get(card_id)
                            .is_some_and(|agents| agents.contains(agent_id))
                    })
                    .collect::<Vec<_>>()
            };
            self.queue_revoked_cards_for_agent(agent_id, &affected_card_ids);
        }
        self.remove_revoked_cards_from_reverse_index(card_ids);

        affected_agents.into_iter().collect()
    }

    fn drain_live_card_events(&self, agent_id: &OwnedAgentId) -> Vec<LiveCardEvent> {
        self.live_card_events
            .write()
            .unwrap()
            .remove(agent_id)
            .map(VecDeque::into_iter)
            .map(Iterator::collect)
            .unwrap_or_default()
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardLiveness>, WorkerExecutorError> {
        let revoked_cards = self.negative_index.read().unwrap().clone();
        let mut result = HashMap::with_capacity(card_ids.len());
        let mut needs_registry_lookup = Vec::new();

        for card_id in card_ids {
            if revoked_cards.contains(&card_id) {
                result.insert(
                    card_id,
                    CardLiveness::Revoked {
                        newly_detected: false,
                    },
                );
            } else if !result.contains_key(&card_id) {
                needs_registry_lookup.push(card_id);
            }
        }

        if needs_registry_lookup.is_empty() {
            return Ok(result);
        }

        let existing = self
            .registry_service
            .batch_get_existing_cards(needs_registry_lookup.clone())
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "Failed checking card existence: {}",
                    err.to_safe_string()
                ))
            })?;
        let existing = existing.into_iter().collect::<HashSet<_>>();
        let missing = needs_registry_lookup
            .iter()
            .copied()
            .filter(|card_id| !existing.contains(card_id))
            .collect::<Vec<_>>();
        self.cache_revoked_cards(&missing);

        for card_id in needs_registry_lookup {
            let liveness = if existing.contains(&card_id) {
                CardLiveness::Live
            } else {
                CardLiveness::Revoked {
                    newly_detected: true,
                }
            };
            result.insert(card_id, liveness);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use golem_common::model::AgentId;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentType, ResolvedAgentType};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::auth::TokenSecret;
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
    use test_r::test;

    struct TestRegistryService;

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
            Ok(card_ids)
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
        CardServiceDefault::new(Arc::new(TestRegistryService))
    }

    fn agent(name: &str) -> OwnedAgentId {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: name.to_string(),
        };
        OwnedAgentId::new(EnvironmentId::new(), &agent_id)
    }

    #[test]
    fn noop_card_service_treats_cards_as_live() {
        let service = NoopCardService;
        let revoked = CardId::new();

        assert!(
            futures::executor::block_on(service.check_cards(vec![revoked]))
                .unwrap()
                .get(&revoked)
                .copied()
                .unwrap()
                .is_live()
        );
    }

    #[test]
    fn revoked_card_is_queued_for_registered_agent() {
        let service = service();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        service.register_agent_cards(agent.clone(), &[card_id]);
        let affected_agents = service.record_revoked_cards(&[card_id]);

        assert_eq!(affected_agents, vec![agent.clone()]);
        assert_eq!(
            service.drain_live_card_events(&agent),
            vec![LiveCardEvent::CardRevoked(card_id)]
        );
        assert!(service.drain_live_card_events(&agent).is_empty());
    }

    #[test]
    fn unrelated_revoked_card_does_not_queue_event() {
        let service = service();
        let agent = agent("agent-1");
        let live_card_id = CardId::new();
        let revoked_card_id = CardId::new();

        service.register_agent_cards(agent.clone(), &[live_card_id]);
        let affected_agents = service.record_revoked_cards(&[revoked_card_id]);

        assert!(affected_agents.is_empty());
        assert!(service.drain_live_card_events(&agent).is_empty());
    }

    #[test]
    fn unregister_agent_removes_reverse_index_and_events() {
        let service = service();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        service.register_agent_cards(agent.clone(), &[card_id]);
        service.enqueue_revoked_cards_for_agent(&agent, &[card_id]);
        service.unregister_agent(&agent);

        assert!(service.record_revoked_cards(&[card_id]).is_empty());
        assert!(service.drain_live_card_events(&agent).is_empty());
    }

    #[test]
    fn enqueue_revoked_cards_for_agent_deduplicates_events_and_caches_revocation() {
        let service = service();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        service.enqueue_revoked_cards_for_agent(&agent, &[card_id]);
        service.enqueue_revoked_cards_for_agent(&agent, &[card_id]);

        assert_eq!(
            service.drain_live_card_events(&agent),
            vec![LiveCardEvent::CardRevoked(card_id)]
        );
        assert_eq!(
            futures::executor::block_on(service.check_cards(vec![card_id]))
                .unwrap()
                .get(&card_id)
                .copied(),
            Some(CardLiveness::Revoked {
                newly_detected: false
            })
        );
    }
}
