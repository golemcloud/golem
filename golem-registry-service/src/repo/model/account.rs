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

use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use crate::repo::model::datetime::SqlDateTime;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRecord {
    pub account_id: Uuid,
    pub email: String,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub name: String,
    pub plan_id: Uuid,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRevisionRecord {
    pub account_id: Uuid,
    pub email: String,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub name: String,
    pub plan_id: Uuid,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountUsageStatsRecord {
    pub account_id: Uuid,
    pub usage_type: i32,
    pub usage_key: String,
    pub value: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct TokenRecord {
    pub token_id: Uuid,
    pub secret: Uuid,
    pub account_id: Uuid,
    pub created_at: SqlDateTime,
    pub expires_at: SqlDateTime,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct OAuth2TokenRecord {
    pub provider: String,
    pub external_id: String,
    pub token_id: Option<Uuid>,
    pub account_id: Uuid,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct OAuth2WebFlowStateRecord {
    pub oauth2_state: String,
    pub metadata: Vec<u8>,
    pub token_id: Uuid,
    pub created_at: SqlDateTime,
}
