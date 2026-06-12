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

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use golem_common::error_forwarding;
use golem_common::model::card::{
    Card, CardId, CardManagedBy, PermissionPattern, PolymorphicCard, PolymorphicPermissionPattern,
    StoredCard,
};
use golem_service_base::repo::{Blob, RepoError, SqlDateTime};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum CardRepoError {
    #[error("Parent card {0} does not exist")]
    ParentNotFound(Uuid),
    #[error("Card tree changed during deletion")]
    CardTreeChangedDuringDelete,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(CardRepoError, RepoError);

#[derive(Debug, Clone, PartialEq, Eq, desert_rust::BinaryCodec)]
pub struct CardDataRecord {
    pub card: StoredCard,
}

impl CardDataRecord {
    pub fn parent_ids(&self) -> &[CardId] {
        self.card.parent_ids()
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct CardRecord {
    pub card_id: Uuid,
    pub data: Blob<CardDataRecord>,
    pub created_at: SqlDateTime,
    pub expires_at: Option<SqlDateTime>,
    pub system_card: bool,
    pub managed_by: Option<Blob<CardManagedBy>>,
}

impl TryFrom<CardRecord> for Card {
    type Error = RepoError;

    fn try_from(value: CardRecord) -> Result<Self, Self::Error> {
        value.try_into_stored_card()?.into_concrete().map_err(|_| {
            RepoError::InternalError(anyhow!(
                "Cannot convert polymorphic card record to concrete card"
            ))
        })
    }
}

impl TryFrom<CardRecord> for PolymorphicCard {
    type Error = RepoError;

    fn try_from(value: CardRecord) -> Result<Self, Self::Error> {
        value
            .try_into_stored_card()?
            .into_polymorphic()
            .map_err(|_| {
                RepoError::InternalError(anyhow!(
                    "Cannot convert concrete card record to polymorphic card"
                ))
            })
    }
}

impl TryFrom<CardRecord> for StoredCard {
    type Error = RepoError;

    fn try_from(value: CardRecord) -> Result<Self, Self::Error> {
        value.try_into_stored_card()
    }
}

impl CardRecord {
    pub fn creation(
        card_id: CardId,
        parent_ids: Vec<CardId>,
        lower_positive: Vec<PermissionPattern>,
        lower_negative: Vec<PermissionPattern>,
        upper_positive: Vec<PermissionPattern>,
        upper_negative: Vec<PermissionPattern>,
        expires_at: Option<DateTime<Utc>>,
        system_card: bool,
        managed_by: Option<CardManagedBy>,
    ) -> Self {
        let card = Card {
            card_id,
            parent_ids,
            lower_positive,
            lower_negative,
            upper_positive,
            upper_negative,
            created_at: Utc::now(),
            expires_at,
            system_card,
            managed_by,
        };
        Self {
            card_id: card.card_id.0,
            data: Blob::new(CardDataRecord {
                card: StoredCard::Concrete(card.clone()),
            }),
            created_at: card.created_at.into(),
            expires_at: card.expires_at.map(Into::into),
            system_card: card.system_card,
            managed_by: card.managed_by.map(Blob::new),
        }
    }

    pub fn polymorphic_creation(
        card_id: CardId,
        parent_ids: Vec<CardId>,
        lower_positive: Vec<PolymorphicPermissionPattern>,
        lower_negative: Vec<PolymorphicPermissionPattern>,
        upper_positive: Vec<PolymorphicPermissionPattern>,
        upper_negative: Vec<PolymorphicPermissionPattern>,
        expires_at: Option<DateTime<Utc>>,
        system_card: bool,
        managed_by: Option<CardManagedBy>,
    ) -> Self {
        let card = PolymorphicCard {
            card_id,
            parent_ids,
            lower_positive,
            lower_negative,
            upper_positive,
            upper_negative,
            created_at: Utc::now(),
            expires_at,
            system_card,
        };
        Self {
            card_id: card.card_id.0,
            data: Blob::new(CardDataRecord {
                card: StoredCard::Polymorphic(card.clone()),
            }),
            created_at: card.created_at.into(),
            expires_at: card.expires_at.map(Into::into),
            system_card: card.system_card,
            managed_by: managed_by.map(Blob::new),
        }
    }

    fn try_into_stored_card(self) -> Result<StoredCard, RepoError> {
        let mut card = self.data.into_value().card;
        match &mut card {
            StoredCard::Concrete(card) => {
                card.card_id = CardId(self.card_id);
                card.created_at = self.created_at.into();
                card.expires_at = self.expires_at.map(Into::into);
                card.system_card = self.system_card;
                card.managed_by = self.managed_by.map(Blob::into_value);
            }
            StoredCard::Polymorphic(card) => {
                card.card_id = CardId(self.card_id);
                card.created_at = self.created_at.into();
                card.expires_at = self.expires_at.map(Into::into);
                card.system_card = self.system_card;
            }
        }
        Ok(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::component::ComponentName;
    use golem_common::model::component_metadata::AgentInitialPermissionTemplate;
    use golem_common::model::environment::EnvironmentName;
    use test_r::test;

    #[test]
    fn polymorphic_creation_persists_agent_initial_template_grants() {
        let template = AgentInitialPermissionTemplate::default_for(
            &EnvironmentName::try_from("prod").unwrap(),
            &ComponentName("cart-svc".to_string()),
        );

        let record = CardRecord::polymorphic_creation(
            template.card_id,
            Vec::new(),
            template.lower_positive.clone(),
            template.lower_negative.clone(),
            template.upper_positive.clone(),
            template.upper_negative.clone(),
            None,
            false,
            None,
        );
        let card = PolymorphicCard::try_from(record).unwrap();

        assert_eq!(card.card_id, template.card_id);
        assert_eq!(card.lower_positive, template.lower_positive);
        assert_eq!(card.lower_negative, template.lower_negative);
        assert_eq!(card.upper_positive, template.upper_positive);
        assert_eq!(card.upper_negative, template.upper_negative);
    }
}
