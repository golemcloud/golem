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
use anyhow::anyhow;
use golem_common::error_forwarders;
use golem_common::model::PlanId;
use golem_common::model::account::{Account, AccountId, AccountRevision};
use golem_common::model::auth::AccountRole;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;
use golem_common::model::environment_share::{EnvironmentShare, EnvironmentShareId, EnvironmentShareRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::auth::EnvironmentRole;
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentShareRepoError {
    #[error("There is already a share for this account in this environment")]
    ShareViolatesUniqueness,
    #[error("Revision already exists: {revision_id}")]
    RevisionAlreadyExists { revision_id: i64 },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarders!(EnvironmentShareRepoError, RepoError);

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

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,

    #[sqlx(skip)]
    pub roles: Vec<EnvironmentShareRoleRecord>,
}

impl EnvironmentShareRevisionRecord {
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
            roles: value
                .revision
                .roles
                .into_iter()
                .map(EnvironmentRole::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct EnvironmentShareRoleRecord {
    pub environment_share_id: Uuid,
    pub revision_id: i64,
    pub role: i32,
}

impl EnvironmentShareRoleRecord {
    pub fn ensure_environment_share(self, environment_share_id: Uuid, revision_id: i64) -> Self {
        Self {
            environment_share_id,
            revision_id,
            ..self
        }
    }

    pub fn from_model(environment_share_id: EnvironmentShareId, revision: EnvironmentShareRevision, value: EnvironmentRole) -> Self {
        Self {
            environment_share_id: environment_share_id.0,
            revision_id: revision.into(),
            role: value as i32,
        }
    }
}

impl TryFrom<EnvironmentShareRoleRecord> for EnvironmentRole {
    type Error = EnvironmentShareRepoError;
    fn try_from(value: EnvironmentShareRoleRecord) -> Result<Self, Self::Error> {
        Ok(EnvironmentRole::try_from(value.role).map_err(|e| anyhow!(e))?)
    }
}
