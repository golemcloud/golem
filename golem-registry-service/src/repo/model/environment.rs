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
use super::environment_share::environment_roles_from_bit_vector;
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
use golem_common::error_forwarding;
use golem_common::model::account::AccountSummary;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::application::ApplicationSummary;
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::diff::Hashable;
use golem_common::model::diff::{self};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentCurrentDeploymentView, EnvironmentId,
    EnvironmentName, EnvironmentRevision, EnvironmentSummary, EnvironmentWithDetails,
};
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentRepoError {
    #[error("Environment violates unique index")]
    EnvironmentViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(EnvironmentRepoError, RepoError);

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentExtRecord {
    pub environment_id: Uuid,
    pub name: String,
    pub application_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,

    pub owner_account_id: Uuid,
    pub environment_roles_from_shares: i32,

    pub current_deployment_revision: Option<i64>,
    pub current_deployment_deployment_revision: Option<i64>,
    pub current_deployment_deployment_version: Option<String>,
    pub current_deployment_deployment_hash: Option<SqlBlake3Hash>,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    pub name: String,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
}

impl EnvironmentRevisionRecord {
    pub fn creation(environment: EnvironmentCreation, actor: AccountId) -> Self {
        Self {
            environment_id: EnvironmentId::new().0,
            revision_id: EnvironmentRevision::INITIAL.into(),
            name: environment.name.0,
            hash: SqlBlake3Hash::empty(),
            compatibility_check: environment.compatibility_check,
            version_check: environment.version_check,
            security_overrides: environment.security_overrides,
            audit: DeletableRevisionAuditFields::new(actor.0),
        }
    }

    pub fn from_model(environment: Environment, audit: DeletableRevisionAuditFields) -> Self {
        Self {
            environment_id: environment.id.0,
            revision_id: environment.revision.into(),
            name: environment.name.0,
            hash: SqlBlake3Hash::empty(),
            compatibility_check: environment.compatibility_check,
            version_check: environment.version_check,
            security_overrides: environment.security_overrides,
            audit,
        }
    }

    pub fn to_diffable(&self) -> diff::Environment {
        diff::Environment {
            compatibility_check: self.compatibility_check,
            version_check: self.version_check,
            security_overrides: self.security_overrides,
        }
    }

    pub fn update_hash(&mut self) {
        self.hash = self.to_diffable().hash().into_blake3().into()
    }

    pub fn with_updated_hash(mut self) -> Self {
        self.update_hash();
        self
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentExtRevisionRecord {
    pub application_id: Uuid,

    #[sqlx(flatten)]
    pub revision: EnvironmentRevisionRecord,

    pub owner_account_id: Uuid,
    pub environment_roles_from_shares: i32,

    pub current_deployment_revision: Option<i64>,
    pub current_deployment_deployment_revision: Option<i64>,
    pub current_deployment_deployment_version: Option<String>,
    pub current_deployment_deployment_hash: Option<SqlBlake3Hash>,
}

impl TryFrom<EnvironmentExtRevisionRecord> for Environment {
    type Error = EnvironmentRepoError;
    fn try_from(value: EnvironmentExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: EnvironmentId(value.revision.environment_id),
            revision: value.revision.revision_id.try_into()?,
            application_id: ApplicationId(value.application_id),
            name: EnvironmentName(value.revision.name),
            compatibility_check: value.revision.compatibility_check,
            version_check: value.revision.version_check,
            security_overrides: value.revision.security_overrides,

            owner_account_id: AccountId(value.owner_account_id),
            roles_from_active_shares: environment_roles_from_bit_vector(
                value.environment_roles_from_shares,
            ),

            current_deployment: match (
                value.current_deployment_revision,
                value.current_deployment_deployment_revision,
                value.current_deployment_deployment_version,
                value.current_deployment_deployment_hash,
            ) {
                (
                    Some(revision),
                    Some(deployment_revision),
                    Some(deployment_version),
                    Some(deployment_hash),
                ) => Some(EnvironmentCurrentDeploymentView {
                    revision: revision.try_into()?,
                    deployment_revision: deployment_revision.try_into()?,
                    deployment_version: deployment_version.into(),
                    deployment_hash: deployment_hash.into_blake3_hash().into(),
                }),
                _ => None,
            },
        })
    }
}

// Special record for listing environments. Parent context is mandatory while the environment itself and all children are optional
// Simplify when https://github.com/launchbadge/sqlx/issues/2934 is fixed
#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct OptionalEnvironmentExtRevisionRecord {
    pub application_id: Uuid,
    pub environment_id: Option<Uuid>,
    pub revision_id: Option<i64>,
    pub name: Option<String>,
    pub hash: Option<SqlBlake3Hash>,
    pub created_at: Option<SqlDateTime>,
    pub created_by: Option<Uuid>,
    pub deleted: Option<bool>,
    pub compatibility_check: Option<bool>,
    pub version_check: Option<bool>,
    pub security_overrides: Option<bool>,

    pub owner_account_id: Uuid,
    pub environment_roles_from_shares: i32,

    pub current_deployment_revision: Option<i64>,
    pub current_deployment_deployment_revision: Option<i64>,
    pub current_deployment_deployment_version: Option<String>,
    pub current_deployment_deployment_hash: Option<SqlBlake3Hash>,
}

impl OptionalEnvironmentExtRevisionRecord {
    pub fn owner_account_id(&self) -> AccountId {
        AccountId(self.owner_account_id)
    }

    pub fn environment_roles_from_shares(&self) -> BTreeSet<EnvironmentRole> {
        environment_roles_from_bit_vector(self.environment_roles_from_shares)
    }

    pub fn into_revision_record(self) -> Option<EnvironmentExtRevisionRecord> {
        let environment_id = self.environment_id?;
        let revision_id = self.revision_id?;
        let name = self.name?;
        let hash = self.hash?;
        let created_at = self.created_at?;
        let created_by = self.created_by?;
        let deleted = self.deleted?;
        let compatibility_check = self.compatibility_check?;
        let version_check = self.version_check?;
        let security_overrides = self.security_overrides?;
        Some(EnvironmentExtRevisionRecord {
            application_id: self.application_id,
            revision: EnvironmentRevisionRecord {
                environment_id,
                revision_id,
                name,
                hash,
                audit: DeletableRevisionAuditFields {
                    created_at,
                    created_by,
                    deleted,
                },
                compatibility_check,
                version_check,
                security_overrides,
            },

            owner_account_id: self.owner_account_id,
            environment_roles_from_shares: self.environment_roles_from_shares,

            current_deployment_revision: self.current_deployment_revision,
            current_deployment_deployment_revision: self.current_deployment_deployment_revision,
            current_deployment_deployment_version: self.current_deployment_deployment_version,
            current_deployment_deployment_hash: self.current_deployment_deployment_hash,
        })
    }
}

// Special record for listing all environments visible to a particular account.
// Includes necessary detials from higher up the hierarchy.
#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentWithDetailsRecord {
    pub environment_id: Uuid,
    pub environment_revision_id: i64,
    pub environment_name: String,
    pub environment_compatibility_check: bool,
    pub environment_version_check: bool,
    pub environment_security_overrides: bool,
    pub environment_roles_from_shares: i32,

    pub current_deployment_revision: Option<i64>,
    pub current_deployment_deployment_revision: Option<i64>,
    pub current_deployment_deployment_version: Option<String>,
    pub current_deployment_deployment_hash: Option<SqlBlake3Hash>,

    pub application_id: Uuid,
    pub application_name: String,

    pub account_id: Uuid,
    pub account_name: String,
    pub account_email: String,
}

impl TryFrom<EnvironmentWithDetailsRecord> for EnvironmentWithDetails {
    type Error = EnvironmentRepoError;
    fn try_from(value: EnvironmentWithDetailsRecord) -> Result<Self, Self::Error> {
        Ok(EnvironmentWithDetails {
            environment: EnvironmentSummary {
                id: EnvironmentId(value.environment_id),
                revision: value.environment_revision_id.try_into()?,
                name: EnvironmentName(value.environment_name),
                compatibility_check: value.environment_compatibility_check,
                version_check: value.environment_version_check,
                security_overrides: value.environment_security_overrides,
                roles_from_active_shares: environment_roles_from_bit_vector(
                    value.environment_roles_from_shares,
                ),
                current_deployment: match (
                    value.current_deployment_revision,
                    value.current_deployment_deployment_revision,
                    value.current_deployment_deployment_version,
                    value.current_deployment_deployment_hash,
                ) {
                    (
                        Some(revision),
                        Some(deployment_revision),
                        Some(deployment_version),
                        Some(deployment_hash),
                    ) => Some(EnvironmentCurrentDeploymentView {
                        revision: revision.try_into()?,
                        deployment_revision: deployment_revision.try_into()?,
                        deployment_version: deployment_version.into(),
                        deployment_hash: deployment_hash.into_blake3_hash().into(),
                    }),
                    _ => None,
                },
            },
            application: ApplicationSummary {
                id: ApplicationId(value.application_id),
                name: ApplicationName(value.application_name),
            },
            account: AccountSummary {
                id: AccountId(value.account_id),
                name: value.account_name,
                email: AccountEmail(value.account_email),
            },
        })
    }
}
