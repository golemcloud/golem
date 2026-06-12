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
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveCardEvent {
    CardRevoked(CardId),
}

#[async_trait]
pub trait CardService: Send + Sync {
    async fn register_agent(&self, agent_id: OwnedAgentId);

    async fn register_agent_cards(&self, agent_id: OwnedAgentId, card_ids: &[CardId]);

    async fn unregister_agent(&self, agent_id: &OwnedAgentId);

    async fn enqueue_revoked_cards_for_agent(&self, agent_id: &OwnedAgentId, card_ids: &[CardId]);

    async fn record_known_revoked_cards(&self, card_ids: &[CardId]);

    async fn record_revoked_cards(&self, card_ids: &[CardId]) -> Vec<OwnedAgentId>;

    async fn drain_live_card_events(&self, agent_id: &OwnedAgentId) -> Vec<LiveCardEvent>;

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashSet<CardId>, WorkerExecutorError>;
}

pub struct CardServiceDefault {
    registry_service: Arc<dyn RegistryService>,
    negative_index: RwLock<HashSet<CardId>>,
    active_agents: RwLock<HashSet<OwnedAgentId>>,
    reverse_index: RwLock<HashMap<CardId, HashSet<OwnedAgentId>>>,
    live_card_events: RwLock<HashMap<OwnedAgentId, VecDeque<LiveCardEvent>>>,
}

pub struct NoopCardService;

#[async_trait]
impl CardService for NoopCardService {
    async fn register_agent(&self, _agent_id: OwnedAgentId) {}

    async fn register_agent_cards(&self, _agent_id: OwnedAgentId, _card_ids: &[CardId]) {}

    async fn unregister_agent(&self, _agent_id: &OwnedAgentId) {}

    async fn enqueue_revoked_cards_for_agent(
        &self,
        _agent_id: &OwnedAgentId,
        _card_ids: &[CardId],
    ) {
    }

    async fn record_known_revoked_cards(&self, _card_ids: &[CardId]) {}

    async fn record_revoked_cards(&self, _card_ids: &[CardId]) -> Vec<OwnedAgentId> {
        Vec::new()
    }

    async fn drain_live_card_events(&self, _agent_id: &OwnedAgentId) -> Vec<LiveCardEvent> {
        Vec::new()
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashSet<CardId>, WorkerExecutorError> {
        Ok(card_ids.into_iter().collect())
    }
}

impl CardServiceDefault {
    pub fn new(registry_service: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service,
            negative_index: RwLock::new(HashSet::new()),
            active_agents: RwLock::new(HashSet::new()),
            reverse_index: RwLock::new(HashMap::new()),
            live_card_events: RwLock::new(HashMap::new()),
        }
    }

    async fn cache_revoked_cards(&self, card_ids: &[CardId]) {
        let mut negative_index = self.negative_index.write().await;
        negative_index.extend(card_ids.iter().copied());
    }

    async fn remove_revoked_cards_from_reverse_index(&self, card_ids: &[CardId]) {
        let mut reverse_index = self.reverse_index.write().await;
        for card_id in card_ids {
            reverse_index.remove(card_id);
        }
    }

    async fn remove_agent_from_reverse_index(&self, agent_id: &OwnedAgentId) {
        let mut reverse_index = self.reverse_index.write().await;
        reverse_index.retain(|_, agents| {
            agents.remove(agent_id);
            !agents.is_empty()
        });
    }

    async fn queue_revoked_cards_for_agent(&self, agent_id: &OwnedAgentId, card_ids: &[CardId]) {
        if card_ids.is_empty() {
            return;
        }

        if !self.active_agents.read().await.contains(agent_id) {
            return;
        }

        let mut live_card_events = self.live_card_events.write().await;
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
    async fn register_agent(&self, agent_id: OwnedAgentId) {
        self.active_agents.write().await.insert(agent_id.clone());
        self.live_card_events
            .write()
            .await
            .entry(agent_id)
            .or_default();
    }

    async fn register_agent_cards(&self, agent_id: OwnedAgentId, card_ids: &[CardId]) {
        if !self.active_agents.read().await.contains(&agent_id) {
            return;
        }

        self.remove_agent_from_reverse_index(&agent_id).await;

        if card_ids.is_empty() {
            return;
        }

        let negative_index = self.negative_index.read().await;
        let live_card_ids = card_ids
            .iter()
            .copied()
            .filter(|card_id| !negative_index.contains(card_id))
            .collect::<Vec<_>>();
        drop(negative_index);

        if live_card_ids.is_empty() {
            return;
        }

        let mut reverse_index = self.reverse_index.write().await;
        for card_id in live_card_ids {
            reverse_index
                .entry(card_id)
                .or_default()
                .insert(agent_id.clone());
        }
    }

    async fn unregister_agent(&self, agent_id: &OwnedAgentId) {
        self.active_agents.write().await.remove(agent_id);

        self.remove_agent_from_reverse_index(agent_id).await;

        self.live_card_events.write().await.remove(agent_id);
    }

    async fn enqueue_revoked_cards_for_agent(&self, agent_id: &OwnedAgentId, card_ids: &[CardId]) {
        self.cache_revoked_cards(card_ids).await;
        self.remove_revoked_cards_from_reverse_index(card_ids).await;
        self.queue_revoked_cards_for_agent(agent_id, card_ids).await;
    }

    async fn record_known_revoked_cards(&self, card_ids: &[CardId]) {
        self.cache_revoked_cards(card_ids).await;
        self.remove_revoked_cards_from_reverse_index(card_ids).await;
    }

    async fn record_revoked_cards(&self, card_ids: &[CardId]) -> Vec<OwnedAgentId> {
        self.cache_revoked_cards(card_ids).await;

        let affected_agent_cards = {
            let reverse_index = self.reverse_index.read().await;
            let mut affected_agent_cards = HashMap::<OwnedAgentId, Vec<CardId>>::new();
            for card_id in card_ids {
                if let Some(agents) = reverse_index.get(card_id) {
                    for agent_id in agents {
                        affected_agent_cards
                            .entry(agent_id.clone())
                            .or_default()
                            .push(*card_id);
                    }
                }
            }
            affected_agent_cards
        };

        for (agent_id, affected_card_ids) in &affected_agent_cards {
            self.queue_revoked_cards_for_agent(agent_id, affected_card_ids)
                .await;
        }
        self.remove_revoked_cards_from_reverse_index(card_ids).await;

        affected_agent_cards.into_keys().collect()
    }

    async fn drain_live_card_events(&self, agent_id: &OwnedAgentId) -> Vec<LiveCardEvent> {
        self.live_card_events
            .write()
            .await
            .remove(agent_id)
            .map(VecDeque::into_iter)
            .map(Iterator::collect)
            .unwrap_or_default()
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashSet<CardId>, WorkerExecutorError> {
        let revoked_cards = self.negative_index.read().await.clone();
        let mut live_cards = HashSet::with_capacity(card_ids.len());
        let mut needs_registry_lookup = Vec::new();
        let mut seen_lookup = HashSet::new();

        for card_id in card_ids {
            if !revoked_cards.contains(&card_id) && seen_lookup.insert(card_id) {
                needs_registry_lookup.push(card_id);
            }
        }

        if needs_registry_lookup.is_empty() {
            return Ok(live_cards);
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
        self.cache_revoked_cards(&missing).await;

        for card_id in needs_registry_lookup {
            if existing.contains(&card_id) {
                live_cards.insert(card_id);
            }
        }

        Ok(live_cards)
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
    async fn noop_card_service_treats_cards_as_live() {
        let service = NoopCardService;
        let revoked = CardId::new();

        assert!(
            service
                .check_cards(vec![revoked])
                .await
                .unwrap()
                .contains(&revoked)
        );
    }

    #[test]
    async fn revoked_card_is_queued_for_registered_agent() {
        let service = service();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        service.register_agent(agent.clone()).await;
        service
            .register_agent_cards(agent.clone(), &[card_id])
            .await;
        let affected_agents = service.record_revoked_cards(&[card_id]).await;

        assert_eq!(affected_agents, vec![agent.clone()]);
        assert_eq!(
            service.drain_live_card_events(&agent).await,
            vec![LiveCardEvent::CardRevoked(card_id)]
        );
        assert!(service.drain_live_card_events(&agent).await.is_empty());
    }

    #[test]
    async fn unrelated_revoked_card_does_not_queue_event() {
        let service = service();
        let agent = agent("agent-1");
        let live_card_id = CardId::new();
        let revoked_card_id = CardId::new();

        service.register_agent(agent.clone()).await;
        service
            .register_agent_cards(agent.clone(), &[live_card_id])
            .await;
        let affected_agents = service.record_revoked_cards(&[revoked_card_id]).await;

        assert!(affected_agents.is_empty());
        assert!(service.drain_live_card_events(&agent).await.is_empty());
    }

    #[test]
    async fn registering_agent_cards_replaces_previous_cards() {
        let service = service();
        let agent = agent("agent-1");
        let old_card_id = CardId::new();
        let new_card_id = CardId::new();

        service.register_agent(agent.clone()).await;
        service
            .register_agent_cards(agent.clone(), &[old_card_id])
            .await;
        service
            .register_agent_cards(agent.clone(), &[new_card_id])
            .await;

        assert!(
            service
                .record_revoked_cards(&[old_card_id])
                .await
                .is_empty()
        );
        assert_eq!(
            service.record_revoked_cards(&[new_card_id]).await,
            vec![agent.clone()]
        );
        assert_eq!(
            service.drain_live_card_events(&agent).await,
            vec![LiveCardEvent::CardRevoked(new_card_id)]
        );
    }

    #[test]
    async fn unregister_agent_removes_reverse_index_and_events() {
        let service = service();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        service.register_agent(agent.clone()).await;
        service
            .register_agent_cards(agent.clone(), &[card_id])
            .await;
        service
            .enqueue_revoked_cards_for_agent(&agent, &[card_id])
            .await;
        service.unregister_agent(&agent).await;

        assert!(service.record_revoked_cards(&[card_id]).await.is_empty());
        assert!(service.drain_live_card_events(&agent).await.is_empty());
    }

    #[test]
    async fn enqueue_revoked_cards_for_agent_deduplicates_events_and_caches_revocation() {
        let service = service();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        service.register_agent(agent.clone()).await;
        service
            .enqueue_revoked_cards_for_agent(&agent, &[card_id])
            .await;
        service
            .enqueue_revoked_cards_for_agent(&agent, &[card_id])
            .await;

        assert_eq!(
            service.drain_live_card_events(&agent).await,
            vec![LiveCardEvent::CardRevoked(card_id)]
        );
        assert!(
            !service
                .check_cards(vec![card_id])
                .await
                .unwrap()
                .contains(&card_id)
        );
    }
}
