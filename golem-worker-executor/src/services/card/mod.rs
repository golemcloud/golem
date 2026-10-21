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
use chrono::Utc;
use golem_common::SafeDisplay;
use golem_common::model::card::{CardId, CardManagedByRuntimeDerived, ScopeCard, StoredCard};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait CardService: Send + Sync {
    async fn record_revoked_cards(&self, card_ids: &[CardId]);

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError>;

    async fn create_runtime_card(
        &self,
        _card: StoredCard,
        _provenance: CardManagedByRuntimeDerived,
    ) -> Result<RuntimeCardCreation, WorkerExecutorError> {
        Err(WorkerExecutorError::runtime(
            "runtime permission-card creation is not supported by this card service",
        ))
    }

    async fn revoke_card(&self, _card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        Err(WorkerExecutorError::runtime(
            "permission-card revocation is not supported by this card service",
        ))
    }

    async fn live_ancestor_ids_including_self(
        &self,
        _card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        Err(WorkerExecutorError::runtime(
            "permission-card ancestor lookup is not supported by this card service",
        ))
    }

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
    revocation_versions: HashMap<CardId, u64>,
    /// A `check_cards` lookup captures this before its registry fetch and
    /// refuses to write results back if it changed in the meantime, so an
    /// in-flight lookup cannot repopulate stale data.
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

/// The outcome of a runtime permission-card creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeCardCreation {
    Created(Box<StoredCard>),
    /// The card was created and then revoked before. The registry never creates a revoked
    /// runtime card again, so every later attempt has this outcome.
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardRevokeResult {
    Revoked(Vec<CardId>),
    AlreadyRevoked(String),
    NotPermitted(String),
}

pub async fn validate_scope_card(
    card_service: &dyn CardService,
    scope_card: &ScopeCard,
) -> Result<Vec<StoredCard>, WorkerExecutorError> {
    if scope_card.scope_card_id.0.is_nil() {
        return Err(WorkerExecutorError::permission_denied(
            "not-permitted: scope-card ID must not be nil",
        ));
    }
    if scope_card.root_card_ids.is_empty() {
        return Err(WorkerExecutorError::permission_denied(
            "not-permitted: scope card must reference at least one root card",
        ));
    }
    if scope_card
        .root_card_ids
        .windows(2)
        .any(|ids| ids[0] >= ids[1])
    {
        return Err(WorkerExecutorError::permission_denied(
            "not-permitted: scope-card root IDs must be strictly increasing",
        ));
    }

    let states = card_service
        .check_cards(scope_card.root_card_ids.clone())
        .await?;
    let mut root_cards = Vec::with_capacity(scope_card.root_card_ids.len());
    for card_id in &scope_card.root_card_ids {
        match states.get(card_id) {
            Some(CardState::Live(card)) if card.card_id() == *card_id => {
                root_cards.push(card.as_ref().clone());
            }
            Some(CardState::Live(_)) => {
                return Err(WorkerExecutorError::permission_denied(format!(
                    "not-permitted: scope-card root {card_id} does not match the registry payload"
                )));
            }
            Some(CardState::Revoked) => {
                return Err(WorkerExecutorError::permission_denied(format!(
                    "card-revoked: scope-card root {card_id} is revoked or expired"
                )));
            }
            Some(CardState::Unknown) | None => {
                return Err(WorkerExecutorError::permission_denied(format!(
                    "not-permitted: scope-card root {card_id} could not be verified"
                )));
            }
        }
    }

    Ok(root_cards)
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
            let version = state.revocation_versions.entry(*card_id).or_default();
            *version = version.wrapping_add(1);
            state.cards.entry(*card_id).or_default().state = CardState::Revoked;
        }
    }

    fn record_live_cards_in_state(state: &mut CardServiceState, cards: Vec<StoredCard>) {
        for card in cards {
            let data = state.cards.entry(card.card_id()).or_default();
            if data.state != CardState::Revoked {
                data.state = if Self::card_is_expired(&card) {
                    CardState::Revoked
                } else {
                    CardState::Live(Box::new(card))
                };
            }
        }
    }

    fn card_is_expired(card: &StoredCard) -> bool {
        card.expires_at()
            .is_some_and(|expires_at| expires_at <= Utc::now())
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
                    if Self::card_is_expired(card) {
                        cached_states.insert(*card_id, CardState::Revoked);
                    } else {
                        cached_states.insert(*card_id, CardState::Live(card.clone()));
                    }
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
            .filter(|card| !Self::card_is_expired(card))
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
        state.cards.get(&card_id).map(|data| match &data.state {
            CardState::Live(card) if Self::card_is_expired(card) => CardState::Revoked,
            state => state.clone(),
        })
    }
}

#[async_trait]
impl CardService for CardServiceDefault {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        let mut state = self.state.write().await;
        Self::record_revoked_cards_in_state(&mut state, card_ids);
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        provenance: CardManagedByRuntimeDerived,
    ) -> Result<RuntimeCardCreation, WorkerExecutorError> {
        let card_id = card.card_id();
        let revocation_version = {
            let state = self.state.read().await;
            state
                .revocation_versions
                .get(&card_id)
                .copied()
                .unwrap_or_default()
        };
        let card = match self
            .registry_service
            .create_runtime_card(card, provenance)
            .await
        {
            Ok(card) => card,
            Err(RegistryServiceError::RuntimeCardRevoked(_)) => {
                self.record_revoked_cards(&[card_id]).await;
                return Ok(RuntimeCardCreation::Revoked);
            }
            Err(err) => {
                return Err(WorkerExecutorError::runtime(format!(
                    "Failed creating runtime permission card: {}",
                    err.to_safe_string()
                )));
            }
        };

        let mut state = self.state.write().await;
        let revocation_is_newer = state
            .revocation_versions
            .get(&card_id)
            .copied()
            .unwrap_or_default()
            != revocation_version;
        state.generation = state.generation.wrapping_add(1);
        if !revocation_is_newer {
            state.cards.entry(card_id).or_default().state = if Self::card_is_expired(&card) {
                CardState::Revoked
            } else {
                CardState::Live(Box::new(card.clone()))
            };
        }
        Ok(RuntimeCardCreation::Created(Box::new(card)))
    }

    async fn revoke_card(&self, card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        match self.registry_service.revoke_card(card_id).await {
            Ok(card_ids) => {
                self.record_revoked_cards(&card_ids).await;
                Ok(CardRevokeResult::Revoked(card_ids))
            }
            Err(error @ RegistryServiceError::NotFound(_)) => {
                self.record_revoked_cards(&[card_id]).await;
                Ok(CardRevokeResult::AlreadyRevoked(error.to_safe_string()))
            }
            Err(error @ RegistryServiceError::AlreadyExists(_)) => {
                match self.registry_service.batch_get_cards(vec![card_id]).await {
                    Ok(cards) if cards.iter().any(|card| card.card_id() == card_id) => {
                        Err(WorkerExecutorError::runtime(format!(
                            "Failed revoking permission card {card_id}: {}",
                            error.to_safe_string()
                        )))
                    }
                    Ok(_) => {
                        self.record_revoked_cards(&[card_id]).await;
                        Ok(CardRevokeResult::AlreadyRevoked(error.to_safe_string()))
                    }
                    Err(lookup_error) => Err(WorkerExecutorError::runtime(format!(
                        "Failed confirming the state of permission card {card_id} after a concurrent revoke failure: {}",
                        lookup_error.to_safe_string()
                    ))),
                }
            }
            Err(
                error @ (RegistryServiceError::Unauthorized(_)
                | RegistryServiceError::BadRequest(_)),
            ) => Ok(CardRevokeResult::NotPermitted(error.to_safe_string())),
            Err(error) => Err(WorkerExecutorError::runtime(format!(
                "Failed revoking permission card {card_id}: {}",
                error.to_safe_string()
            ))),
        }
    }

    async fn live_ancestor_ids_including_self(
        &self,
        card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        let mut ancestors = BTreeSet::from([card.card_id()]);
        let mut visited = HashSet::from([card.card_id()]);
        let mut frontier = card.parent_ids().to_vec();

        while !frontier.is_empty() {
            frontier.retain(|card_id| visited.insert(*card_id));
            if frontier.is_empty() {
                break;
            }

            let states = self.check_cards(frontier.clone()).await?;
            let mut next = Vec::new();
            for card_id in &frontier {
                if let Some(CardState::Live(parent)) = states.get(card_id) {
                    ancestors.insert(*card_id);
                    next.extend_from_slice(parent.parent_ids());
                }
            }
            frontier = next;
        }

        Ok(ancestors.into_iter().collect())
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
                    .filter(|card| !Self::card_is_expired(card))
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
mod tests;
