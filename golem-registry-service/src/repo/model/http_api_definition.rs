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
use desert_rust::BinaryCodec;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::api_definition::{
    HttpApiDefinition, HttpApiDefinitionId, HttpApiDefinitionName, HttpApiDefinitionRevision,
    HttpApiDefinitionVersion, HttpApiRoute,
};
use golem_common::model::diff;
use golem_common::model::diff::Hashable;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum HttpApiDefinitionRepoError {
    #[error("There is already an api definition with this name in the environment")]
    ApiDefinitionViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("Version already exists: {version}")]
    VersionAlreadyExists { version: String },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(HttpApiDefinitionRepoError, RepoError);

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRecord {
    pub http_api_definition_id: Uuid,
    pub name: String,
    pub environment_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
}

// Definition field of the HttpApiDefinitionRevisionRecord record. Must be kept backwards compatible
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct HttpApiDefinitionDefinitionBlob {
    routes: Vec<HttpApiRoute>,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRevisionRecord {
    pub http_api_definition_id: Uuid,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub definition: Vec<u8>, // TODO: model
}

impl HttpApiDefinitionRevisionRecord {
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
        http_api_definition_id: HttpApiDefinitionId,
        version: HttpApiDefinitionVersion,
        routes: Vec<HttpApiRoute>,
        actor: AccountId,
    ) -> Result<Self, HttpApiDefinitionRepoError> {
        let blob = HttpApiDefinitionDefinitionBlob { routes };
        let serialized_blob = desert_rust::serialize_to_byte_vec(&blob).map_err(|e| {
            anyhow::Error::from(e).context("serializing api definition blob failed")
        })?;

        Ok(Self {
            http_api_definition_id: http_api_definition_id.0,
            revision_id: HttpApiDefinitionRevision::INITIAL.0 as i64,
            version: version.0,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            definition: serialized_blob,
        })
    }

    pub fn from_model(
        value: HttpApiDefinition,
        audit: DeletableRevisionAuditFields,
    ) -> Result<Self, HttpApiDefinitionRepoError> {
        let blob = HttpApiDefinitionDefinitionBlob {
            routes: value.routes,
        };
        let serialized_blob = desert_rust::serialize_to_byte_vec(&blob).map_err(|e| {
            anyhow::Error::from(e).context("serializing api definition blob failed")
        })?;

        Ok(Self {
            http_api_definition_id: value.id.0,
            revision_id: value.revision.0 as i64,
            version: value.version.0,
            hash: SqlBlake3Hash::empty(),
            audit,
            definition: serialized_blob,
        })
    }

    pub fn deletion(
        created_by: Uuid,
        http_api_definition_id: Uuid,
        current_revision_id: i64,
    ) -> Self {
        Self {
            http_api_definition_id,
            revision_id: current_revision_id + 1,
            version: "".to_string(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            definition: vec![],
        }
    }

    pub fn to_diffable(&self) -> diff::HttpApiDefinition {
        diff::HttpApiDefinition {
            // TODO: add proper model
            routes: Default::default(),
            version: self.version.clone(),
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
pub struct HttpApiDefinitionExtRevisionRecord {
    pub name: String,
    pub environment_id: Uuid,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: HttpApiDefinitionRevisionRecord,
}

impl HttpApiDefinitionExtRevisionRecord {
    pub fn to_identity(self) -> HttpApiDefinitionRevisionIdentityRecord {
        HttpApiDefinitionRevisionIdentityRecord {
            http_api_definition_id: self.revision.http_api_definition_id,
            name: self.name,
            revision_id: self.revision.revision_id,
            version: self.revision.version,
            hash: self.revision.hash,
        }
    }
}

impl TryFrom<HttpApiDefinitionExtRevisionRecord> for HttpApiDefinition {
    type Error = HttpApiDefinitionRepoError;
    fn try_from(value: HttpApiDefinitionExtRevisionRecord) -> Result<Self, Self::Error> {
        let deserialzed_blob: HttpApiDefinitionDefinitionBlob =
            desert_rust::deserialize(&value.revision.definition).map_err(|e| {
                anyhow::Error::from(e).context("deserializing api definition blob failed")
            })?;

        Ok(Self {
            id: HttpApiDefinitionId(value.revision.http_api_definition_id),
            revision: HttpApiDefinitionRevision(value.revision.revision_id as u64),
            environment_id: EnvironmentId(value.environment_id),
            name: HttpApiDefinitionName(value.name),
            version: HttpApiDefinitionVersion(value.revision.version),
            routes: deserialzed_blob.routes,
            created_at: value.entity_created_at.into(),
            updated_at: value.revision.audit.created_at.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRevisionIdentityRecord {
    pub http_api_definition_id: Uuid,
    pub name: String,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash,
}

impl HttpApiDefinitionRevisionIdentityRecord {
    // NOTE: on deployment inserts we only expect names to be provided
    pub fn for_deployment_insert(name: String) -> Self {
        Self {
            http_api_definition_id: Uuid::nil(),
            name,
            revision_id: 0,
            version: "".to_string(),
            hash: SqlBlake3Hash::empty(),
        }
    }
}
