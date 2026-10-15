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

use golem_common::model::OwnedAgentId;
use golem_common::model::card::CardId;
use golem_common::model::entity::EntityInvocationId;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CardAuthorityRecoveryEpoch(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CardAuthorityRecoveryFinalize {
    Reopened,
    InterestChanged,
    StaleEpoch,
}

struct CardAuthorityRecovery {
    state: AtomicU64,
    changes: watch::Sender<u64>,
}

impl Default for CardAuthorityRecovery {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            state: AtomicU64::new(0),
            changes,
        }
    }
}

impl CardAuthorityRecovery {
    const CLOSED_BIT: u64 = 1;

    fn is_open_state(state: u64) -> bool {
        state & Self::CLOSED_BIT == 0
    }

    fn epoch(state: u64) -> CardAuthorityRecoveryEpoch {
        CardAuthorityRecoveryEpoch(state >> 1)
    }

    fn is_open(&self) -> bool {
        Self::is_open_state(self.state.load(Ordering::Acquire))
    }

    fn close(&self) -> CardAuthorityRecoveryEpoch {
        let mut current = self.state.load(Ordering::Acquire);
        loop {
            let next_epoch = (current >> 1).wrapping_add(1);
            let closed = (next_epoch << 1) | Self::CLOSED_BIT;
            match self.state.compare_exchange_weak(
                current,
                closed,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.changes.send_replace(closed);
                    return CardAuthorityRecoveryEpoch(next_epoch);
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn is_current(&self, epoch: CardAuthorityRecoveryEpoch) -> bool {
        let state = self.state.load(Ordering::Acquire);
        !Self::is_open_state(state) && Self::epoch(state) == epoch
    }

    fn reopen(&self, epoch: CardAuthorityRecoveryEpoch) -> bool {
        let closed = (epoch.0 << 1) | Self::CLOSED_BIT;
        let open = epoch.0 << 1;
        if self
            .state
            .compare_exchange(closed, open, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.changes.send_replace(open);
            true
        } else {
            false
        }
    }

    async fn wait_until_open(&self) {
        if self.is_open() {
            return;
        }

        let mut changes = self.changes.subscribe();
        loop {
            if self.is_open() {
                return;
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    }
}

#[derive(Default)]
struct CardInterests {
    revision: u64,
    by_card: HashMap<CardId, HashSet<OwnedAgentId>>,
    by_owner: HashMap<OwnedAgentId, OwnerCardInterests>,
}

struct OwnerCardInterests {
    generation: u64,
    wallet: HashSet<CardId>,
    contributions: HashMap<u64, HashSet<CardId>>,
    entities: HashMap<EntityInvocationId, Weak<CardInterestContribution>>,
}

impl OwnerCardInterests {
    fn union(&self) -> HashSet<CardId> {
        self.wallet
            .iter()
            .chain(self.contributions.values().flatten())
            .copied()
            .collect()
    }
}

pub(crate) struct CardInterestGeneration {
    index: Weak<CardInterestIndex>,
    owner: OwnedAgentId,
    id: u64,
}

pub(crate) struct CardInterestContribution {
    generation: Arc<CardInterestGeneration>,
    id: u64,
    entity: Option<EntityInvocationId>,
    roots: HashSet<CardId>,
}

impl CardInterestGeneration {
    pub(crate) async fn activate(&self, cards: &[CardId]) {
        let Some(index) = self.index.upgrade() else {
            return;
        };
        let _cleanup = index.cleanup.lock().await;
        let mut interests = index.interests.lock().unwrap();
        let previous = interests
            .by_owner
            .get(&self.owner)
            .map(OwnerCardInterests::union)
            .unwrap_or_default();
        let current = cards.iter().copied().collect();
        interests.by_owner.insert(
            self.owner.clone(),
            OwnerCardInterests {
                generation: self.id,
                wallet: cards.iter().copied().collect(),
                contributions: HashMap::new(),
                entities: HashMap::new(),
            },
        );
        CardInterestIndex::replace_union(&mut interests, &self.owner, previous, current);
    }

    pub(crate) fn is_current(&self) -> bool {
        self.index.upgrade().is_some_and(|index| {
            index
                .interests
                .lock()
                .unwrap()
                .by_owner
                .get(&self.owner)
                .is_some_and(|owner| owner.generation == self.id)
        })
    }

    pub(crate) fn set_wallet(&self, cards: impl Iterator<Item = CardId>) {
        self.update(|owner| owner.wallet = cards.collect());
    }

    fn update(&self, update: impl FnOnce(&mut OwnerCardInterests)) {
        let Some(index) = self.index.upgrade() else {
            return;
        };
        let mut interests = index.interests.lock().unwrap();
        let Some(owner) = interests
            .by_owner
            .get_mut(&self.owner)
            .filter(|owner| owner.generation == self.id)
        else {
            return;
        };
        let previous = owner.union();
        update(owner);
        let current = owner.union();
        CardInterestIndex::replace_union(&mut interests, &self.owner, previous, current);
    }

    pub(crate) fn contribute(
        self: &Arc<Self>,
        entity: Option<EntityInvocationId>,
        roots: &[CardId],
    ) -> Arc<CardInterestContribution> {
        let index = self
            .index
            .upgrade()
            .expect("interest index outlives resident wallet");
        let mut interests = index.interests.lock().unwrap();
        let owner = interests
            .by_owner
            .get_mut(&self.owner)
            .filter(|owner| owner.generation == self.id);
        if let Some(existing) = owner
            .as_ref()
            .and_then(|owner| {
                entity
                    .as_ref()
                    .and_then(|entity| owner.entities.get(entity))
            })
            .and_then(Weak::upgrade)
        {
            return existing;
        }
        let guard = Arc::new(CardInterestContribution {
            generation: self.clone(),
            id: index.next_id.fetch_add(1, Ordering::Relaxed),
            entity,
            roots: roots.iter().copied().collect(),
        });
        if let Some(owner) = owner {
            let previous = owner.union();
            owner.contributions.insert(guard.id, guard.roots.clone());
            if let Some(entity) = &guard.entity {
                owner
                    .entities
                    .insert(entity.clone(), Arc::downgrade(&guard));
            }
            let current = owner.union();
            CardInterestIndex::replace_union(&mut interests, &self.owner, previous, current);
        }
        guard
    }

    pub(crate) fn retire(&self) {
        if let Some(index) = self.index.upgrade() {
            let mut interests = index.interests.lock().unwrap();
            if interests
                .by_owner
                .get(&self.owner)
                .is_some_and(|owner| owner.generation == self.id)
            {
                CardInterestIndex::remove_owner(&mut interests, &self.owner);
            }
        }
    }
}

impl Drop for CardInterestGeneration {
    fn drop(&mut self) {
        self.retire();
    }
}

impl CardInterestContribution {
    pub(crate) fn invalidate(&self) {
        self.generation.update(|owner| {
            owner.contributions.remove(&self.id);
        });
    }

    pub(crate) fn roots_include(&self, cards: &[CardId]) -> bool {
        cards.iter().any(|id| self.roots.contains(id))
    }

    pub(crate) fn roots(&self) -> &HashSet<CardId> {
        &self.roots
    }
}

impl Drop for CardInterestContribution {
    fn drop(&mut self) {
        self.generation.update(|owner| {
            owner.contributions.remove(&self.id);
            if let Some(entity) = &self.entity
                && owner
                    .entities
                    .get(entity)
                    .is_some_and(|guard| std::ptr::eq(guard.as_ptr(), self))
            {
                owner.entities.remove(entity);
            }
        });
    }
}

#[derive(Default)]
pub struct CardInterestIndex {
    interests: Mutex<CardInterests>,
    cleanup: tokio::sync::Mutex<()>,
    next_id: AtomicU64,
    authority_recovery: CardAuthorityRecovery,
}

impl CardInterestIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn generation(self: &Arc<Self>, owner: OwnedAgentId) -> Arc<CardInterestGeneration> {
        Arc::new(CardInterestGeneration {
            index: Arc::downgrade(self),
            owner,
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
        })
    }

    fn replace_union(
        interests: &mut CardInterests,
        agent_id: &OwnedAgentId,
        previous: HashSet<CardId>,
        current: HashSet<CardId>,
    ) {
        if previous == current {
            return;
        }
        Self::remove_agent_from_all_cards(&mut interests.by_card, agent_id);
        for card_id in current {
            interests
                .by_card
                .entry(card_id)
                .or_default()
                .insert(agent_id.clone());
        }
        interests.revision = interests.revision.wrapping_add(1);
    }

    fn remove_owner(interests: &mut CardInterests, agent_id: &OwnedAgentId) {
        if let Some(owner) = interests.by_owner.remove(agent_id) {
            Self::replace_union(interests, agent_id, owner.union(), HashSet::new());
        }
    }

    /// Clears an agent's interests only when `condition` succeeds. Generation
    /// activation is serialized with the condition so a replacement for the
    /// same agent cannot register interests between cache removal and cleanup.
    pub(crate) async fn clear_agent_interest_if(
        &self,
        agent_id: &OwnedAgentId,
        condition: impl Future<Output = bool> + Send,
    ) -> bool {
        let _cleanup = self.cleanup.lock().await;
        if !condition.await {
            return false;
        }
        Self::remove_owner(&mut self.interests.lock().unwrap(), agent_id);
        true
    }

    pub async fn tracked_card_ids(&self) -> Vec<CardId> {
        let interests = self.interests.lock().unwrap();
        interests.by_card.keys().copied().collect()
    }

    pub(crate) async fn tracked_card_ids_with_revision(&self) -> (u64, Vec<CardId>) {
        let interests = self.interests.lock().unwrap();
        (
            interests.revision,
            interests.by_card.keys().copied().collect(),
        )
    }

    pub(crate) fn close_authority(&self) -> CardAuthorityRecoveryEpoch {
        self.authority_recovery.close()
    }

    pub(crate) fn authority_is_open(&self) -> bool {
        self.authority_recovery.is_open()
    }

    pub(crate) async fn wait_until_authority_open(&self) {
        self.authority_recovery.wait_until_open().await;
    }

    pub(crate) fn is_current_recovery(&self, epoch: CardAuthorityRecoveryEpoch) -> bool {
        self.authority_recovery.is_current(epoch)
    }

    pub(crate) async fn finalize_recovery(
        &self,
        epoch: CardAuthorityRecoveryEpoch,
        expected_interest_revision: u64,
    ) -> CardAuthorityRecoveryFinalize {
        let interests = self.interests.lock().unwrap();
        if interests.revision != expected_interest_revision {
            return CardAuthorityRecoveryFinalize::InterestChanged;
        }
        if self.authority_recovery.reopen(epoch) {
            CardAuthorityRecoveryFinalize::Reopened
        } else {
            CardAuthorityRecoveryFinalize::StaleEpoch
        }
    }

    pub async fn interested_agents(
        &self,
        card_ids: &[CardId],
    ) -> HashMap<OwnedAgentId, Vec<CardId>> {
        let interests = self.interests.lock().unwrap();
        let mut affected_agent_cards = HashMap::<OwnedAgentId, Vec<CardId>>::new();
        for card_id in card_ids {
            if let Some(agents) = interests.by_card.get(card_id) {
                for agent_id in agents {
                    affected_agent_cards
                        .entry(agent_id.clone())
                        .or_default()
                        .push(*card_id);
                }
            }
        }
        affected_agent_cards
    }

    fn remove_agent_from_all_cards(
        interests: &mut HashMap<CardId, HashSet<OwnedAgentId>>,
        agent_id: &OwnedAgentId,
    ) {
        interests.retain(|_, agents| {
            agents.remove(agent_id);
            !agents.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use test_r::test;

    fn agent(name: &str) -> OwnedAgentId {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: name.to_string(),
        };
        OwnedAgentId::new(EnvironmentId::new(), &agent_id)
    }

    async fn register(
        index: &Arc<CardInterestIndex>,
        owner: OwnedAgentId,
        cards: &[CardId],
    ) -> Arc<CardInterestGeneration> {
        let generation = index.generation(owner);
        generation.activate(cards).await;
        generation
    }

    #[test]
    async fn revoked_card_finds_interested_agent() {
        let index = Arc::new(CardInterestIndex::new());
        let agent = agent("agent-1");
        let card_id = CardId::new();

        let _registration = register(&index, agent.clone(), &[card_id]).await;

        assert_eq!(
            index.interested_agents(&[card_id]).await.get(&agent),
            Some(&vec![card_id])
        );
    }

    #[test]
    async fn disjoint_revocations_are_routed_per_agent() {
        let index = Arc::new(CardInterestIndex::new());
        let first_agent = agent("agent-1");
        let second_agent = agent("agent-2");
        let first_card = CardId::new();
        let second_card = CardId::new();

        let _first = register(&index, first_agent.clone(), &[first_card]).await;
        let _second = register(&index, second_agent.clone(), &[second_card]).await;

        let affected = index.interested_agents(&[first_card, second_card]).await;
        assert_eq!(affected.get(&first_agent), Some(&vec![first_card]));
        assert_eq!(affected.get(&second_agent), Some(&vec![second_card]));
    }

    #[test]
    async fn unrelated_revoked_card_does_not_affect_agent() {
        let index = Arc::new(CardInterestIndex::new());
        let agent = agent("agent-1");
        let live_card_id = CardId::new();
        let revoked_card_id = CardId::new();

        let _registration = register(&index, agent, &[live_card_id]).await;

        assert!(index.interested_agents(&[revoked_card_id]).await.is_empty());
    }

    #[test]
    async fn setting_card_interest_replaces_previous_cards() {
        let index = Arc::new(CardInterestIndex::new());
        let agent = agent("agent-1");
        let old_card_id = CardId::new();
        let new_card_id = CardId::new();

        let registration = register(&index, agent.clone(), &[old_card_id]).await;
        registration.set_wallet(std::iter::once(new_card_id));

        assert!(index.interested_agents(&[old_card_id]).await.is_empty());
        assert_eq!(
            index.interested_agents(&[new_card_id]).await.get(&agent),
            Some(&vec![new_card_id])
        );
    }

    #[test]
    async fn empty_card_interest_removes_reverse_index() {
        let index = Arc::new(CardInterestIndex::new());
        let agent = agent("agent-1");
        let card_id = CardId::new();

        let registration = register(&index, agent, &[card_id]).await;
        registration.set_wallet(std::iter::empty());

        assert!(index.interested_agents(&[card_id]).await.is_empty());
    }

    #[test]
    async fn tracked_card_ids_reports_all_interested_cards() {
        let index = Arc::new(CardInterestIndex::new());
        let first_agent = agent("agent-1");
        let second_agent = agent("agent-2");
        let card_a = CardId::new();
        let card_b = CardId::new();
        let card_c = CardId::new();

        let _first = register(&index, first_agent, &[card_a, card_b]).await;
        let _second = register(&index, second_agent, &[card_b, card_c]).await;

        let tracked = index.tracked_card_ids().await;
        assert_eq!(tracked.len(), 3);
        assert_eq!(
            tracked.into_iter().collect::<HashSet<_>>(),
            HashSet::from([card_a, card_b, card_c])
        );
    }

    #[test]
    async fn tracked_card_ids_drops_cards_after_wallet_cleared() {
        let index = Arc::new(CardInterestIndex::new());
        let agent = agent("agent-1");
        let card_id = CardId::new();

        let registration = register(&index, agent, &[card_id]).await;
        assert_eq!(index.tracked_card_ids().await, vec![card_id]);

        registration.set_wallet(std::iter::empty());
        assert!(index.tracked_card_ids().await.is_empty());
    }

    #[test]
    async fn card_is_removed_from_reverse_index_only_after_wallet_removal() {
        let index = Arc::new(CardInterestIndex::new());
        let first_agent = agent("agent-1");
        let second_agent = agent("agent-2");
        let card_id = CardId::new();

        let first = register(&index, first_agent.clone(), &[card_id]).await;
        let _second = register(&index, second_agent.clone(), &[card_id]).await;

        let affected_agents = index.interested_agents(&[card_id]).await;
        assert_eq!(affected_agents.len(), 2);
        assert_eq!(affected_agents.get(&first_agent), Some(&vec![card_id]));
        assert_eq!(affected_agents.get(&second_agent), Some(&vec![card_id]));

        first.set_wallet(std::iter::empty());

        let affected_agents = index.interested_agents(&[card_id]).await;
        assert_eq!(affected_agents.len(), 1);
        assert_eq!(affected_agents.get(&second_agent), Some(&vec![card_id]));
    }

    #[test]
    async fn authority_recovery_waits_until_matching_epoch_reopens() {
        let index = CardInterestIndex::new();
        let epoch = index.close_authority();
        assert!(!index.authority_is_open());

        let (_, tracked) = index.tracked_card_ids_with_revision().await;
        assert!(tracked.is_empty());
        let revision = index.tracked_card_ids_with_revision().await.0;
        assert_eq!(
            index.finalize_recovery(epoch, revision).await,
            CardAuthorityRecoveryFinalize::Reopened
        );

        index.wait_until_authority_open().await;
        assert!(index.authority_is_open());
    }

    #[test]
    async fn stale_recovery_cannot_reopen_newer_epoch() {
        let index = CardInterestIndex::new();
        let stale = index.close_authority();
        let current = index.close_authority();
        let revision = index.tracked_card_ids_with_revision().await.0;

        assert_eq!(
            index.finalize_recovery(stale, revision).await,
            CardAuthorityRecoveryFinalize::StaleEpoch
        );
        assert!(!index.authority_is_open());
        assert_eq!(
            index.finalize_recovery(current, revision).await,
            CardAuthorityRecoveryFinalize::Reopened
        );
    }

    #[test]
    async fn interest_change_prevents_recovery_reopen() {
        let index = Arc::new(CardInterestIndex::new());
        let epoch = index.close_authority();
        let (revision, _) = index.tracked_card_ids_with_revision().await;

        let _registration = register(&index, agent("agent-1"), &[CardId::new()]).await;

        assert_eq!(
            index.finalize_recovery(epoch, revision).await,
            CardAuthorityRecoveryFinalize::InterestChanged
        );
        assert!(!index.authority_is_open());
    }

    #[test]
    async fn unchanged_interest_does_not_advance_recovery_revision() {
        let index = Arc::new(CardInterestIndex::new());
        let agent = agent("agent-1");
        let card_id = CardId::new();
        let registration = register(&index, agent, &[card_id]).await;
        let (revision, _) = index.tracked_card_ids_with_revision().await;

        registration.set_wallet(std::iter::once(card_id));

        assert_eq!(index.tracked_card_ids_with_revision().await.0, revision);
    }

    #[test]
    async fn scope_and_candidate_interest_survives_wallet_and_sibling_cleanup() {
        let index = Arc::new(CardInterestIndex::new());
        let owner = agent("owner");
        let wallet = CardId::new();
        let root = CardId::new();
        let candidate = CardId::new();
        let generation = register(&index, owner.clone(), &[wallet]).await;
        let first = generation.contribute(None, &[root]);
        let second = generation.contribute(None, &[root]);
        let installing = generation.contribute(None, &[candidate]);
        let revision = index.tracked_card_ids_with_revision().await.0;
        drop(first);
        assert_eq!(index.tracked_card_ids_with_revision().await.0, revision);
        generation.set_wallet([candidate].into_iter());
        drop(installing);
        assert!(index.interested_agents(&[wallet]).await.is_empty());
        assert_eq!(
            index.interested_agents(&[root]).await.get(&owner),
            Some(&vec![root])
        );
        assert_eq!(
            index.interested_agents(&[candidate]).await.get(&owner),
            Some(&vec![candidate])
        );
        drop(second);
        assert!(index.interested_agents(&[root]).await.is_empty());
        assert_eq!(index.tracked_card_ids().await, vec![candidate]);
        drop(generation);
        assert!(index.tracked_card_ids().await.is_empty());
    }

    #[test]
    async fn entity_guard_handoff_cannot_resurrect_invalidated_scope() {
        use golem_common::model::entity::{AgentEntity, OwnedAgentEntityId};
        use golem_common::model::oplog::OplogIndex;
        use golem_common::model::tool::ToolName;

        let index = Arc::new(CardInterestIndex::new());
        let owner = agent("owner");
        let root = CardId::new();
        let generation = register(&index, owner.clone(), &[]).await;
        let entity = EntityInvocationId::new(
            OwnedAgentEntityId {
                owner,
                entity: AgentEntity::Tool(ToolName::try_from("scoped").unwrap()),
            },
            OplogIndex::from_u64(42),
        )
        .unwrap();
        let admission = generation.contribute(Some(entity.clone()), &[root]);
        let store = generation.contribute(Some(entity.clone()), &[root]);
        drop(admission);
        assert_eq!(index.tracked_card_ids().await, vec![root]);
        store.invalidate();
        let retained = generation.contribute(Some(entity), &[root]);
        assert!(index.tracked_card_ids().await.is_empty());
        drop(store);
        drop(retained);
        assert!(index.tracked_card_ids().await.is_empty());
    }

    #[test]
    async fn stale_generation_cleanup_cannot_remove_replacement_interest() {
        let index = Arc::new(CardInterestIndex::new());
        let owner = agent("owner");
        let old = CardId::new();
        let new = CardId::new();
        let generation = register(&index, owner.clone(), &[old]).await;
        let retained = generation.contribute(None, &[old]);
        let replacement = register(&index, owner, &[new]).await;
        generation.set_wallet([old].into_iter());
        let late = generation.contribute(None, &[old]);
        generation.retire();
        drop(retained);
        drop(late);
        drop(generation);
        assert_eq!(index.tracked_card_ids().await, vec![new]);
        assert!(replacement.is_current());
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn conditional_removal_serializes_replacement_activation() {
        let index = Arc::new(CardInterestIndex::new());
        let owner = agent("owner");
        let card = CardId::new();
        let old = register(&index, owner.clone(), &[card]).await;
        let (entered, entry) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let clearing = tokio::spawn({
            let index = index.clone();
            let owner = owner.clone();
            async move {
                index
                    .clear_agent_interest_if(&owner, async {
                        entered.send(()).unwrap();
                        released.await.unwrap();
                        true
                    })
                    .await
            }
        });
        entry.await.unwrap();
        let replacement = index.generation(owner);
        let cards = [card];
        let mut activation = Box::pin(replacement.activate(&cards));
        assert!(futures::poll!(&mut activation).is_pending());
        release.send(()).unwrap();
        assert!(clearing.await.unwrap());
        activation.await;
        drop(old);
        assert!(replacement.is_current());
        assert_eq!(index.tracked_card_ids().await, vec![card]);
        assert!(
            !index
                .clear_agent_interest_if(&replacement.owner, async { false })
                .await
        );
        assert_eq!(index.tracked_card_ids().await, vec![card]);
        assert!(
            index
                .clear_agent_interest_if(&replacement.owner, async { true })
                .await
        );
        replacement.set_wallet([card].into_iter());
        assert!(index.tracked_card_ids().await.is_empty());
    }
}
