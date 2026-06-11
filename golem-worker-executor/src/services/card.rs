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
use golem_common::model::card::CardId;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

#[async_trait]
pub trait CardService: Send + Sync {
    fn record_revoked_cards(&self, card_ids: &[CardId]);

    fn is_card_revoked(&self, card_id: CardId) -> bool;

    async fn existing_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<Vec<CardId>, WorkerExecutorError>;
}

pub struct CardServiceDefault {
    registry_service: Option<Arc<dyn RegistryService>>,
    revoked_cards: RwLock<HashSet<CardId>>,
}

pub struct NoopCardService;

#[async_trait]
impl CardService for NoopCardService {
    fn record_revoked_cards(&self, _card_ids: &[CardId]) {}

    fn is_card_revoked(&self, _card_id: CardId) -> bool {
        false
    }

    async fn existing_cards(&self, card_ids: Vec<CardId>) -> Result<Vec<CardId>, WorkerExecutorError> {
        Ok(card_ids)
    }
}

impl CardServiceDefault {
    pub fn new(registry_service: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service: Some(registry_service),
            revoked_cards: RwLock::new(HashSet::new()),
        }
    }

    #[cfg(test)]
    fn for_tests() -> Self {
        Self {
            registry_service: None,
            revoked_cards: RwLock::new(HashSet::new()),
        }
    }
}

#[async_trait]
impl CardService for CardServiceDefault {
    fn record_revoked_cards(&self, card_ids: &[CardId]) {
        let mut revoked_cards = self.revoked_cards.write().unwrap();
        revoked_cards.extend(card_ids.iter().copied());
    }

    fn is_card_revoked(&self, card_id: CardId) -> bool {
        self.revoked_cards.read().unwrap().contains(&card_id)
    }

    async fn existing_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        let Some(registry_service) = &self.registry_service else {
            return Ok(card_ids);
        };
        let existing = registry_service
            .batch_get_existing_cards(card_ids.clone())
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "Failed checking card existence: {}",
                    err.to_safe_string()
                ))
            })?;
        let missing = card_ids
            .into_iter()
            .filter(|card_id| !existing.contains(card_id))
            .collect::<Vec<_>>();
        self.record_revoked_cards(&missing);
        Ok(existing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn records_revoked_card_ids() {
        let service = CardServiceDefault::for_tests();
        let revoked = CardId::new();
        let live = CardId::new();

        assert!(!service.is_card_revoked(revoked));
        service.record_revoked_cards(&[revoked]);

        assert!(service.is_card_revoked(revoked));
        assert!(!service.is_card_revoked(live));
    }
}
