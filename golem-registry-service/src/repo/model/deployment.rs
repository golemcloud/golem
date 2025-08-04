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
use crate::repo::model::component::ComponentRevisionIdentityRecord;
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use crate::repo::model::http_api_deployment::HttpApiDeploymentRevisionIdentityRecord;
use golem_common::model::diff;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum DeployError {
    #[error("Deployment concurrent revision creation")]
    DeploymentConcurrentRevisionCreation,
    #[error("Deployment hash mismatch: requested hash: {requested_hash:?}, actual hash: {actual_hash:?}.")]
    DeploymentHashMismatch {
        requested_hash: SqlBlake3Hash,
        actual_hash: SqlBlake3Hash,
    },
    #[error("Deployment version check failed, requested version: {version}")]
    DeploymentVersionCheckFailed { version: String },
    #[error("Deployment validation failed:\n{errors}", errors=format_validation_errors(.0.as_slice()))]
    ValidationErrors(Vec<DeployValidationError>),
}

fn format_validation_errors(errors: &[DeployValidationError]) -> String {
    errors
        .iter()
        .map(|err| format!("{err}"))
        .collect::<Vec<_>>()
        .join(",\n")
}

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum DeployValidationError {
    #[error("Missing HTTP API definitions for deployment: {http_api_deployment_id}")]
    HttpApiDeploymentMissingHttpApiDefinition {
        http_api_deployment_id: Uuid,
        missing_http_api_definition_ids: Vec<Uuid>,
    },
}

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

pub struct DeploymentHashes {
    pub env_hash: SqlBlake3Hash,
    pub deployment_hash: SqlBlake3Hash,
}

pub struct DeploymentIdentity {
    pub components: Vec<ComponentRevisionIdentityRecord>,
    pub http_api_definitions: Vec<HttpApiDefinitionRevisionIdentityRecord>,
    pub http_api_deployments: Vec<HttpApiDeploymentRevisionIdentityRecord>,
}

pub struct DeployedDeploymentIdentity {
    pub deployment_revision: DeploymentRevisionRecord,
    pub identity: DeploymentIdentity,
}

impl DeploymentIdentity {
    pub fn to_diffable(&self) -> diff::Deployment {
        diff::Deployment {
            components: self
                .components
                .iter()
                .map(|component| {
                    (
                        component.name.clone(),
                        diff::HashOf::from_blake3_hash(component.hash.unwrap().into()), // TODO: unwrap
                    )
                })
                .collect(),
            http_api_definitions: self
                .http_api_definitions
                .iter()
                .map(|definition| {
                    (
                        definition.name.clone(),
                        diff::HashOf::from_blake3_hash(definition.hash.into()),
                    )
                })
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|deployment| {
                    (
                        (deployment.subdomain.as_ref(), &deployment.host).into(),
                        diff::HashOf::from_blake3_hash(deployment.hash.into()),
                    )
                })
                .collect(),
        }
    }
}
