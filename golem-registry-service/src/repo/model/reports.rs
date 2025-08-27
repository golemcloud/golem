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

use super::datetime::SqlDateTime;
use crate::repo::model::audit::AuditFields;
use golem_common::model::account::AccountId;
use golem_common::model::reports::{AccountCounts, AccountSummary};
use sqlx::FromRow;
use std::fmt::Debug;
use uuid::Uuid;

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRecord {
    pub account_id: Uuid,

    pub email: String,

    #[sqlx(flatten)]
    pub audit: AuditFields,

    pub current_revision_id: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountSummaryRecord {
    pub account_id: Uuid,
    pub email: String,
    pub created_at: SqlDateTime,
    pub name: String,
    pub components_count: i64,
    pub workers_count: i64,
}

impl From<AccountSummaryRecord> for AccountSummary {
    fn from(value: AccountSummaryRecord) -> Self {
        Self {
            id: AccountId(value.account_id),
            name: value.name,
            email: value.email,
            components_count: value.components_count,
            workers_count: value.workers_count,
            created_at: value.created_at.into(),
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountCountsRecord {
    pub total_accounts: i64,
    pub total_active_accounts: i64,
    pub total_deleted_accounts: i64,
}

impl From<AccountCountsRecord> for AccountCounts {
    fn from(value: AccountCountsRecord) -> Self {
        Self {
            total_accounts: value.total_accounts,
            total_active_accounts: value.total_active_accounts,
            total_deleted_accounts: value.total_deleted_accounts,
        }
    }
}
