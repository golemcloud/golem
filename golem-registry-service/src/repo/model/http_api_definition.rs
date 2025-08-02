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
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRecord {
    pub http_api_definition_id: Uuid,
    pub name: String,
    pub environment_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRevisionRecord {
    pub http_api_definition_id: Uuid,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub definition: Vec<u8>, // TODO: model
}

impl HttpApiDefinitionRevisionRecord {
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

    pub fn deletion(
        created_by: Uuid,
        http_api_definition_id: Uuid,
        current_revision_id: i64,
    ) -> Self {
        Self {
            http_api_definition_id,
            revision_id: current_revision_id + 1,
            version: "".to_string(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            definition: vec![],
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRevisionIdentityRecord {
    pub http_api_definition_id: Uuid,
    pub name: String,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash,
}
