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

use crate::repo::card::CardRepo;
use crate::repo::model::card::{CardRecord, CardRepoError};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::card::{CardId, CardManagedBy, CardManagedByAgentInitial, PolymorphicCard, StoredCard};
use golem_common::model::component::{ComponentId, ComponentRevision};
use std::sync::Arc;

pub struct CardService {
    card_repo: Arc<dyn CardRepo>,
}

impl CardService {
    pub fn new(card_repo: Arc<dyn CardRepo>) -> Self {
        Self { card_repo }
    }

    pub async fn create_agent_initial_card(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type: AgentTypeName,
        card: &PolymorphicCard,
    ) -> Result<CardId, CardRepoError> {
        let card_id = card.card_id;
        self.card_repo
            .create(CardRecord::polymorphic_creation(
                card_id,
                card.parent_ids.clone(),
                card.lower_positive.clone(),
                card.lower_negative.clone(),
                card.upper_positive.clone(),
                card.upper_negative.clone(),
                card.expires_at,
                card.system_card,
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision,
                    agent_type,
                })),
            ))
            .await?;

        Ok(card_id)
    }

    pub async fn existing(&self, card_ids: Vec<CardId>) -> Result<Vec<CardId>, CardRepoError> {
        self.card_repo.existing(card_ids).await.map_err(Into::into)
    }

    pub async fn get_cards(&self, card_ids: Vec<CardId>) -> Result<Vec<StoredCard>, CardRepoError> {
        let mut result = Vec::new();
        for card_id in card_ids {
            if let Some(record) = self.card_repo.get(card_id).await?
                && let Ok(card) = StoredCard::try_from(record)
            {
                result.push(card);
            }
        }
        Ok(result)
    }
}
