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

use super::RecordWithEnvironmentCtx;
use super::environment_share::environment_roles_from_bit_vector;
use crate::model::WithEnvironmentCtx;
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields, RevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::diff;
use golem_common::model::diff::Hashable;
use golem_common::model::environment::{Environment, EnvironmentId, EnvironmentName};
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
pub struct EnvironmentRecord {
    pub environment_id: Uuid,
    pub name: String,
    pub application_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
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
pub struct EnvironmentExtRevisionRecord {
    pub application_id: Uuid,

    #[sqlx(flatten)]
    pub revision: EnvironmentRevisionRecord,
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
        }
    }
}

impl From<RecordWithEnvironmentCtx<EnvironmentExtRevisionRecord>>
    for WithEnvironmentCtx<Environment>
{
    fn from(record: RecordWithEnvironmentCtx<EnvironmentExtRevisionRecord>) -> Self {
        Self {
            value: record.value.into(),
            owner_account_id: AccountId(record.owner_account_id),
            roles_from_shares: HashSet::from_iter(environment_roles_from_bit_vector(
                record.environment_roles_from_shares,
            )),
        }
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
