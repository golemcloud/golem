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

use crate::repo::model::datetime::SqlDateTime;
use crate::repo::model::token::TokenRecord;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OAuth2WebFlowStateMetadata {
    pub redirect: Option<url::Url>,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct OAuth2WebFlowStateRecord {
    pub state_id: Uuid,
    pub metadata: Json<OAuth2WebFlowStateMetadata>,
    pub token_id: Option<Uuid>,
    pub created_at: SqlDateTime,

    #[sqlx(skip)]
    pub token: Option<TokenRecord>,
}
