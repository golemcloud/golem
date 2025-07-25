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

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentRecord {
    pub http_api_deployment_id: Uuid,
    pub name: String,
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
    pub hash: Option<SqlBlake3Hash>,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentDefinitionRecord {
    pub http_api_deployment_id: Uuid,
    pub revision_id: i64,
    pub http_definition_id: Uuid,
}
