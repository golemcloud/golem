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
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields, RevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::diff::Hashable;
use golem_common::model::diff::{self};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentCurrentDeploymentView, EnvironmentId,
    EnvironmentName, EnvironmentRevision,
};
use golem_service_base::repo::RepoError;
use sqlx::{FromRow, types::Json};
use std::collections::{BTreeMap, HashSet};
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
    pub current_deployment_hash: Option<SqlBlake3Hash>,
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
    pub fn from_new_model(environment: EnvironmentCreation, actor: AccountId) -> Self {
        Self {
            environment_id: EnvironmentId::new_v4().0,
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
            revision_id: environment.revision.0 as i64,
            name: environment.name.0,
            hash: SqlBlake3Hash::empty(),
            compatibility_check: environment.compatibility_check,
            version_check: environment.version_check,
            security_overrides: environment.security_overrides,
            audit,
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

    pub fn ensure_deletion(self, current_revision_id: i64) -> Self {
        Self {
            revision_id: current_revision_id + 1,
            audit: self.audit.ensure_deletion(),
            ..self
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
pub struct MinimalEnvironmentExtRevisionRecord {
    pub application_id: Uuid,

    #[sqlx(flatten)]
    pub revision: EnvironmentRevisionRecord,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentExtRevisionRecord {
    pub application_id: Uuid,

    #[sqlx(flatten)]
    pub revision: EnvironmentRevisionRecord,

    pub owner_account_id: Uuid,
    pub environment_roles_from_shares: i32,

    pub current_deployment_revision: Option<i64>,
    pub current_deployment_hash: Option<SqlBlake3Hash>,
}

impl From<EnvironmentExtRevisionRecord> for Environment {
    fn from(value: EnvironmentExtRevisionRecord) -> Self {
        Self {
            id: EnvironmentId(value.revision.environment_id),
            revision: value.revision.revision_id.into(),
            application_id: ApplicationId(value.application_id),
            name: EnvironmentName(value.revision.name),
            compatibility_check: value.revision.compatibility_check,
            version_check: value.revision.version_check,
            security_overrides: value.revision.security_overrides,

            owner_account_id: AccountId(value.owner_account_id),
            roles_from_shares: HashSet::from_iter(environment_roles_from_bit_vector(
                value.environment_roles_from_shares,
            )),

            current_deployment: Option::zip(
                value.current_deployment_revision,
                value.current_deployment_hash,
            )
            .map(|(revision, hash)| EnvironmentCurrentDeploymentView {
                revision: revision.into(),
                hash: hash.into_blake3_hash().into(),
            }),
        }
    }
}

// Special record for listing environments. Parent context is mandatory while the environment itself and all children are mandatory
// Simplify when https://github.com/launchbadge/sqlx/issues/2934 is fixed
#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct OptionalEnvironmentExtRevisionRecord {
    pub application_id: Option<Uuid>,
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
    pub current_deployment_hash: Option<SqlBlake3Hash>,
}

impl OptionalEnvironmentExtRevisionRecord {
    pub fn owner_account_id(&self) -> AccountId {
        AccountId(self.owner_account_id)
    }

    pub fn environment_roles_from_shares(&self) -> HashSet<EnvironmentRole> {
        HashSet::from_iter(environment_roles_from_bit_vector(
            self.environment_roles_from_shares,
        ))
    }

    pub fn into_revision_record(self) -> Option<EnvironmentExtRevisionRecord> {
        let application_id = self.application_id?;
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
            application_id,
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
            current_deployment_hash: self.current_deployment_hash,
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentPluginInstallationRecord {
    pub environment_id: Uuid,
    pub hash: SqlBlake3Hash,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,

    #[sqlx(skip)]
    pub plugins: Vec<EnvironmentPluginInstallationRevisionRecord>,
}

impl EnvironmentPluginInstallationRecord {
    pub fn to_diffable(&self) -> diff::EnvironmentPluginInstallations {
        diff::EnvironmentPluginInstallations {
            plugins_by_priority: self
                .plugins
                .iter()
                .map(|plugin| {
                    (
                        plugin.priority.to_string(),
                        diff::PluginInstallation {
                            plugin_id: plugin.plugin_id,
                            parameters: plugin.parameters.0.clone(),
                        },
                    )
                })
                .collect(),
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
pub struct EnvironmentPluginInstallationRevisionRecord {
    pub environment_id: Uuid, // NOTE: set by repo during insert
    pub revision_id: i64,     // NOTE: set by repo during insert
    pub priority: i32,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub plugin_id: Uuid,        // NOTE: required for insert
    pub plugin_name: String,    // NOTE: returned by repo, not required to set
    pub plugin_version: String, // NOTE: returned by repo, not required to set
    pub parameters: Json<BTreeMap<String, String>>,
}

impl EnvironmentPluginInstallationRevisionRecord {
    pub fn ensure_environment(
        self,
        environment_id: Uuid,
        revision_id: i64,
        created_by: Uuid,
    ) -> Self {
        Self {
            environment_id,
            revision_id,
            audit: RevisionAuditFields {
                created_by,
                ..self.audit
            },
            ..self
        }
    }
}
