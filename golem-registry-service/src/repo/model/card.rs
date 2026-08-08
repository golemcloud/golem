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
use chrono::{DateTime, Timelike, Utc};
use golem_common::model::card::{
    Card, CardId, CardManagedBy, CardManagedByRuntimeDerived, PermissionPattern, PolymorphicCard,
    StoredCard,
};
use golem_common::{IntoAnyhow, error_forwarding};
use golem_service_base::repo::{Blob, RepoError, SqlDateTime};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum CardRepoError {
    #[error("Parent card {0} does not exist")]
    ParentNotFound(Uuid),
    #[error("Card {0} already exists")]
    CardAlreadyExists(Uuid),
    #[error("Runtime card {0} has been revoked")]
    RuntimeCardRevoked(Uuid),
    #[error("Card tree changed during deletion")]
    CardTreeChangedDuringDelete,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(CardRepoError);

impl From<RepoError> for CardRepoError {
    fn from(value: RepoError) -> Self {
        let is_postgres_deadlock = match &value {
            RepoError::InternalError(error) => error.chain().any(|cause| {
                cause
                    .downcast_ref::<sqlx::Error>()
                    .and_then(sqlx::Error::as_database_error)
                    .and_then(|error| error.code())
                    .is_some_and(|code| code == "40P01")
            }),
            RepoError::UniqueViolation(_) | RepoError::ForeignKeyViolation(_) => false,
        };

        if is_postgres_deadlock {
            Self::ConcurrentModification
        } else {
            Self::InternalError(value.into_anyhow())
        }
    }
}

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
    pub fn runtime_creation(mut card: StoredCard, provenance: CardManagedByRuntimeDerived) -> Self {
        let managed_by = CardManagedBy::RuntimeDerived(provenance);
        match &mut card {
            StoredCard::Concrete(card) => {
                card.parent_ids.sort_unstable();
                card.parent_ids.dedup();
                card.created_at = sql_timestamp_precision(card.created_at);
                card.expires_at = card.expires_at.map(sql_timestamp_precision);
                card.managed_by = Some(managed_by.clone());
            }
            StoredCard::Polymorphic(card) => {
                card.parent_ids.sort_unstable();
                card.parent_ids.dedup();
                card.created_at = sql_timestamp_precision(card.created_at);
                card.expires_at = card.expires_at.map(sql_timestamp_precision);
            }
        }

        Self {
            card_id: card.card_id().0,
            created_at: card.created_at().into(),
            expires_at: card.expires_at().map(Into::into),
            system_card: card.system_card(),
            data: Blob::new(CardDataRecord { card }),
            managed_by: Some(Blob::new(managed_by)),
        }
    }

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
        mut card: PolymorphicCard,
        managed_by: Option<CardManagedBy>,
    ) -> Self {
        card.parent_ids.sort_unstable();
        card.parent_ids.dedup();
        card.created_at = sql_timestamp_precision(card.created_at);
        card.expires_at = card.expires_at.map(sql_timestamp_precision);
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

fn sql_timestamp_precision(timestamp: DateTime<Utc>) -> DateTime<Utc> {
    timestamp
        .with_nanosecond(timestamp.nanosecond() / 1_000 * 1_000)
        .expect("truncating nanoseconds preserves a valid timestamp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::card::recipient::RecipientPattern;
    use golem_common::model::card::{
        CardManagedByRuntimeDerived, default_agent_initial_permission_grants,
    };
    use golem_common::model::{AgentId, IdempotencyKey, OplogIndex};
    use test_r::test;

    fn runtime_derived_provenance() -> CardManagedByRuntimeDerived {
        CardManagedByRuntimeDerived {
            environment_id: golem_common::model::environment::EnvironmentId(Uuid::new_v4()),
            agent_id: AgentId {
                component_id: golem_common::model::component::ComponentId(Uuid::new_v4()),
                agent_id: "source-agent".to_string(),
            },
            invocation_key: IdempotencyKey::new("source-invocation".to_string()),
            oplog_index: OplogIndex::from_u64(42),
        }
    }

    #[test]
    fn concrete_creation_persists_runtime_derived_provenance() {
        let provenance = runtime_derived_provenance();
        let created_at = DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap();
        let expires_at = DateTime::from_timestamp(1_800_000_000, 987_654_321).unwrap();
        let record = CardRecord::runtime_creation(
            StoredCard::Concrete(Card {
                card_id: CardId::new(),
                parent_ids: Vec::new(),
                lower_positive: Vec::new(),
                lower_negative: Vec::new(),
                upper_positive: Vec::new(),
                upper_negative: Vec::new(),
                created_at,
                expires_at: Some(expires_at),
                system_card: false,
                managed_by: None,
            }),
            provenance.clone(),
        );
        let expected_provenance = CardManagedBy::RuntimeDerived(provenance);

        assert_eq!(
            record.managed_by.as_ref().map(|value| value.value()),
            Some(&expected_provenance)
        );
        let card = Card::try_from(record).unwrap();
        assert_eq!(card.created_at, sql_timestamp_precision(created_at));
        assert_eq!(card.expires_at, Some(sql_timestamp_precision(expires_at)));
        assert_eq!(card.managed_by, Some(expected_provenance));
    }

    #[test]
    fn polymorphic_creation_persists_agent_initial_card_grants() {
        let created_at = DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap();
        let initial_card = PolymorphicCard {
            card_id: CardId::new(),
            parent_ids: Vec::new(),
            lower_positive: default_agent_initial_permission_grants(RecipientPattern::Any),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at,
            expires_at: None,
            system_card: false,
        };

        let record = CardRecord::polymorphic_creation(initial_card.clone(), None);
        let card = PolymorphicCard::try_from(record).unwrap();

        assert_eq!(card.card_id, initial_card.card_id);
        assert_eq!(card.lower_positive, initial_card.lower_positive);
        assert_eq!(card.lower_negative, initial_card.lower_negative);
        assert_eq!(card.upper_positive, initial_card.upper_positive);
        assert_eq!(card.upper_negative, initial_card.upper_negative);
        assert_eq!(card.created_at, sql_timestamp_precision(created_at));
    }

    #[test]
    fn polymorphic_creation_persists_runtime_derived_provenance() {
        let provenance = runtime_derived_provenance();
        let created_at = DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap();
        let record = CardRecord::runtime_creation(
            StoredCard::Polymorphic(PolymorphicCard {
                card_id: CardId::new(),
                parent_ids: Vec::new(),
                lower_positive: Vec::new(),
                lower_negative: Vec::new(),
                upper_positive: Vec::new(),
                upper_negative: Vec::new(),
                created_at,
                expires_at: None,
                system_card: false,
            }),
            provenance.clone(),
        );
        let expected_provenance = CardManagedBy::RuntimeDerived(provenance);

        assert_eq!(
            record.managed_by.as_ref().map(|value| value.value()),
            Some(&expected_provenance)
        );
        let card = PolymorphicCard::try_from(record).unwrap();
        assert_eq!(card.created_at, sql_timestamp_precision(created_at));
    }
}
