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
use tokio::sync::RwLock;

#[derive(Default)]
pub struct CardInterestIndex {
    interests: RwLock<HashMap<CardId, HashSet<OwnedAgentId>>>,
}

impl CardInterestIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set_card_interest(&self, agent_id: OwnedAgentId, card_ids: &[CardId]) {
        let mut interests = self.interests.write().await;
        Self::remove_agent_from_all_cards(&mut interests, &agent_id);

        for card_id in card_ids {
            interests
                .entry(*card_id)
                .or_default()
                .insert(agent_id.clone());
        }
    }

    pub async fn interested_agents(
        &self,
        card_ids: &[CardId],
    ) -> HashMap<OwnedAgentId, Vec<CardId>> {
        let interests = self.interests.read().await;
        let mut affected_agent_cards = HashMap::<OwnedAgentId, Vec<CardId>>::new();
        for card_id in card_ids {
            if let Some(agents) = interests.get(card_id) {
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
}
