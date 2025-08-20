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
use anyhow::anyhow;
use golem_common::model::PlanId;
use golem_common::model::account::{Account, AccountId, AccountRevision};
use golem_common::model::auth::AccountRole;
use golem_common::{error_forwarders, into_internal_error};
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AccountRepoError {
    #[error("Account for this email already exists")]
    AccountViolatesUniqueness,
    #[error("Version already exists: {version}")]
    VersionAlreadyExists { version: i64 },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

into_internal_error!(AccountRepoError);

error_forwarders!(AccountRepoError, RepoError);

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRecord {
    pub account_id: Uuid,
    pub email: String,

    #[sqlx(flatten)]
    pub audit: AuditFields,

    pub current_revision_id: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRevisionRecord {
    pub account_id: Uuid,
    pub revision_id: i64,
    pub email: String,
    pub name: String,
    pub plan_id: Uuid,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    #[sqlx(skip)]
    pub roles: Vec<AccountRoleRecord>,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct JoinedAccountRecord {
    pub account_id: Uuid,
    pub current_revision_id: i64,
    pub email: String,
    pub name: String,
    pub plan_id: Uuid,

    #[sqlx(flatten)]
    pub audit: AuditFields,
    #[sqlx(skip)]
    pub roles: Vec<AccountRoleRecord>,
}

impl TryFrom<JoinedAccountRecord> for Account {
    type Error = AccountRepoError;
    fn try_from(value: JoinedAccountRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: AccountId(value.account_id),
            revision: value.current_revision_id.into(),
            name: value.name,
            email: value.email,
            plan_id: PlanId(value.plan_id),
            roles: value
                .roles
                .into_iter()
                .map(AccountRole::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRoleRecord {
    pub account_id: Uuid,
    pub revision_id: i64,
    pub role: i32,
}

impl AccountRoleRecord {
    pub fn from_model(account: AccountId, revision: AccountRevision, value: AccountRole) -> Self {
        Self {
            account_id: account.0,
            revision_id: revision.into(),
            role: value as i32,
        }
    }
}

impl TryFrom<AccountRoleRecord> for AccountRole {
    type Error = AccountRepoError;
    fn try_from(value: AccountRoleRecord) -> Result<Self, Self::Error> {
        Ok(AccountRole::try_from(value.role).map_err(|e| anyhow!(e))?)
    }
}
