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
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use golem_common::model::diff;
use golem_common::model::diff::Hashable;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentRecord {
    pub http_api_deployment_id: Uuid,
    pub environment_id: Uuid,
    pub host: String,
    pub subdomain: Option<String>,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentRevisionRecord {
    pub http_api_deployment_id: Uuid,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,

    #[sqlx(skip)]
    pub http_api_definitions: Vec<HttpApiDefinitionRevisionIdentityRecord>,
}

impl HttpApiDeploymentRevisionRecord {
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
        http_api_deployment_id: Uuid,
        current_revision_id: i64,
    ) -> Self {
        Self {
            http_api_deployment_id,
            revision_id: current_revision_id + 1,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            http_api_definitions: vec![],
        }
    }

    pub fn to_diffable(&self) -> diff::HttpApiDeployment {
        diff::HttpApiDeployment {
            apis: self
                .http_api_definitions
                .iter()
                .map(|def| def.name.clone())
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
pub struct HttpApiDeploymentExtRevisionRecord {
    pub environment_id: Uuid,
    pub host: String,
    pub subdomain: Option<String>,
    #[sqlx(flatten)]
    pub revision: HttpApiDeploymentRevisionRecord,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentDefinitionRecord {
    pub http_api_deployment_id: Uuid,
    pub revision_id: i64,
    pub http_definition_id: Uuid,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentRevisionIdentityRecord {
    pub http_api_deployment_id: Uuid,
    pub host: String,
    pub subdomain: Option<String>,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash,

    #[sqlx(skip)]
    pub http_api_definitions: Vec<Uuid>,
}
