// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::model::login::{OAuth2WebflowState, OAuth2WebflowStateMetadata};
use crate::repo::model::datetime::SqlDateTime;
use crate::repo::model::token::TokenRecord;
use sqlx::FromRow;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct OAuth2WebFlowStateRecord {
    pub state_id: Uuid,
    pub metadata: Json<OAuth2WebflowStateMetadata>,
    pub token_id: Option<Uuid>,
    pub created_at: SqlDateTime,

    #[sqlx(skip)]
    pub token: Option<TokenRecord>,
}

impl From<OAuth2WebFlowStateRecord> for OAuth2WebflowState {
    fn from(value: OAuth2WebFlowStateRecord) -> Self {
        Self {
            metadata: value.metadata.0,
            token: value.token.map(|t| t.into()),
        }
    }
}
