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

use crate::gateway_security::{
    GolemIdentityProviderMetadata, Provider, SecurityScheme, SecuritySchemeIdentifier,
    SecuritySchemeWithProviderMetadata,
};
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::fmt::Display;
use std::result::Result;
use std::str::FromStr;
use tracing::{info_span, Instrument, Span};

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct SecuritySchemeRecord {
    pub namespace: String,
    pub provider_type: String,
    pub security_scheme_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: String,
    pub security_scheme_metadata: Vec<u8>,
}

impl SecuritySchemeRecord {
    pub fn from_security_scheme_metadata<Namespace: Display>(
        namespace: &Namespace,
        value: &SecuritySchemeWithProviderMetadata,
    ) -> Result<SecuritySchemeRecord, String> {
        let metadata = identity_provider_metadata_serde::serialize(&value.provider_metadata)?;
        let scopes = value
            .security_scheme
            .scopes()
            .iter()
            .map(|scope| scope.to_string())
            .collect::<Vec<_>>()
            .join(",");

        Ok(SecuritySchemeRecord {
            namespace: namespace.to_string(),
            provider_type: value.security_scheme.provider_type().to_string(),
            security_scheme_id: value.security_scheme.scheme_identifier().to_string(),
            client_id: value.security_scheme.client_id().to_string(),
            client_secret: value.security_scheme.client_secret().secret().to_string(),
            redirect_url: value.security_scheme.redirect_url().to_string(),
            scopes,
            security_scheme_metadata: metadata.into(),
        })
    }
}

impl TryFrom<SecuritySchemeRecord> for SecuritySchemeWithProviderMetadata {
    type Error = String;
    fn try_from(value: SecuritySchemeRecord) -> Result<Self, Self::Error> {
        let provider_metadata: GolemIdentityProviderMetadata =
            identity_provider_metadata_serde::deserialize(&value.security_scheme_metadata)?;

        let redirect_url = RedirectUrl::new(value.redirect_url).map_err(|e| e.to_string())?;

        let provider_type = Provider::from_str(&value.provider_type).map_err(|e| e.to_string())?;

        let scheme_identifier = SecuritySchemeIdentifier::new(value.security_scheme_id);

        let client_id = ClientId::new(value.client_id);

        let client_secret = ClientSecret::new(value.client_secret);

        let scopes = value
            .scopes
            .split(",")
            .map(|x| Scope::new(x.trim().to_string()))
            .collect();

        let security_scheme = SecurityScheme::new(
            provider_type,
            scheme_identifier,
            client_id,
            client_secret,
            redirect_url,
            scopes,
        );

        Ok(SecuritySchemeWithProviderMetadata {
            security_scheme,
            provider_metadata,
        })
    }
}

#[async_trait]
pub trait SecuritySchemeRepo: Send + Sync {
    async fn create(&self, security_scheme_record: &SecuritySchemeRecord) -> Result<(), RepoError>;

    async fn get(
        &self,
        security_scheme_id: &str,
    ) -> Result<Option<SecuritySchemeRecord>, RepoError>;
}

pub struct DbSecuritySchemeRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbSecuritySchemeRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

pub struct LoggedSecuritySchemeRepo<Repo: SecuritySchemeRepo> {
    repo: Repo,
}

impl<Repo: SecuritySchemeRepo> LoggedSecuritySchemeRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span(security_scheme_id: &str) -> Span {
        info_span!(
            "security scheme repository ",
            security_scheme_id = security_scheme_id
        )
    }
}

#[async_trait]
impl<Repo: SecuritySchemeRepo + Send + Sync> SecuritySchemeRepo for LoggedSecuritySchemeRepo<Repo> {
    async fn create(&self, security_scheme_record: &SecuritySchemeRecord) -> Result<(), RepoError> {
        self.repo
            .create(security_scheme_record)
            .instrument(Self::span(&security_scheme_record.security_scheme_id))
            .await
    }

    async fn get(
        &self,
        security_scheme_id: &str,
    ) -> Result<Option<SecuritySchemeRecord>, RepoError> {
        self.repo
            .get(security_scheme_id)
            .instrument(Self::span(security_scheme_id))
            .await
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl SecuritySchemeRepo for DbSecuritySchemeRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, security: &SecuritySchemeRecord) -> Result<(), RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("security_scheme", "create")
            .begin()
            .await?;

        let query = sqlx::query(
            r#"
                  INSERT INTO security_schemes
                    (namespace, security_scheme_id, provider_type, client_id, client_secret, redirect_url, scopes, security_scheme_metadata)
                  VALUES
                    ($1, $2, $3, $4, $5, $6, $7, $8)
                   "#,
        )
            .bind(security.namespace.clone())
            .bind(security.security_scheme_id.clone())
            .bind(security.provider_type.clone())
            .bind(security.client_id.clone())
            .bind(security.client_secret.clone())
            .bind(security.redirect_url.clone())
            .bind(security.scopes.clone())
            .bind(security.security_scheme_metadata.clone());

        transaction.execute(query).await?;

        self.db_pool
            .with_rw("security_scheme", "create")
            .commit(transaction)
            .await?;
        Ok(())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(
        &self,
        security_scheme_id: &str,
    ) -> Result<Option<SecuritySchemeRecord>, RepoError> {
        let query = sqlx::query_as::<_, SecuritySchemeRecord>(
            r#"
                SELECT
                    namespace,
                    security_scheme_id,
                    provider_type,
                    client_id,
                    client_secret,
                    redirect_url,
                    scopes,
                    security_scheme_metadata
                FROM security_schemes
                WHERE security_scheme_id = $1
                "#,
        )
        .bind(security_scheme_id.to_string());

        self.db_pool
            .with("security_scheme", "get")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get(
        &self,
        security_scheme_id: &str,
    ) -> Result<Option<SecuritySchemeRecord>, RepoError> {
        let query = sqlx::query_as::<_, SecuritySchemeRecord>(
            r#"
                SELECT
                    namespace,
                    security_scheme_id,
                    provider_type,
                    client_id,
                    client_secret,
                    redirect_url,
                    scopes,
                    security_scheme_metadata
                FROM security_schemes
                WHERE security_scheme_id = $1
               "#,
        )
        .bind(security_scheme_id);

        self.db_pool
            .with_ro("security_scheme", "get")
            .fetch_optional_as(query)
            .await
    }
}

pub mod identity_provider_metadata_serde {
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::apidefinition::IdentityProviderMetadata as IdentityProviderMetadataProto;

    use crate::gateway_security::{
        from_identity_provider_metadata_proto, to_identity_provider_metadata_proto,
        GolemIdentityProviderMetadata,
    };
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &GolemIdentityProviderMetadata) -> Result<Bytes, String> {
        let proto_value: IdentityProviderMetadataProto =
            to_identity_provider_metadata_proto(value.clone());
        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<GolemIdentityProviderMetadata, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: IdentityProviderMetadataProto = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;
                let value = from_identity_provider_metadata_proto(proto_value)?;
                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}

pub mod constraint_serde {
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
    use golem_common::model::component_constraint::FunctionConstraints;
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &FunctionConstraints) -> Result<Bytes, String> {
        let proto_value: FunctionConstraintCollectionProto =
            FunctionConstraintCollectionProto::from(value.clone());

        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<FunctionConstraints, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: FunctionConstraintCollectionProto = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;

                let value = FunctionConstraints::try_from(proto_value.clone())?;

                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
