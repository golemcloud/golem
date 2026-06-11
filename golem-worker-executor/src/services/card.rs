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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLiveness {
    Live,
    Revoked { newly_detected: bool },
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
    fn record_revoked_cards(&self, card_ids: &[CardId]);

    async fn check_card(&self, card_id: CardId) -> Result<CardLiveness, WorkerExecutorError>;
}

pub struct CardServiceDefault {
    registry_service: Arc<dyn RegistryService>,
    revoked_cards: RwLock<HashSet<CardId>>,
}

pub struct NoopCardService;

#[async_trait]
impl CardService for NoopCardService {
    fn record_revoked_cards(&self, _card_ids: &[CardId]) {}

    async fn check_card(&self, _card_id: CardId) -> Result<CardLiveness, WorkerExecutorError> {
        Ok(CardLiveness::Live)
    }
}

impl CardServiceDefault {
    pub fn new(registry_service: Arc<dyn RegistryService>) -> Self {
        Self {
            registry_service,
            revoked_cards: RwLock::new(HashSet::new()),
        }
    }

    fn cache_revoked_cards(&self, card_ids: &[CardId]) {
        let mut revoked_cards = self.revoked_cards.write().unwrap();
        revoked_cards.extend(card_ids.iter().copied());
    }
}

#[async_trait]
impl CardService for CardServiceDefault {
    fn record_revoked_cards(&self, card_ids: &[CardId]) {
        self.cache_revoked_cards(card_ids);
    }

    async fn check_card(&self, card_id: CardId) -> Result<CardLiveness, WorkerExecutorError> {
        if self.revoked_cards.read().unwrap().contains(&card_id) {
            return Ok(CardLiveness::Revoked {
                newly_detected: false,
            });
        }

        let existing = self
            .registry_service
            .batch_get_existing_cards(vec![card_id])
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "Failed checking card existence: {}",
                    err.to_safe_string()
                ))
            })?;

        if existing.contains(&card_id) {
            Ok(CardLiveness::Live)
        } else {
            self.cache_revoked_cards(&[card_id]);
            Ok(CardLiveness::Revoked {
                newly_detected: true,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn noop_card_service_treats_cards_as_live() {
        let service = NoopCardService;
        let revoked = CardId::new();

        assert!(
            futures::executor::block_on(service.check_card(revoked))
                .unwrap()
                .is_live()
        );
    }
}
