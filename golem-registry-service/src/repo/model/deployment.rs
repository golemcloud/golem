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

use crate::model::api_definition::{CompiledRoute, CompiledRouteWithContext};
use crate::model::component::Component;
use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::component::ComponentRevisionIdentityRecord;
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use crate::repo::model::http_api_deployment::HttpApiDeploymentRevisionIdentityRecord;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{
    Deployment, DeploymentPlan, DeploymentRevision, DeploymentSummary,
};
use golem_common::model::diff::{self, Hash, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{HttpApiDefinition, HttpApiDefinitionName};
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::repo::RepoError;
use golem_service_base::repo::blob::Blob;
use rib::RibCompilationError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DeployRepoError {
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("Version already exists: {version}")]
    VersionAlreadyExists { version: String },
    #[error("Deployment validation failed:\n{errors}", errors=format_validation_errors(.0.as_slice()))]
    ValidationErrors(Vec<DeployValidationError>),
    #[error("Deployment not found by revision: {revision_id}")]
    DeploymentNotFoundByRevision { revision_id: i64 },
    #[error("Deployment not found by version: {version}")]
    DeploymentNotfoundByVersion { version: String },
    #[error("Deployment is not unique by version: {version}")]
    DeploymentIsNotUniqueByVersion { version: String },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(DeployRepoError, RepoError);

fn format_validation_errors(errors: &[DeployValidationError]) -> String {
    errors
        .iter()
        .map(|err| format!("{err}"))
        .collect::<Vec<_>>()
        .join(",\n")
}

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum DeployValidationError {
    #[error(
        "Http api definition {missing_http_api_definition} requested by http api deployment {http_api_deployment_domain} is not part of the deployment"
    )]
    HttpApiDeploymentMissingHttpApiDefinition {
        http_api_deployment_domain: Domain,
        missing_http_api_definition: HttpApiDefinitionName,
    },
    #[error("Invalid path pattern: {0}")]
    HttpApiDefinitionInvalidPathPattern(String),
    #[error("Invalid rib expression: {0}")]
    InvalidRibExpr(String),
    #[error("Failed rib compilation: {0}")]
    RibCompilationFailed(RibCompilationError),
    #[error("Invalid http cors binding expression: {0}")]
    InvalidHttpCorsBindingExpr(String),
    #[error("Component {0} not found in deployment")]
    ComponentNotFound(ComponentName),
}

impl SafeDisplay for DeployValidationError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub deployment_revision_id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentExtRevisionRecord {
    #[sqlx(flatten)]
    pub revision: CurrentDeploymentRevisionRecord,

    pub deployment_version: String,
    pub deployment_hash: SqlBlake3Hash,
}

impl From<CurrentDeploymentExtRevisionRecord> for Deployment {
    fn from(value: CurrentDeploymentExtRevisionRecord) -> Self {
        Self {
            environment_id: EnvironmentId(value.revision.environment_id),
            revision: value.revision.deployment_revision_id.into(),
            version: value.deployment_version,
            deployment_hash: Hash::new(value.deployment_hash.into_blake3_hash()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentRecord {
    pub environment_id: Uuid,
    pub current_revision_id: i64,
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

impl From<DeploymentRevisionRecord> for Deployment {
    fn from(value: DeploymentRevisionRecord) -> Self {
        Self {
            environment_id: EnvironmentId(value.environment_id),
            revision: value.revision_id.into(),
            version: value.version,
            deployment_hash: Hash::new(value.hash.into_blake3_hash()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DeploymentComponentRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub component_id: Uuid,
    pub component_revision_id: i64,
}

impl DeploymentComponentRevisionRecord {
    pub fn from_model(
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        component: Component,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            component_id: component.id.0,
            component_revision_id: component.revision.into(),
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct DeploymentHttpApiDefinitionRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub http_api_definition_id: Uuid,
    pub http_api_definition_revision_id: i64,
}

impl DeploymentHttpApiDefinitionRevisionRecord {
    pub fn from_model(
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        http_api_definition: HttpApiDefinition,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            http_api_definition_id: http_api_definition.id.0,
            http_api_definition_revision_id: http_api_definition.revision.into(),
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct DeploymentHttpApiDeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub http_api_deployment_id: Uuid,
    pub http_api_deployment_revision_id: i64,
}

impl DeploymentHttpApiDeploymentRevisionRecord {
    pub fn from_model(
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        http_api_deployment: HttpApiDeployment,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            http_api_deployment_id: http_api_deployment.id.0,
            http_api_deployment_revision_id: http_api_deployment.revision.into(),
        }
    }
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

impl DeploymentIdentity {
    pub fn into_plan(
        self,
        current_deployment_revision: Option<DeploymentRevision>,
    ) -> DeploymentPlan {
        DeploymentPlan {
            current_deployment_revision,
            deployment_hash: self.to_diffable().hash(),
            components: self.components.into_iter().map(|c| c.into()).collect(),
            http_api_definitions: self
                .http_api_definitions
                .into_iter()
                .map(|had| had.into())
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .into_iter()
                .map(|had| had.into())
                .collect(),
        }
    }
}

impl From<DeploymentIdentity> for DeploymentSummary {
    fn from(value: DeploymentIdentity) -> Self {
        Self {
            deployment_hash: value.to_diffable().hash(),
            components: value.components.into_iter().map(|c| c.into()).collect(),
            http_api_definitions: value
                .http_api_definitions
                .into_iter()
                .map(|had| had.into())
                .collect(),
            http_api_deployments: value
                .http_api_deployments
                .into_iter()
                .map(|had| had.into())
                .collect(),
        }
    }
}

pub struct StagedDeploymentIdentity {
    pub latest_revision: DeploymentRevision,
    pub identity: DeploymentIdentity,
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
                        diff::HashOf::from_blake3_hash(component.hash.into()),
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
                        (&deployment.domain).into(),
                        diff::HashOf::from_blake3_hash(deployment.hash.into()),
                    )
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct DeploymentCompiledHttpApiRouteRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub id: i32,

    pub domain: String,

    pub security_scheme: Option<String>,
    pub compiled_route: Blob<CompiledRoute>,
}

impl DeploymentCompiledHttpApiRouteRecord {
    pub fn from_model(
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        id: i32,
        compiled_route: CompiledRouteWithContext,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            id,
            domain: compiled_route.domain.0,
            security_scheme: compiled_route.security_scheme.map(|scn| scn.0),
            compiled_route: Blob::new(compiled_route.route),
        }
    }
}

pub struct DeploymentRevisionCreationRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,

    pub version: String,
    pub hash: SqlBlake3Hash,

    pub components: Vec<DeploymentComponentRevisionRecord>,
    pub http_api_definitions: Vec<DeploymentHttpApiDefinitionRevisionRecord>,
    pub http_api_deployments: Vec<DeploymentHttpApiDeploymentRevisionRecord>,
    pub compiled_http_api_routes: Vec<DeploymentCompiledHttpApiRouteRecord>,
}

impl DeploymentRevisionCreationRecord {
    pub fn from_model(
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        version: String,
        hash: diff::Hash,
        components: Vec<Component>,
        http_api_definitions: Vec<HttpApiDefinition>,
        http_api_deployments: Vec<HttpApiDeployment>,
        compiled_routes: Vec<CompiledRouteWithContext>,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            version,
            hash: hash.into(),
            components: components
                .into_iter()
                .map(|c| {
                    DeploymentComponentRevisionRecord::from_model(
                        environment_id,
                        deployment_revision,
                        c,
                    )
                })
                .collect(),
            http_api_definitions: http_api_definitions
                .into_iter()
                .map(|had| {
                    DeploymentHttpApiDefinitionRevisionRecord::from_model(
                        environment_id,
                        deployment_revision,
                        had,
                    )
                })
                .collect(),
            http_api_deployments: http_api_deployments
                .into_iter()
                .map(|had| {
                    DeploymentHttpApiDeploymentRevisionRecord::from_model(
                        environment_id,
                        deployment_revision,
                        had,
                    )
                })
                .collect(),
            compiled_http_api_routes: compiled_routes
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    DeploymentCompiledHttpApiRouteRecord::from_model(
                        environment_id,
                        deployment_revision,
                        i32::try_from(i).expect("too many routes for i32"),
                        r,
                    )
                })
                .collect(),
        }
    }
}
