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
use super::hash::SqlBlake3Hash;
use crate::repo::model::datetime::SqlDateTime;
use desert_rust::BinaryCodec;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry;
use golem_common::model::diff::{
    Hashable, McpDeployment as DiffMcpDeployment, McpDeploymentAgentOptions,
};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_deployment::{McpDeployment, McpDeploymentId, McpDeploymentRevision};
use golem_service_base::repo::RepoError;
use golem_service_base::repo::blob::Blob;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::BTreeMap;
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, BinaryCodec)]
pub struct McpDeploymentData {
    pub agents: BTreeMap<AgentTypeName, McpDeploymentAgentOptions>,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct McpDeploymentRevisionRecord {
    pub mcp_deployment_id: Uuid,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub domain: String,
    pub data: Blob<McpDeploymentData>,
}

impl McpDeploymentRevisionRecord {
    pub fn creation(
        mcp_deployment_id: McpDeploymentId,
        domain: Domain,
        actor: AccountId,
        agents: BTreeMap<AgentTypeName, McpDeploymentAgentOptions>,
    ) -> Self {
        let mut value = Self {
            mcp_deployment_id: mcp_deployment_id.0,
            revision_id: McpDeploymentRevision::INITIAL.into(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            domain: domain.0,
            data: Blob::new(McpDeploymentData { agents }),
        };
        value.update_hash();
        value
    }

    pub fn from_model(deployment: McpDeployment, audit: DeletableRevisionAuditFields) -> Self {
        let mut value = Self {
            mcp_deployment_id: deployment.id.0,
            revision_id: deployment.revision.into(),
            hash: SqlBlake3Hash::empty(),
            audit,
            domain: deployment.domain.0,
            data: Blob::new(McpDeploymentData {
                agents: deployment.agents,
            }),
        };
        value.update_hash();
        value
    }

    pub fn deletion(
        created_by: uuid::Uuid,
        mcp_deployment_id: Uuid,
        current_revision_id: i64,
        domain: String,
    ) -> Self {
        let mut value = Self {
            mcp_deployment_id,
            revision_id: current_revision_id,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            domain,
            data: Blob::new(McpDeploymentData {
                agents: Default::default(),
            }),
        };
        value.update_hash();
        value
    }

    pub fn to_diffable(&self) -> DiffMcpDeployment {
        DiffMcpDeployment {
            agents: self
                .data
                .value()
                .agents
                .iter()
                .map(|(k, v)| (k.0.clone(), v.clone()))
                .collect(),
        }
    }

    pub fn update_hash(&mut self) {
        self.hash = self.to_diffable().hash().into_blake3().into();
    }

    pub fn with_updated_hash(mut self) -> Self {
        self.update_hash();
        self
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct McpDeploymentExtRevisionRecord {
    pub environment_id: Uuid,
    pub domain: String,
    pub entity_created_at: SqlDateTime,
    #[sqlx(flatten)]
    pub revision: McpDeploymentRevisionRecord,
}

impl TryFrom<McpDeploymentExtRevisionRecord> for McpDeployment {
    type Error = McpDeploymentRepoError;

    fn try_from(value: McpDeploymentExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(McpDeployment {
            id: McpDeploymentId(value.revision.mcp_deployment_id),
            revision: value.revision.revision_id.try_into()?,
            environment_id: EnvironmentId(value.environment_id),
            domain: Domain(value.domain),
            hash: value.revision.hash.into(),
            agents: value.revision.data.value().agents.clone(),
            created_at: value.entity_created_at.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct McpDeploymentRevisionIdentityRecord {
    pub mcp_deployment_id: Uuid,
    pub domain: String,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash,
}

impl TryFrom<McpDeploymentRevisionIdentityRecord> for DeploymentPlanMcpDeploymentEntry {
    type Error = RepoError;
    fn try_from(value: McpDeploymentRevisionIdentityRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: McpDeploymentId(value.mcp_deployment_id),
            revision: value.revision_id.try_into()?,
            domain: Domain(value.domain),
            hash: value.hash.into(),
        })
    }
}
