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

use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::hash::SqlBlake3Hash;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentRecord {
    pub environment_id: Uuid,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DeploymentComponentRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub component_id: Uuid,
    pub component_revision_id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct DeploymentHttpApiDefinitionRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub http_api_definition_id: Uuid,
    pub http_api_definition_revision_id: i64,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct DeploymentHttpApiDeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub http_api_deployment_id: Uuid,
    pub http_api_deployment_revision_id: i64,
}
