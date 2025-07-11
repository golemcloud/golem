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

use crate::repo::model::audit::{AuditFields, RevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
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
    pub hash: SqlBlake3Hash,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
}

impl EnvironmentRevisionRecord {
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
            audit: RevisionAuditFields::deletion(created_by),
            compatibility_check: false,
            version_check: false,
            security_overrides: false,
            hash: blake3::hash("".as_bytes()).into(),
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentCurrentRevisionRecord {
    pub name: String,
    pub application_id: Uuid,
    #[sqlx(flatten)]
    pub revision: EnvironmentRevisionRecord,
}
