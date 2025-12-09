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
use crate::model::security_scheme::SecurityScheme;
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use anyhow::anyhow;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{
    Provider, SecuritySchemeId, SecuritySchemeName, SecuritySchemeRevision,
};
use golem_service_base::repo::RepoError;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use sqlx::FromRow;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum SecuritySchemeRepoError {
    #[error("There is a security scheme with this name in the environment")]
    SecuritySchemeViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(SecuritySchemeRepoError, RepoError);

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct SecuritySchemeRecord {
    pub security_scheme_id: Uuid,
    pub environment_id: Uuid,
    pub name: String,

    #[sqlx(flatten)]
    pub audit: AuditFields,

    pub current_revision_id: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct SecuritySchemeRevisionRecord {
    pub security_scheme_id: Uuid,
    pub revision_id: i64,

    pub provider_type: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: String,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl SecuritySchemeRevisionRecord {
    pub fn creation(
        security_scheme_id: SecuritySchemeId,
        provider_type: Provider,
        client_id: String,
        client_secret: String,
        redirect_url: &RedirectUrl,
        scopes: &[Scope],
        actor: AccountId,
    ) -> Self {
        let redirect_url: String = serde_json::to_string(&redirect_url).unwrap();
        let scopes: String = serde_json::to_string(&scopes).unwrap();

        Self {
            security_scheme_id: security_scheme_id.0,
            revision_id: SecuritySchemeRevision::INITIAL.into(),
            provider_type: provider_type.to_string(),
            client_id,
            client_secret,
            redirect_url,
            scopes,
            audit: DeletableRevisionAuditFields::new(actor.0),
        }
    }

    pub fn from_model(value: SecurityScheme, audit: DeletableRevisionAuditFields) -> Self {
        let redirect_url: String = serde_json::to_string(&value.redirect_url).unwrap();
        let scopes: String = serde_json::to_string(&value.scopes).unwrap();

        Self {
            security_scheme_id: value.id.0,
            revision_id: value.revision.into(),
            provider_type: value.provider_type.to_string(),
            client_id: value.client_id.into(),
            client_secret: value.client_secret.secret().clone(),
            redirect_url,
            scopes,
            audit,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct SecuritySchemeExtRevisionRecord {
    pub environment_id: Uuid,
    pub name: String,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: SecuritySchemeRevisionRecord,
}

impl TryFrom<SecuritySchemeExtRevisionRecord> for SecurityScheme {
    type Error = SecuritySchemeRepoError;
    fn try_from(value: SecuritySchemeExtRevisionRecord) -> Result<Self, Self::Error> {
        let scopes: Vec<Scope> = serde_json::from_str(&value.revision.scopes)
            .map_err(|e| anyhow::Error::from(e).context("Failed parsing scopes"))?;
        let redirect_url: RedirectUrl = serde_json::from_str(&value.revision.redirect_url)
            .map_err(|e| anyhow::Error::from(e).context("Failed parsing redirect_url"))?;
        let provider_type = Provider::from_str(&value.revision.provider_type)
            .map_err(|e| anyhow!("Failed parsing provider type: {e}"))?;
        let client_id = ClientId::new(value.revision.client_id);
        let client_secret = ClientSecret::new(value.revision.client_secret);

        Ok(Self {
            id: SecuritySchemeId(value.revision.security_scheme_id),
            revision: value.revision.revision_id.try_into()?,
            environment_id: EnvironmentId(value.environment_id),
            name: SecuritySchemeName(value.name),
            provider_type,
            client_id,
            client_secret,
            redirect_url,
            scopes,
        })
    }
}
