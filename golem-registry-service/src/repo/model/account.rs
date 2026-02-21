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
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use golem_common::error_forwarding;
use golem_common::model::account::{Account, AccountEmail, AccountId, AccountRevision};
use golem_common::model::auth::AccountRole;
use golem_common::model::plan::PlanId;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use strum::IntoEnumIterator;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AccountRepoError {
    #[error("Account for this email already exists")]
    AccountViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(AccountRepoError, RepoError);

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

    pub name: String,
    pub email: String,
    pub plan_id: Uuid,
    // Bitvector of roles
    pub roles: i32,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl AccountRevisionRecord {
    pub fn new(
        account_id: AccountId,
        name: String,
        email: String,
        plan_id: PlanId,
        roles: Vec<AccountRole>,
        actor: AccountId,
    ) -> Self {
        Self {
            account_id: account_id.0,
            revision_id: AccountRevision::INITIAL.into(),
            name,
            email,
            plan_id: plan_id.0,
            roles: roles_to_bit_vector(&roles),
            audit: DeletableRevisionAuditFields::new(actor.0),
        }
    }

    pub fn from_model(value: Account, audit: DeletableRevisionAuditFields) -> Self {
        Self {
            account_id: value.id.0,
            revision_id: value.revision.into(),
            name: value.name,
            email: value.email.0,
            plan_id: value.plan_id.0,
            roles: roles_to_bit_vector(&value.roles),
            audit,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountExtRevisionRecord {
    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: AccountRevisionRecord,
}

impl TryFrom<AccountExtRevisionRecord> for Account {
    type Error = AccountRepoError;
    fn try_from(value: AccountExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: AccountId(value.revision.account_id),
            revision: value.revision.revision_id.try_into()?,
            name: value.revision.name,
            email: AccountEmail(value.revision.email),
            plan_id: PlanId(value.revision.plan_id),
            roles: roles_from_bit_vector(value.revision.roles),
        })
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountBySecretRecord {
    pub token_id: Uuid,
    pub token_expires_at: SqlDateTime,

    #[sqlx(flatten)]
    pub value: AccountExtRevisionRecord,
}

// To allow abstracting over postgres and sqlite roles are stored as a bit vector.
fn role_bit(role: &AccountRole) -> i32 {
    match role {
        AccountRole::Admin => 1,
        AccountRole::MarketingAdmin => 1 << 1,
    }
}

fn roles_to_bit_vector(roles: &[AccountRole]) -> i32 {
    let mut result: i32 = 0;
    for role in roles {
        result |= role_bit(role)
    }
    result
}

fn roles_from_bit_vector(value: i32) -> Vec<AccountRole> {
    let mut result: Vec<AccountRole> = Vec::new();
    for role in AccountRole::iter() {
        let has_role = (value & role_bit(&role)) != 0;
        if has_role {
            result.push(role);
        }
    }
    result
}
