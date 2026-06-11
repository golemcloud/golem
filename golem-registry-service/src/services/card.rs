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
use golem_common::model::card::{CardId, CardManagedBy};
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
    ) -> Result<CardId, CardRepoError> {
        let card_id = CardId::new();
        self.card_repo
            .create(CardRecord::creation(
                card_id,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
                false,
                Some(CardManagedBy::AgentInitial {
                    component_id,
                    component_revision,
                    agent_type,
                }),
            ))
            .await?;

        Ok(card_id)
    }
}
