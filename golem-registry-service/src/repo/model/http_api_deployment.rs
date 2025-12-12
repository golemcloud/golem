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
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::{Database, FromRow};
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

// stored as string containing a json array
#[derive(Debug, Clone, PartialEq)]
pub struct HttpApiDefinitionNameList(pub Vec<HttpApiDefinitionName>);

impl<DB: Database> sqlx::Type<DB> for HttpApiDefinitionNameList
where
    String: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <String as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <String as sqlx::Type<DB>>::compatible(ty)
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for HttpApiDefinitionNameList
where
    String: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        let serialized = serde_json::to_string(&self.0)?;
        serialized.encode(buf)
    }

    fn size_hint(&self) -> usize {
        match serde_json::to_string(&self.0) {
            Ok(string) => string.size_hint(),
            Err(_) => 0,
        }
    }
}

impl<'r, DB: Database> sqlx::Decode<'r, DB> for HttpApiDefinitionNameList
where
    &'r str: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        let deserialized: Vec<HttpApiDefinitionName> =
            serde_json::from_str(<&'r str>::decode(value)?)?;
        Ok(Self(deserialized))
    }
}

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
    pub http_api_definitions: HttpApiDefinitionNameList,
}

impl HttpApiDeploymentRevisionRecord {
    pub fn for_recreation(
        mut self,
        http_api_deployment_id: Uuid,
        revision_id: i64,
    ) -> Result<Self, HttpApiDeploymentRepoError> {
        let revision: HttpApiDeploymentRevision = revision_id.try_into()?;
        let next_revision_id = revision.next()?.into();

        self.http_api_deployment_id = http_api_deployment_id;
        self.revision_id = next_revision_id;

        Ok(self)
    }

    pub fn creation(
        http_api_deployment_id: HttpApiDeploymentId,
        http_api_definitions: Vec<HttpApiDefinitionName>,
        actor: AccountId,
    ) -> Self {
        let mut value = Self {
            http_api_deployment_id: http_api_deployment_id.0,
            revision_id: HttpApiDeploymentRevision::INITIAL.into(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            http_api_definitions: HttpApiDefinitionNameList(http_api_definitions),
        };
        value.update_hash();
        value
    }

    pub fn from_model(value: HttpApiDeployment, audit: DeletableRevisionAuditFields) -> Self {
        let mut value = Self {
            http_api_deployment_id: value.id.0,
            revision_id: value.revision.into(),
            hash: SqlBlake3Hash::empty(),
            audit,
            http_api_definitions: HttpApiDefinitionNameList(value.api_definitions),
        };
        value.update_hash();
        value
    }

    pub fn deletion(
        created_by: Uuid,
        http_api_deployment_id: Uuid,
        current_revision_id: i64,
    ) -> Self {
        Self {
            http_api_deployment_id,
            revision_id: current_revision_id,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            http_api_definitions: HttpApiDefinitionNameList(Vec::new()),
        }
    }

    pub fn to_diffable(&self) -> diff::HttpApiDeployment {
        diff::HttpApiDeployment {
            apis: self
                .http_api_definitions
                .0
                .iter()
                .map(|had| had.0.clone())
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
        Ok(Self {
            id: HttpApiDeploymentId(value.revision.http_api_deployment_id),
            revision: value.revision.revision_id.try_into()?,
            environment_id: EnvironmentId(value.environment_id),
            domain: Domain(value.domain),
            hash: value.revision.hash.into(),
            api_definitions: value.revision.http_api_definitions.0,
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

impl TryFrom<HttpApiDeploymentRevisionIdentityRecord> for DeploymentPlanHttpApiDeploymentEntry {
    type Error = RepoError;
    fn try_from(value: HttpApiDeploymentRevisionIdentityRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: HttpApiDeploymentId(value.http_api_deployment_id),
            revision: value.revision_id.try_into()?,
            domain: Domain(value.domain),
            hash: value.hash.into(),
        })
    }
}
