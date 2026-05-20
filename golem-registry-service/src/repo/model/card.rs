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
use golem_common::model::card::Card;
use golem_service_base::repo::{Blob, RepoError, SqlDateTime};
use sqlx::FromRow;
use uuid::Uuid;

pub use golem_common::model::card::PatternGrant;

#[derive(Debug, thiserror::Error)]
pub enum CardRepoError {
    #[error("Parent card {0} does not exist")]
    ParentNotFound(Uuid),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(CardRepoError, RepoError);

#[derive(Debug, Clone, PartialEq, Eq, desert_rust::BinaryCodec)]
pub struct CardDataRecord {
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct CardRecord {
    pub card_id: Uuid,
    pub data: Blob<CardDataRecord>,
    pub created_at: SqlDateTime,
    pub expires_at: Option<SqlDateTime>,
    pub system_card: bool,
    pub polymorphic: bool,
}

impl TryFrom<CardRecord> for Card {
    type Error = RepoError;

    fn try_from(value: CardRecord) -> Result<Self, Self::Error> {
        let data = value.data.into_value();

        Ok(Card {
            card_id: value.card_id,
            parent_ids: data.parent_ids,
            lower_positive: data.lower_positive,
            lower_negative: data.lower_negative,
            upper_positive: data.upper_positive,
            upper_negative: data.upper_negative,
            created_at: value.created_at.into(),
            expires_at: value.expires_at.map(Into::into),
            system_card: value.system_card,
            polymorphic: value.polymorphic,
        })
    }
}

impl CardRecord {
    pub fn creation(
        card_id: Uuid,
        parent_ids: Vec<Uuid>,
        lower_positive: Vec<PatternGrant>,
        lower_negative: Vec<PatternGrant>,
        upper_positive: Vec<PatternGrant>,
        upper_negative: Vec<PatternGrant>,
        expires_at: Option<DateTime<Utc>>,
        system_card: bool,
        polymorphic: bool,
    ) -> Self {
        Self {
            card_id,
            data: Blob::new(CardDataRecord {
                parent_ids,
                lower_positive,
                lower_negative,
                upper_positive,
                upper_negative,
            }),
            created_at: SqlDateTime::now(),
            expires_at: expires_at.map(Into::into),
            system_card,
            polymorphic,
        }
    }
}
