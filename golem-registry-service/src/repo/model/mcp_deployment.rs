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

use super::audit::DeletableRevisionAuditFields;
use crate::repo::model::datetime::SqlDateTime;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_deployment::{McpDeployment, McpDeploymentId, McpDeploymentRevision};
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum McpDeploymentRepoError {
    #[error("MCP deployment violates unique index")]
    McpDeploymentViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(McpDeploymentRepoError, RepoError);

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct McpDeploymentRevisionRecord {
    pub mcp_deployment_id: Uuid,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub domain: String,
}

impl McpDeploymentRevisionRecord {
    pub fn creation(mcp_deployment_id: McpDeploymentId, domain: Domain, actor: AccountId) -> Self {
        Self {
            mcp_deployment_id: mcp_deployment_id.0,
            revision_id: McpDeploymentRevision::INITIAL.into(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            domain: domain.0,
        }
    }

    pub fn from_model(deployment: McpDeployment) -> Self {
        Self {
            mcp_deployment_id: deployment.id.0,
            revision_id: deployment.revision.into(),
            audit: DeletableRevisionAuditFields::new(uuid::Uuid::nil()),
            domain: deployment.domain.0,
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct McpDeploymentExtRevisionRecord {
    pub mcp_deployment_id: Uuid,
    pub environment_id: Uuid,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub domain: String,
    pub created_at: SqlDateTime,
}

impl TryFrom<McpDeploymentExtRevisionRecord> for McpDeployment {
    type Error = McpDeploymentRepoError;

    fn try_from(value: McpDeploymentExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(McpDeployment {
            id: McpDeploymentId(value.mcp_deployment_id),
            revision: value.revision_id.try_into()?,
            environment_id: EnvironmentId(value.environment_id),
            domain: Domain(value.domain),
            created_at: value.created_at.into(),
        })
    }
}
