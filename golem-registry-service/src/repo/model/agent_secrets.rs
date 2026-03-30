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
use desert_rust::BinaryCodec;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::repo::Blob;
use golem_service_base::repo::RepoError;
use golem_service_base::repo::SqlDateTime;
use golem_wasm::analysis::AnalysedType;
use sqlx::FromRow;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AgentSecretRepoError {
    #[error("There is already a secret for this path in this environment")]
    SecretViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(AgentSecretRepoError, RepoError);

#[derive(Debug, Clone)]
pub struct AgentSecretCreationRecord {
    pub environment_id: Uuid,
    pub path: Json<Vec<String>>,
    pub agent_secret_data: Blob<AgentSecretData>,

    pub revision: AgentSecretRevisionRecord,
}

impl AgentSecretCreationRecord {
    pub fn new(
        id: AgentSecretId,
        environment_id: EnvironmentId,
        path: CanonicalAgentSecretPath,
        secret_type: AnalysedType,
        secret_value: Option<golem_wasm::Value>,
        actor: AccountId,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            path: Json(path.0),
            agent_secret_data: Blob::new(AgentSecretData { secret_type }),
            revision: AgentSecretRevisionRecord {
                agent_secret_id: id.0,
                revision_id: AgentSecretRevision::INITIAL.into(),
                agent_secret_revision_data: Blob::new(AgentSecretRevisionData { secret_value }),
                audit: DeletableRevisionAuditFields::new(actor.0),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct AgentSecretData {
    pub secret_type: AnalysedType,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct AgentSecretRevisionData {
    pub secret_value: Option<golem_wasm::Value>,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AgentSecretRecord {
    pub agent_secret_id: Uuid,
    pub environment_id: Uuid,
    pub path: Json<Vec<String>>,

    pub agent_secret_data: Blob<AgentSecretData>,

    #[sqlx(flatten)]
    pub audit: AuditFields,

    pub current_revision_id: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AgentSecretRevisionRecord {
    pub agent_secret_id: Uuid,
    pub revision_id: i64,

    pub agent_secret_revision_data: Blob<AgentSecretRevisionData>,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl AgentSecretRevisionRecord {
    pub fn from_model(value: AgentSecret, audit: DeletableRevisionAuditFields) -> Self {
        Self {
            agent_secret_id: value.id.0,
            revision_id: value.revision.into(),
            agent_secret_revision_data: Blob::new(AgentSecretRevisionData {
                secret_value: value.secret_value,
            }),
            audit,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AgentSecretExtRevisionRecord {
    pub environment_id: Uuid,
    pub path: Json<Vec<String>>,

    pub agent_secret_data: Blob<AgentSecretData>,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: AgentSecretRevisionRecord,
}

impl TryFrom<AgentSecretExtRevisionRecord> for AgentSecret {
    type Error = AgentSecretRepoError;
    fn try_from(value: AgentSecretExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: AgentSecretId(value.revision.agent_secret_id),
            environment_id: EnvironmentId(value.environment_id),
            path: CanonicalAgentSecretPath(value.path.0),
            revision: value.revision.revision_id.try_into()?,
            secret_type: value.agent_secret_data.into_value().secret_type,
            secret_value: value
                .revision
                .agent_secret_revision_data
                .into_value()
                .secret_value,
        })
    }
}
