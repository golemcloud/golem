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

//! The card service of a never-live owner. It refuses the registry mutations and lets the
//! card reads and the local cache updates through.

use super::*;

pub(crate) struct RefusingCardService {
    pub(crate) inner: Arc<dyn CardService>,
}

#[async_trait]
impl CardService for RefusingCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        self.inner.record_revoked_cards(card_ids).await
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        self.inner.check_cards(card_ids).await
    }

    async fn create_runtime_card(
        &self,
        _card: StoredCard,
        _provenance: CardManagedByRuntimeDerived,
    ) -> Result<RuntimeCardCreation, WorkerExecutorError> {
        Err(crate::durable_host::never_live::refusal(
            "card::create_runtime_card",
        ))
    }

    async fn revoke_card(&self, _card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        Err(crate::durable_host::never_live::refusal(
            "card::revoke_card",
        ))
    }

    async fn live_ancestor_ids_including_self(
        &self,
        card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        self.inner.live_ancestor_ids_including_self(card).await
    }

    async fn invalidate_all(&self) {
        self.inner.invalidate_all().await
    }
}
