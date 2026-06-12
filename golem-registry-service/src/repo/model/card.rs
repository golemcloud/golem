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

use chrono::{DateTime, Utc};
use golem_common::error_forwarding;
use golem_common::model::card::{
    Card, CardId, CardManagedBy, PermissionPattern, PolymorphicCard, PolymorphicPermissionPattern,
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
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PermissionPattern>,
    pub lower_negative: Vec<PermissionPattern>,
    pub upper_positive: Vec<PermissionPattern>,
    pub upper_negative: Vec<PermissionPattern>,
    pub polymorphic_lower_positive: Vec<PolymorphicPermissionPattern>,
    pub polymorphic_lower_negative: Vec<PolymorphicPermissionPattern>,
    pub polymorphic_upper_positive: Vec<PolymorphicPermissionPattern>,
    pub polymorphic_upper_negative: Vec<PolymorphicPermissionPattern>,
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
        let data = value.data.into_value();

        Ok(Card {
            card_id: CardId(value.card_id),
            parent_ids: data.parent_ids.into_iter().map(CardId).collect(),
            lower_positive: data.lower_positive,
            lower_negative: data.lower_negative,
            upper_positive: data.upper_positive,
            upper_negative: data.upper_negative,
            created_at: value.created_at.into(),
            expires_at: value.expires_at.map(Into::into),
            system_card: value.system_card,
            managed_by: value.managed_by.map(Blob::into_value),
        })
    }
}

impl TryFrom<CardRecord> for PolymorphicCard {
    type Error = RepoError;

    fn try_from(value: CardRecord) -> Result<Self, Self::Error> {
        let data = value.data.into_value();

        Ok(PolymorphicCard {
            card_id: CardId(value.card_id),
            parent_ids: data.parent_ids.into_iter().map(CardId).collect(),
            lower_positive: data.polymorphic_lower_positive,
            lower_negative: data.polymorphic_lower_negative,
            upper_positive: data.polymorphic_upper_positive,
            upper_negative: data.polymorphic_upper_negative,
            created_at: value.created_at.into(),
            expires_at: value.expires_at.map(Into::into),
            system_card: value.system_card,
        })
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
        Self {
            card_id: card_id.0,
            data: Blob::new(CardDataRecord {
                parent_ids: parent_ids.into_iter().map(|id| id.0).collect(),
                lower_positive,
                lower_negative,
                upper_positive,
                upper_negative,
                polymorphic_lower_positive: Vec::new(),
                polymorphic_lower_negative: Vec::new(),
                polymorphic_upper_positive: Vec::new(),
                polymorphic_upper_negative: Vec::new(),
            }),
            created_at: SqlDateTime::now(),
            expires_at: expires_at.map(Into::into),
            system_card,
            managed_by: managed_by.map(Blob::new),
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
        Self {
            card_id: card_id.0,
            data: Blob::new(CardDataRecord {
                parent_ids: parent_ids.into_iter().map(|id| id.0).collect(),
                lower_positive: Vec::new(),
                lower_negative: Vec::new(),
                upper_positive: Vec::new(),
                upper_negative: Vec::new(),
                polymorphic_lower_positive: lower_positive,
                polymorphic_lower_negative: lower_negative,
                polymorphic_upper_positive: upper_positive,
                polymorphic_upper_negative: upper_negative,
            }),
            created_at: SqlDateTime::now(),
            expires_at: expires_at.map(Into::into),
            system_card,
            managed_by: managed_by.map(Blob::new),
        }
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
