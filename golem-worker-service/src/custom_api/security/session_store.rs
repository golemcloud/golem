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

use super::model::{PendingOidcLogin, SessionId};
use crate::custom_api::model::OidcSession;
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{TimeDelta, Utc};
use fred::types::Expiration;
use golem_common::error_forwarding;
use golem_common::redis::{RedisError, RedisPool};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::repo::RepoError;
use sqlx::Row;
use std::time::Duration;
use tokio::task;
use tokio::time::interval;
use tracing::{Instrument, error};

#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(SessionStoreError, RepoError);

impl From<RedisError> for SessionStoreError {
    fn from(value: RedisError) -> Self {
        Self::InternalError(anyhow::Error::from(value).context("RedisError"))
    }
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn store_pending_oidc_login(
        &self,
        state: String,
        login: PendingOidcLogin,
    ) -> Result<(), SessionStoreError>;

    async fn take_pending_oidc_login(
        &self,
        state: &str,
    ) -> Result<Option<PendingOidcLogin>, SessionStoreError>;

    async fn store_authenticated_session(
        &self,
        session_id: &SessionId,
        session: OidcSession,
    ) -> Result<(), SessionStoreError>;

    async fn get_authenticated_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<OidcSession>, SessionStoreError>;
}

#[derive(Clone)]
pub struct RedisSessionStore {
    redis: RedisPool,
    pending_login_expiration: Expiration,
}

impl RedisSessionStore {
    pub fn new(redis: RedisPool, pending_login_expiration: Expiration) -> Self {
        Self {
            redis,
            pending_login_expiration,
        }
    }

    fn redis_key_for_session(session_id: &SessionId) -> String {
        format!("oidc_session:{}", session_id.0)
    }

    fn redis_key_for_pending(state: &str) -> String {
        format!("oidc_pending_login:{}", state)
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn store_pending_oidc_login(
        &self,
        state: String,
        login: PendingOidcLogin,
    ) -> Result<(), SessionStoreError> {
        let record = records::PendingOidcLoginRecord::from(login);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("PendingOidcLoginRecord serialization error: {e}"))?;

        let _: () = self
            .redis
            .with("session_store", "store_pending_oidc_login")
            .set(
                Self::redis_key_for_pending(&state),
                serialized,
                Some(self.pending_login_expiration.clone()),
                None,
                false,
            )
            .await?;

        Ok(())
    }

    async fn take_pending_oidc_login(
        &self,
        state: &str,
    ) -> Result<Option<PendingOidcLogin>, SessionStoreError> {
        let key = Self::redis_key_for_pending(state);
        let maybe_bytes: Option<Bytes> = self
            .redis
            .with("session_store", "take_pending_oidc_login")
            .get(&key)
            .await?;

        if let Some(bytes) = maybe_bytes {
            let record: records::PendingOidcLoginRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| anyhow!("PendingOidcLogin deserialization error: {e}"))?;
            let login = PendingOidcLogin::from(record);

            let _: i32 = self
                .redis
                .with("session_store", "del_pending")
                .del(&key)
                .await?;
            Ok(Some(login))
        } else {
            Ok(None)
        }
    }

    async fn store_authenticated_session(
        &self,
        session_id: &SessionId,
        session: OidcSession,
    ) -> Result<(), SessionStoreError> {
        let record = records::OidcSessionRecord::try_from(session)?;
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("OidcSessionRecord serialization error: {e}"))?;

        let now = chrono::Utc::now();
        let ttl_secs = (record.expires_at - now).num_seconds();
        let expiration = if ttl_secs > 0 {
            Expiration::EX(ttl_secs)
        } else {
            Expiration::EX(1)
        };

        let _: () = self
            .redis
            .with("session_store", "store_authenticated_session")
            .set(
                Self::redis_key_for_session(session_id),
                serialized,
                Some(expiration),
                None,
                false,
            )
            .await?;

        Ok(())
    }

    async fn get_authenticated_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<OidcSession>, SessionStoreError> {
        let maybe_bytes: Option<Bytes> = self
            .redis
            .with("session_store", "get_authenticated_session")
            .get(&Self::redis_key_for_session(session_id))
            .await?;

        if let Some(bytes) = maybe_bytes {
            let record: records::OidcSessionRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| anyhow!("OidcSession deserialization error: {e}"))?;
            let session = OidcSession::try_from(record)?;

            Ok(Some(session))
        } else {
            Ok(None)
        }
    }
}

pub struct SqliteSessionStore {
    pool: SqlitePool,
    pending_login_expiration: i64,
}

impl SqliteSessionStore {
    pub async fn new(
        pool: SqlitePool,
        pending_login_expiration: i64,
        cleanup_interval: Duration,
    ) -> anyhow::Result<Self> {
        Self::init(&pool).await?;
        Self::spawn_expiration_task(pool.clone(), cleanup_interval);
        Ok(Self {
            pool,
            pending_login_expiration,
        })
    }

    async fn init(pool: &SqlitePool) -> anyhow::Result<()> {
        pool.with_rw("session_store", "init")
            .execute(sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS oidc_pending_login (
                    state TEXT PRIMARY KEY,
                    value BLOB NOT NULL,
                    expires_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS oidc_session (
                    session_id TEXT PRIMARY KEY,
                    value BLOB NOT NULL,
                    expires_at INTEGER NOT NULL
                );
                "#,
            ))
            .await?;

        Ok(())
    }

    fn spawn_expiration_task(db_pool: SqlitePool, cleanup_interval: Duration) {
        task::spawn(
            async move {
                let mut cleanup_interval = interval(cleanup_interval);

                loop {
                    cleanup_interval.tick().await;

                    if let Err(e) = Self::cleanup_expired_oidc_pending_login(
                        db_pool.clone(),
                        Self::current_time(),
                    )
                    .await
                    {
                        error!("Failed to expire oidc pending logins: {}", e);
                    }

                    if let Err(e) =
                        Self::cleanup_expired_oidc_session(db_pool.clone(), Self::current_time())
                            .await
                    {
                        error!("Failed to expire oidc sessions: {}", e);
                    }
                }
            }
            .in_current_span(),
        );
    }

    async fn cleanup_expired_oidc_pending_login(
        pool: SqlitePool,
        current_time: i64,
    ) -> anyhow::Result<()> {
        let query =
            sqlx::query("DELETE FROM oidc_pending_login WHERE expires_at < ?;").bind(current_time);

        pool.with_rw("session_store", "cleanup_expired_oidc_pending_login")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn cleanup_expired_oidc_session(
        pool: SqlitePool,
        current_time: i64,
    ) -> anyhow::Result<()> {
        let query =
            sqlx::query("DELETE FROM oidc_session WHERE expires_at < ?;").bind(current_time);

        pool.with_rw("session_store", "cleanup_expired_oidc_session")
            .execute(query)
            .await?;

        Ok(())
    }

    pub fn current_time() -> i64 {
        chrono::Utc::now().timestamp()
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn store_pending_oidc_login(
        &self,
        state: String,
        login: PendingOidcLogin,
    ) -> Result<(), SessionStoreError> {
        let record = records::PendingOidcLoginRecord::from(login);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

        let expiry = Utc::now()
            .checked_add_signed(TimeDelta::seconds(self.pending_login_expiration))
            .ok_or_else(|| anyhow!("Failed to compute expiry"))?
            .timestamp();

        self
            .pool
            .with_rw("session_store", "store_pending_oidc_login")
            .execute(
                sqlx::query("INSERT OR REPLACE INTO oidc_pending_login (state, value, expires_at) VALUES (?, ?, ?)")
                    .bind(state)
                    .bind(serialized)
                    .bind(expiry)
            )
            .await?;

        Ok(())
    }

    async fn take_pending_oidc_login(
        &self,
        state: &str,
    ) -> Result<Option<PendingOidcLogin>, SessionStoreError> {
        let row = self
            .pool
            .with_ro("session_store", "take_pending_oidc_login_read")
            .fetch_optional(
                sqlx::query(
                    "SELECT value FROM oidc_pending_login WHERE state = ? AND expires_at > ?",
                )
                .bind(state)
                .bind(Self::current_time()),
            )
            .await?;

        if let Some(row) = row {
            let bytes: Vec<u8> = row.get(0);
            let record: records::PendingOidcLoginRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

            let login = PendingOidcLogin::from(record);

            self.pool
                .with_rw("session_store", "take_pending_oidc_login_write")
                .execute(sqlx::query("DELETE FROM oidc_pending_login WHERE state = ?").bind(state))
                .await?;

            Ok(Some(login))
        } else {
            Ok(None)
        }
    }

    async fn store_authenticated_session(
        &self,
        session_id: &SessionId,
        session: OidcSession,
    ) -> Result<(), SessionStoreError> {
        let record = records::OidcSessionRecord::try_from(session)?;
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

        let expires_at = record.expires_at.timestamp();

        self
            .pool
            .with_rw("session_store", "store_authenticated_session")
            .execute(
                sqlx::query("INSERT OR REPLACE INTO oidc_session (session_id, value, expires_at) VALUES (?, ?, ?)")
                    .bind(session_id.0)
                    .bind(serialized)
                    .bind(expires_at)
            )
            .await?;

        Ok(())
    }

    async fn get_authenticated_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<OidcSession>, SessionStoreError> {
        let row = self
            .pool
            .with_ro("session_store", "get_authenticated_session_read")
            .fetch_optional(
                sqlx::query("SELECT value, expires_at FROM oidc_session WHERE session_id = ? AND expires_at > ?")
                    .bind(session_id.0)
                    .bind(Self::current_time()),
            )
            .await?;

        if let Some(row) = row {
            let bytes: Vec<u8> = row.get(0);
            let record: records::OidcSessionRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

            let session = OidcSession::try_from(record)?;

            Ok(Some(session))
        } else {
            Ok(None)
        }
    }
}

mod records {
    use super::SessionStoreError;
    use crate::custom_api::model::OidcSession;
    use crate::custom_api::security::model::PendingOidcLogin;
    use anyhow::anyhow;
    use chrono::{DateTime, Utc};
    use desert_rust::BinaryCodec;
    use golem_common::model::security_scheme::SecuritySchemeId;
    use openidconnect::{Nonce, Scope};
    use std::collections::HashSet;

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct PendingOidcLoginRecord {
        pub scheme_id: SecuritySchemeId,
        pub original_uri: String,
        pub nonce: String,
    }

    impl From<PendingOidcLogin> for PendingOidcLoginRecord {
        fn from(value: PendingOidcLogin) -> Self {
            Self {
                scheme_id: value.scheme_id,
                original_uri: value.original_uri,
                nonce: value.nonce.secret().clone(),
            }
        }
    }

    impl From<PendingOidcLoginRecord> for PendingOidcLogin {
        fn from(value: PendingOidcLoginRecord) -> Self {
            Self {
                scheme_id: value.scheme_id,
                original_uri: value.original_uri,
                nonce: Nonce::new(value.nonce),
            }
        }
    }

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct OidcSessionRecord {
        pub subject: String,
        pub issuer: String,

        pub email: Option<String>,
        pub name: Option<String>,
        pub email_verified: Option<bool>,
        pub given_name: Option<String>,
        pub family_name: Option<String>,
        pub picture: Option<String>,
        pub preferred_username: Option<String>,

        pub claims: String,
        pub scopes: HashSet<String>,
        pub expires_at: DateTime<Utc>,
    }

    impl TryFrom<OidcSession> for OidcSessionRecord {
        type Error = SessionStoreError;

        fn try_from(value: OidcSession) -> Result<Self, Self::Error> {
            Ok(Self {
                subject: value.subject,
                issuer: value.issuer,

                email: value.email,
                name: value.name,
                email_verified: value.email_verified,
                given_name: value.given_name,
                family_name: value.family_name,
                picture: value.picture,
                preferred_username: value.preferred_username,

                claims: serde_json::to_string(&value.claims)
                    .map_err(|e| anyhow!("CoreIdTokenClaims serialization error: {e}"))?,
                scopes: value.scopes.into_iter().map(|s| s.to_string()).collect(),
                expires_at: value.expires_at,
            })
        }
    }

    impl TryFrom<OidcSessionRecord> for OidcSession {
        type Error = SessionStoreError;

        fn try_from(value: OidcSessionRecord) -> Result<Self, Self::Error> {
            Ok(Self {
                subject: value.subject,
                issuer: value.issuer,

                email: value.email,
                name: value.name,
                email_verified: value.email_verified,
                given_name: value.given_name,
                family_name: value.family_name,
                picture: value.picture,
                preferred_username: value.preferred_username,

                claims: serde_json::from_str(&value.claims)
                    .map_err(|e| anyhow!("CoreIdTokenClaims deserialization error: {e}"))?,
                scopes: value.scopes.into_iter().map(Scope::new).collect(),
                expires_at: value.expires_at,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::records;
    use crate::custom_api::OidcSession;
    use crate::custom_api::security::model::PendingOidcLogin;
    use chrono::{TimeDelta, Utc};
    use golem_common::model::security_scheme::SecuritySchemeId;
    use openidconnect::core::CoreIdTokenClaims;
    use openidconnect::{
        Audience, EmptyAdditionalClaims, IssuerUrl, Nonce, Scope, StandardClaims, SubjectIdentifier,
    };
    use std::collections::HashSet;
    use test_r::test;

    fn sample_pending_login() -> PendingOidcLogin {
        PendingOidcLogin {
            scheme_id: SecuritySchemeId::new(),
            original_uri: "https://example.com".to_string(),
            nonce: Nonce::new("nonce123".to_string()),
        }
    }

    fn sample_claims(expires_at: chrono::DateTime<Utc>) -> CoreIdTokenClaims {
        let issuer = IssuerUrl::new("https://issuer.example".to_string()).unwrap();
        let audience = Audience::new("client_id".to_string());

        let standard_claims = StandardClaims::new(SubjectIdentifier::new("sub".to_string()));

        CoreIdTokenClaims::new(
            issuer,
            vec![audience],
            expires_at,
            Utc::now(),
            standard_claims,
            EmptyAdditionalClaims {},
        )
    }

    fn sample_session(expires_at: chrono::DateTime<Utc>) -> OidcSession {
        OidcSession {
            subject: "sub".into(),
            issuer: "issuer".into(),
            email: Some("a@b.com".into()),
            name: Some("Alice".into()),
            email_verified: Some(true),
            given_name: None,
            family_name: None,
            picture: None,
            preferred_username: None,
            claims: sample_claims(expires_at),
            scopes: HashSet::from([Scope::new("openid".into())]),
            expires_at,
        }
    }

    #[test]
    fn pending_login_record_roundtrip() {
        let login = sample_pending_login();
        let record = records::PendingOidcLoginRecord::from(login.clone());
        let login2 = PendingOidcLogin::from(record);

        assert_eq!(login.scheme_id, login2.scheme_id);
        assert_eq!(login.original_uri, login2.original_uri);
        assert_eq!(login.nonce.secret(), login2.nonce.secret());
    }

    #[test]
    fn oidc_session_record_roundtrip() {
        let expires = Utc::now() + TimeDelta::seconds(60);
        let session = sample_session(expires);

        let record = records::OidcSessionRecord::try_from(session.clone()).unwrap();
        let session2 = OidcSession::try_from(record).unwrap();

        assert_eq!(session.subject, session2.subject);
        assert_eq!(session.issuer, session2.issuer);
        assert_eq!(session.expires_at, session2.expires_at);
        assert_eq!(session.scopes.len(), session2.scopes.len());
    }
}
