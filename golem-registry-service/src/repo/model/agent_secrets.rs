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
    AgentSecretId, AgentSecretOwner, AgentSecretRevision, CanonicalAgentSecretPath,
    SecretDestination,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::ToolName;
use golem_common::model::{AgentFingerprint, AgentId};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_value::SchemaValue;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::repo::Blob;
use golem_service_base::repo::RepoError;
use golem_service_base::repo::SqlDateTime;
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

#[derive(Debug, Clone, PartialEq)]
pub struct AgentSecretOwnerColumns {
    pub owner_kind: String,
    pub agent_id: Option<Uuid>,
    pub agent_fingerprint: Option<Uuid>,
    pub agent_type: Option<i64>,
    pub tool_name: Option<String>,
}

impl<'r, R: sqlx::Row> FromRow<'r, R> for AgentSecretOwnerColumns
where
    &'r str: sqlx::ColumnIndex<R>,
    String: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    Uuid: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    i64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    fn from_row(row: &'r R) -> Result<Self, sqlx::Error> {
        let columns = Self {
            owner_kind: row.try_get("owner_kind")?,
            agent_id: row.try_get("agent_id")?,
            agent_fingerprint: row.try_get("agent_fingerprint")?,
            agent_type: row.try_get("agent_type")?,
            tool_name: row.try_get("tool_name")?,
        };
        AgentSecretOwner::try_from(&columns).map_err(|error| sqlx::Error::Decode(error.into()))?;
        Ok(columns)
    }
}

impl From<&AgentSecretOwner> for AgentSecretOwnerColumns {
    fn from(owner: &AgentSecretOwner) -> Self {
        let mut columns = Self {
            owner_kind: "environment".to_string(),
            agent_id: None,
            agent_fingerprint: None,
            agent_type: None,
            tool_name: None,
        };
        match owner {
            AgentSecretOwner::Environment => {}
            AgentSecretOwner::Agent {
                agent_id,
                fingerprint,
            } => {
                columns.owner_kind = "agent".to_string();
                columns.agent_id = Some(agent_id.0);
                columns.agent_fingerprint = Some(fingerprint.0);
                columns.agent_type = Some(i64::from(agent_id.type_tag()));
            }
            AgentSecretOwner::Tool { tool_name } => {
                columns.owner_kind = "tool".to_string();
                columns.tool_name = Some(tool_name.to_string());
            }
        }
        columns
    }
}

impl TryFrom<&AgentSecretOwnerColumns> for AgentSecretOwner {
    type Error = anyhow::Error;

    fn try_from(columns: &AgentSecretOwnerColumns) -> Result<Self, Self::Error> {
        match (
            columns.owner_kind.as_str(),
            columns.agent_id,
            columns.agent_fingerprint,
            columns.agent_type,
            &columns.tool_name,
        ) {
            ("environment", None, None, None, None) => Ok(Self::Environment),
            ("agent", Some(id), Some(fingerprint), Some(type_tag), None) => {
                let agent_id = AgentId(id);
                anyhow::ensure!(
                    agent_id.is_issued()
                        && agent_id.type_id().is_some()
                        && i64::from(agent_id.type_tag()) == type_tag,
                    "Invalid secret agent owner identity"
                );
                Ok(Self::Agent {
                    agent_id,
                    fingerprint: AgentFingerprint(fingerprint),
                })
            }
            ("tool", None, None, None, Some(name)) => Ok(Self::Tool {
                tool_name: ToolName::try_from(name.as_str()).map_err(anyhow::Error::msg)?,
            }),
            _ => Err(anyhow::anyhow!("Invalid secret owner columns")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentSecretDestinations {
    Copy,
    Set(Vec<SecretDestination>),
}

impl<'r, R: sqlx::Row> FromRow<'r, R> for AgentSecretDestinations
where
    &'r str: sqlx::ColumnIndex<R>,
    Json<Vec<SecretDestination>>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    fn from_row(row: &'r R) -> Result<Self, sqlx::Error> {
        Ok(Self::Set(
            row.try_get::<Json<Vec<SecretDestination>>, _>("destinations")?
                .0,
        ))
    }
}

#[derive(Debug, Clone)]
pub struct AgentSecretCreationRecord {
    pub environment_id: Uuid,
    pub path: Json<Vec<String>>,
    pub owner: AgentSecretOwnerColumns,
    pub agent_secret_data: Blob<AgentSecretData>,

    pub revision: AgentSecretRevisionRecord,
}

impl AgentSecretCreationRecord {
    /// Construct a creation record from the in-memory schema-layer carrier.
    ///
    /// The persisted blob shape stores schema types directly.
    pub fn new(
        id: AgentSecretId,
        environment_id: EnvironmentId,
        owner: AgentSecretOwner,
        path: CanonicalAgentSecretPath,
        secret_type: SchemaGraph,
        secret_value: Option<SchemaValue>,
        destinations: Vec<SecretDestination>,
        actor: AccountId,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            path: Json(path.0),
            owner: (&owner).into(),
            agent_secret_data: Blob::new(AgentSecretData { secret_type }),
            revision: AgentSecretRevisionRecord {
                agent_secret_id: id.0,
                revision_id: AgentSecretRevision::INITIAL.into(),
                agent_secret_revision_data: Blob::new(AgentSecretRevisionData { secret_value }),
                destinations: AgentSecretDestinations::Set(destinations),
                audit: DeletableRevisionAuditFields::new(actor.0),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct AgentSecretData {
    pub secret_type: SchemaGraph,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct AgentSecretRevisionData {
    pub secret_value: Option<SchemaValue>,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AgentSecretRecord {
    pub agent_secret_id: Uuid,
    pub environment_id: Uuid,
    pub path: Json<Vec<String>>,
    #[sqlx(flatten)]
    pub owner: AgentSecretOwnerColumns,

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
    pub destinations: AgentSecretDestinations,

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
            destinations: if audit.deleted {
                AgentSecretDestinations::Set(vec![])
            } else {
                AgentSecretDestinations::Copy
            },
            audit,
        }
    }

    pub fn delete(
        id: AgentSecretId,
        current_revision: AgentSecretRevision,
        actor: AccountId,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            agent_secret_id: id.0,
            revision_id: current_revision.next()?.into(),
            agent_secret_revision_data: Blob::new(AgentSecretRevisionData { secret_value: None }),
            destinations: AgentSecretDestinations::Set(vec![]),
            audit: DeletableRevisionAuditFields::deletion(actor.0),
        })
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AgentSecretExtRevisionRecord {
    pub environment_id: Uuid,
    pub path: Json<Vec<String>>,
    #[sqlx(flatten)]
    pub owner: AgentSecretOwnerColumns,

    pub agent_secret_data: Blob<AgentSecretData>,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: AgentSecretRevisionRecord,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AgentSecretAuthExtRevisionRecord {
    #[sqlx(flatten)]
    pub agent_secret: AgentSecretExtRevisionRecord,

    pub environment_name: String,
    pub application_name: String,
    pub owner_account_email: String,
}

impl TryFrom<AgentSecretExtRevisionRecord> for AgentSecret {
    type Error = AgentSecretRepoError;
    fn try_from(value: AgentSecretExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: AgentSecretId(value.revision.agent_secret_id),
            owner: AgentSecretOwner::try_from(&value.owner)?,
            destinations: match value.revision.destinations {
                AgentSecretDestinations::Set(destinations) => destinations,
                AgentSecretDestinations::Copy => {
                    return Err(anyhow::anyhow!("Unresolved secret destinations").into());
                }
            },
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
