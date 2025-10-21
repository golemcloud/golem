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
use golem_common::model::application::{Application, ApplicationId, ApplicationName};
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ApplicationRepoError {
    #[error("Application violates unique index")]
    ApplicationViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(ApplicationRepoError, RepoError);

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ApplicationRecord {
    pub application_id: Uuid,
    pub name: String,
    pub account_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ApplicationRevisionRecord {
    pub application_id: Uuid,
    pub revision_id: i64,
    pub name: String,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl ApplicationRevisionRecord {
    pub fn from_model(application: Application, audit: DeletableRevisionAuditFields) -> Self {
        Self {
            application_id: application.id.0,
            revision_id: application.revision.0 as i64,
            name: application.name.0,
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
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ApplicationExtRevisionRecord {
    pub account_id: Uuid,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: ApplicationRevisionRecord,
}

impl From<ApplicationExtRevisionRecord> for Application {
    fn from(value: ApplicationExtRevisionRecord) -> Self {
        Self {
            id: ApplicationId(value.revision.application_id),
            revision: value.revision.revision_id.into(),
            account_id: AccountId(value.account_id),
            name: ApplicationName(value.revision.name),
        }
    }
}
