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
use golem_common::model::account::AccountId;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::environment::{EnvironmentId, EnvironmentRevision};
use golem_common::model::environment_share::{EnvironmentShare, EnvironmentShareId};
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use strum::IntoEnumIterator;
use uuid::Uuid;
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentShareRepoError {
    #[error("There is already a share for this account in this environment")]
    ShareViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(EnvironmentShareRepoError, RepoError);

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct EnvironmentShareRecord {
    pub environment_id: Uuid,
    pub environment_share_id: Uuid,
    pub grantee_account_id: Uuid,

    #[sqlx(flatten)]
    pub audit: AuditFields,

    pub current_revision_id: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct EnvironmentShareRevisionRecord {
    pub environment_share_id: Uuid,
    pub revision_id: i64,

    // Bitvector of roles
    pub roles: i32,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl EnvironmentShareRevisionRecord {
    pub fn creation(id: EnvironmentShareId, roles: Vec<EnvironmentRole>, actor: AccountId) -> Self {
        Self {
            environment_share_id: id.0,
            revision_id: EnvironmentRevision::INITIAL.into(),
            roles: roles_to_bit_vector(&roles),
            audit: DeletableRevisionAuditFields::new(actor.0),
        }
    }

    pub fn from_model(value: EnvironmentShare, actor: AccountId) -> Self {
        Self {
            environment_share_id: value.id.0,
            revision_id: value.revision.into(),
            roles: roles_to_bit_vector(&value.roles),
            audit: DeletableRevisionAuditFields::new(actor.0),
        }
    }

    pub fn ensure_first(self) -> Self {
        Self {
            revision_id: 0,
            audit: self.audit.ensure_new(),
            ..self
        }
    }

    pub fn ensure_new(self, current_revision_id: i64) -> Self {
        Self {
            revision_id: current_revision_id + 1,
            audit: self.audit.ensure_new(),
            ..self
        }
    }

    pub fn ensure_deleted(self, current_revision_id: i64) -> Self {
        Self {
            revision_id: current_revision_id + 1,
            audit: self.audit.ensure_deletion(),
            ..self
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct EnvironmentShareExtRevisionRecord {
    pub environment_id: Uuid,
    pub grantee_account_id: Uuid,
    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: EnvironmentShareRevisionRecord,
}

impl TryFrom<EnvironmentShareExtRevisionRecord> for EnvironmentShare {
    type Error = EnvironmentShareRepoError;
    fn try_from(value: EnvironmentShareExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            environment_id: EnvironmentId(value.environment_id),
            id: EnvironmentShareId(value.revision.environment_share_id),
            revision: value.revision.revision_id.into(),
            grantee_account_id: AccountId(value.grantee_account_id),
            roles: environment_roles_from_bit_vector(value.revision.roles),
        })
    }
}

// To allow abstracting over postgres and sqlite roles are stored as a bit vector.
fn role_bit(role: &EnvironmentRole) -> i32 {
    match role {
        EnvironmentRole::Admin => 1,
        EnvironmentRole::Deployer => 1 << 1,
        EnvironmentRole::Viewer => 1 << 2,
    }
}

fn roles_to_bit_vector(roles: &[EnvironmentRole]) -> i32 {
    let mut result: i32 = 0;
    for role in roles {
        result |= role_bit(role)
    }
    result
}

pub fn environment_roles_from_bit_vector(value: i32) -> Vec<EnvironmentRole> {
    let mut result: Vec<EnvironmentRole> = Vec::new();
    for role in EnvironmentRole::iter() {
        let has_role = (value & role_bit(&role)) != 0;
        if has_role {
            result.push(role);
        }
    }
    result
}
