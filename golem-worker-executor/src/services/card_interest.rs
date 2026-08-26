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
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, watch};

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
}

#[derive(Default)]
pub struct CardInterestIndex {
    interests: RwLock<CardInterests>,
    authority_recovery: CardAuthorityRecovery,
}

impl CardInterestIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set_card_interest(&self, agent_id: OwnedAgentId, card_ids: &[CardId]) {
        let mut interests = self.interests.write().await;
        let previous = interests
            .by_card
            .iter()
            .filter_map(|(card_id, agents)| agents.contains(&agent_id).then_some(*card_id))
            .collect::<HashSet<_>>();
        let current = card_ids.iter().copied().collect::<HashSet<_>>();
        if previous == current {
            return;
        }

        Self::remove_agent_from_all_cards(&mut interests.by_card, &agent_id);

        for card_id in current {
            interests
                .by_card
                .entry(card_id)
                .or_default()
                .insert(agent_id.clone());
        }
        interests.revision = interests.revision.wrapping_add(1);
    }

    pub async fn tracked_card_ids(&self) -> Vec<CardId> {
        let interests = self.interests.read().await;
        interests.by_card.keys().copied().collect()
    }

    pub(crate) async fn tracked_card_ids_with_revision(&self) -> (u64, Vec<CardId>) {
        let interests = self.interests.read().await;
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
        let interests = self.interests.write().await;
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
        let interests = self.interests.read().await;
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

    #[test]
    async fn revoked_card_finds_interested_agent() {
        let index = CardInterestIndex::new();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        index.set_card_interest(agent.clone(), &[card_id]).await;

        assert_eq!(
            index.interested_agents(&[card_id]).await.get(&agent),
            Some(&vec![card_id])
        );
    }

    #[test]
    async fn disjoint_revocations_are_routed_per_agent() {
        let index = CardInterestIndex::new();
        let first_agent = agent("agent-1");
        let second_agent = agent("agent-2");
        let first_card = CardId::new();
        let second_card = CardId::new();

        index
            .set_card_interest(first_agent.clone(), &[first_card])
            .await;
        index
            .set_card_interest(second_agent.clone(), &[second_card])
            .await;

        let affected = index.interested_agents(&[first_card, second_card]).await;
        assert_eq!(affected.get(&first_agent), Some(&vec![first_card]));
        assert_eq!(affected.get(&second_agent), Some(&vec![second_card]));
    }

    #[test]
    async fn unrelated_revoked_card_does_not_affect_agent() {
        let index = CardInterestIndex::new();
        let agent = agent("agent-1");
        let live_card_id = CardId::new();
        let revoked_card_id = CardId::new();

        index.set_card_interest(agent, &[live_card_id]).await;

        assert!(index.interested_agents(&[revoked_card_id]).await.is_empty());
    }

    #[test]
    async fn setting_card_interest_replaces_previous_cards() {
        let index = CardInterestIndex::new();
        let agent = agent("agent-1");
        let old_card_id = CardId::new();
        let new_card_id = CardId::new();

        index.set_card_interest(agent.clone(), &[old_card_id]).await;
        index.set_card_interest(agent.clone(), &[new_card_id]).await;

        assert!(index.interested_agents(&[old_card_id]).await.is_empty());
        assert_eq!(
            index.interested_agents(&[new_card_id]).await.get(&agent),
            Some(&vec![new_card_id])
        );
    }

    #[test]
    async fn empty_card_interest_removes_reverse_index() {
        let index = CardInterestIndex::new();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        index.set_card_interest(agent.clone(), &[card_id]).await;
        index.set_card_interest(agent, &[]).await;

        assert!(index.interested_agents(&[card_id]).await.is_empty());
    }

    #[test]
    async fn tracked_card_ids_reports_all_interested_cards() {
        let index = CardInterestIndex::new();
        let first_agent = agent("agent-1");
        let second_agent = agent("agent-2");
        let card_a = CardId::new();
        let card_b = CardId::new();
        let card_c = CardId::new();

        index
            .set_card_interest(first_agent, &[card_a, card_b])
            .await;
        index
            .set_card_interest(second_agent, &[card_b, card_c])
            .await;

        let tracked = index.tracked_card_ids().await;
        assert_eq!(tracked.len(), 3);
        assert_eq!(
            tracked.into_iter().collect::<HashSet<_>>(),
            HashSet::from([card_a, card_b, card_c])
        );
    }

    #[test]
    async fn tracked_card_ids_drops_cards_after_wallet_cleared() {
        let index = CardInterestIndex::new();
        let agent = agent("agent-1");
        let card_id = CardId::new();

        index.set_card_interest(agent.clone(), &[card_id]).await;
        assert_eq!(index.tracked_card_ids().await, vec![card_id]);

        index.set_card_interest(agent, &[]).await;
        assert!(index.tracked_card_ids().await.is_empty());
    }

    #[test]
    async fn card_is_removed_from_reverse_index_only_after_wallet_removal() {
        let index = CardInterestIndex::new();
        let first_agent = agent("agent-1");
        let second_agent = agent("agent-2");
        let card_id = CardId::new();

        index
            .set_card_interest(first_agent.clone(), &[card_id])
            .await;
        index
            .set_card_interest(second_agent.clone(), &[card_id])
            .await;

        let affected_agents = index.interested_agents(&[card_id]).await;
        assert_eq!(affected_agents.len(), 2);
        assert_eq!(affected_agents.get(&first_agent), Some(&vec![card_id]));
        assert_eq!(affected_agents.get(&second_agent), Some(&vec![card_id]));

        index.set_card_interest(first_agent.clone(), &[]).await;

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
        let revision = index.interests.read().await.revision;
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
        let revision = index.interests.read().await.revision;

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
        let index = CardInterestIndex::new();
        let epoch = index.close_authority();
        let (revision, _) = index.tracked_card_ids_with_revision().await;

        index
            .set_card_interest(agent("agent-1"), &[CardId::new()])
            .await;

        assert_eq!(
            index.finalize_recovery(epoch, revision).await,
            CardAuthorityRecoveryFinalize::InterestChanged
        );
        assert!(!index.authority_is_open());
    }

    #[test]
    async fn unchanged_interest_does_not_advance_recovery_revision() {
        let index = CardInterestIndex::new();
        let agent = agent("agent-1");
        let card_id = CardId::new();
        index.set_card_interest(agent.clone(), &[card_id]).await;
        let (revision, _) = index.tracked_card_ids_with_revision().await;

        index.set_card_interest(agent, &[card_id]).await;

        assert_eq!(index.tracked_card_ids_with_revision().await.0, revision);
    }
}
