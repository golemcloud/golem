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
use crate::repo::model::hash::SqlBlake3Hash;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::deployment::DeploymentPlanHttpApiDeploymentEntry;
use golem_common::model::diff;
use golem_common::model::diff::Hashable;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use golem_common::model::http_api_deployment::{
    HttpApiDeployment, HttpApiDeploymentId, HttpApiDeploymentRevision,
};
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum HttpApiDeploymentRepoError {
    #[error("Api deployment violates unique index")]
    ApiDeploymentViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(HttpApiDeploymentRepoError, RepoError);

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentRecord {
    pub http_api_deployment_id: Uuid,
    pub environment_id: Uuid,
    pub domain: String,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentRevisionRecord {
    pub http_api_deployment_id: Uuid,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,

    // json string array as string
    pub http_api_definitions: String,
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

    pub fn creation(
        http_api_deployment_id: HttpApiDeploymentId,
        http_api_definitions: &Vec<HttpApiDefinitionName>,
        actor: AccountId,
    ) -> Self {
        Self {
            http_api_deployment_id: http_api_deployment_id.0,
            revision_id: HttpApiDeploymentRevision::INITIAL.into(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            http_api_definitions: serde_json::to_string(http_api_definitions).unwrap(),
        }
    }

    pub fn from_model(value: HttpApiDeployment, audit: DeletableRevisionAuditFields) -> Self {
        Self {
            http_api_deployment_id: value.id.0,
            revision_id: value.revision.into(),
            hash: SqlBlake3Hash::empty(),
            audit,
            http_api_definitions: serde_json::to_string(&value.api_definitions).unwrap(),
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
            http_api_definitions: serde_json::to_string::<Vec<HttpApiDefinitionName>>(&Vec::new())
                .unwrap(),
        }
    }

    pub fn to_diffable(&self) -> anyhow::Result<diff::HttpApiDeployment> {
        let http_api_definitions: Vec<HttpApiDefinitionName> =
            serde_json::from_str(&self.http_api_definitions)?;

        Ok(diff::HttpApiDeployment {
            apis: http_api_definitions.into_iter().map(|had| had.0).collect(),
        })
    }

    pub fn update_hash(&mut self) -> anyhow::Result<()> {
        self.hash = self.to_diffable()?.hash().into_blake3().into();
        Ok(())
    }

    pub fn with_updated_hash(mut self) -> anyhow::Result<Self> {
        self.update_hash()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentExtRevisionRecord {
    pub environment_id: Uuid,
    pub domain: String,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: HttpApiDeploymentRevisionRecord,
}

impl TryFrom<HttpApiDeploymentExtRevisionRecord> for HttpApiDeployment {
    type Error = HttpApiDeploymentRepoError;
    fn try_from(value: HttpApiDeploymentExtRevisionRecord) -> Result<Self, Self::Error> {
        let http_api_definitions: Vec<HttpApiDefinitionName> =
            serde_json::from_str(&value.revision.http_api_definitions).map_err(|err| {
                anyhow::Error::from(err).context("Failed parsing persisted http_api_definitions")
            })?;

        Ok(Self {
            id: HttpApiDeploymentId(value.revision.http_api_deployment_id),
            revision: HttpApiDeploymentRevision(value.revision.revision_id as u64),
            environment_id: EnvironmentId(value.environment_id),
            domain: Domain(value.domain),
            hash: value.revision.hash.into(),
            api_definitions: http_api_definitions,
            created_at: value.entity_created_at.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDeploymentRevisionIdentityRecord {
    pub http_api_deployment_id: Uuid,
    pub domain: String,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash,
}

impl From<HttpApiDeploymentRevisionIdentityRecord> for DeploymentPlanHttpApiDeploymentEntry {
    fn from(value: HttpApiDeploymentRevisionIdentityRecord) -> Self {
        Self {
            id: HttpApiDeploymentId(value.http_api_deployment_id),
            revision: value.revision_id.into(),
            domain: Domain(value.domain),
            hash: value.hash.into(),
        }
    }
}
