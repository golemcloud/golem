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
use crate::repo::model::hash::SqlBlake3Hash;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::diff;
use golem_common::model::diff::Hashable;
use golem_common::model::environment::{
    Environment, EnvironmentId, EnvironmentName, NewEnvironmentData,
};
use sqlx::FromRow;
use uuid::Uuid;

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
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
}

impl EnvironmentRevisionRecord {
    pub fn from_new_model(
        environment_id: EnvironmentId,
        data: NewEnvironmentData,
        actor: AccountId,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            revision_id: 0,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            compatibility_check: data.compatibility_check,
            version_check: data.version_check,
            security_overrides: data.security_overrides,
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

    pub fn deletion(created_by: Uuid, environment_id: Uuid, current_revision_id: i64) -> Self {
        Self {
            environment_id,
            revision_id: current_revision_id + 1,
            audit: DeletableRevisionAuditFields::deletion(created_by),
            compatibility_check: false,
            version_check: false,
            security_overrides: false,
            hash: SqlBlake3Hash::empty(),
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
    pub name: String,
    pub application_id: Uuid,
    #[sqlx(flatten)]
    pub revision: EnvironmentRevisionRecord,
}

impl From<EnvironmentExtRevisionRecord> for Environment {
    fn from(value: EnvironmentExtRevisionRecord) -> Self {
        Self {
            id: EnvironmentId(value.revision.environment_id),
            application_id: ApplicationId(value.application_id),
            name: EnvironmentName(value.name),
            compatibility_check: value.revision.compatibility_check,
            version_check: value.revision.version_check,
            security_overrides: value.revision.security_overrides,
        }
    }
}
