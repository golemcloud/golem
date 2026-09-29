// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::model::{
    AuthorizationCode, BearerToken, McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin,
    PendingPkceLogin, PkceAuthorizationCode, PkceBearerCredential, PkceBinding, SessionId,
};
use crate::custom_api::model::OidcSession;
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{TimeDelta, Utc};
use fred::types::Expiration;
use golem_common::error_forwarding;
use golem_common::redis::{RedisError, RedisPool};
use golem_service_base::db::PoolApi;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::repo::RepoError;
use sqlx::Row;
use std::time::Duration;
use tokio::task;
use tokio_util::sync::CancellationToken;
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
        state: &str,
        login: PendingOidcLogin,
    ) -> Result<(), SessionStoreError>;

    async fn take_pending_oidc_login(
        &self,
        state: &str,
    ) -> Result<Option<PendingOidcLogin>, SessionStoreError>;

    async fn store_pending_pkce_login(
        &self,
        upstream_state: &str,
        login: PendingPkceLogin,
    ) -> Result<(), SessionStoreError>;

    async fn take_pending_pkce_login(
        &self,
        upstream_state: &str,
    ) -> Result<Option<PendingPkceLogin>, SessionStoreError>;

    async fn store_authorization_code(
        &self,
        code: &AuthorizationCode,
        entry: PkceAuthorizationCode,
    ) -> Result<(), SessionStoreError>;

    async fn take_authorization_code(
        &self,
        code: &str,
    ) -> Result<Option<PkceAuthorizationCode>, SessionStoreError>;

    async fn store_bearer_credential(
        &self,
        token: &BearerToken,
        credential: PkceBearerCredential,
    ) -> Result<(), SessionStoreError>;

    async fn get_bearer_credential(
        &self,
        token: &str,
        expected_binding: &PkceBinding,
    ) -> Result<Option<PkceBearerCredential>, SessionStoreError>;

    async fn store_authenticated_session(
        &self,
        session_id: &SessionId,
        session: OidcSession,
    ) -> Result<(), SessionStoreError>;

    async fn get_authenticated_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<OidcSession>, SessionStoreError>;

    async fn store_mcp_pending_auth(
        &self,
        state: &str,
        pending: McpPendingAuth,
    ) -> Result<(), SessionStoreError>;

    async fn take_mcp_pending_auth(
        &self,
        state: &str,
    ) -> Result<Option<McpPendingAuth>, SessionStoreError>;

    async fn store_mcp_proxy_code(
        &self,
        code: &str,
        entry: McpProxyCodeEntry,
    ) -> Result<(), SessionStoreError>;

    async fn take_mcp_proxy_code(
        &self,
        code: &str,
    ) -> Result<Option<McpProxyCodeEntry>, SessionStoreError>;
}

#[derive(Clone)]
pub struct RedisSessionStore {
    redis: RedisPool,
    pending_login_expiration: Expiration,
}

impl RedisSessionStore {
    const ATOMIC_TAKE_SCRIPT: &'static str = r#"
local value = redis.call('GET', KEYS[1])
if value then
  redis.call('DEL', KEYS[1])
end
return value
"#;

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

    fn redis_key_for_pending_pkce(state: &str) -> String {
        format!("oidc_pkce_pending:{}", credential_digest("pending", state))
    }

    fn redis_key_for_authorization_code(code: &str) -> String {
        format!(
            "oidc_pkce_authorization_code:{}",
            credential_digest("authorization-code", code)
        )
    }

    fn redis_key_for_bearer(token: &str) -> String {
        format!("oidc_pkce_bearer:{}", credential_digest("bearer", token))
    }

    fn redis_key_for_mcp_pending(state: &str) -> String {
        format!("mcp_pending_auth:{}", state)
    }

    fn redis_key_for_mcp_proxy_code(code: &str) -> String {
        format!("mcp_proxy_code:{}", code)
    }

    fn expiration(expires_at: chrono::DateTime<Utc>) -> Expiration {
        Expiration::PX((expires_at - Utc::now()).num_milliseconds().max(1))
    }

    async fn atomic_take(
        &self,
        operation: &'static str,
        key: String,
    ) -> Result<Option<Bytes>, SessionStoreError> {
        let value = self
            .redis
            .with("session_store", operation)
            .eval(Self::ATOMIC_TAKE_SCRIPT, &[key], vec![], None)
            .await?;
        Ok(value.convert()?)
    }
}

fn credential_digest(kind: &str, secret: &str) -> String {
    let context = format!("golem oidc pkce {kind}");
    blake3::derive_key(&context, secret.as_bytes())
        .as_slice()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn store_pending_oidc_login(
        &self,
        state: &str,
        login: PendingOidcLogin,
    ) -> Result<(), SessionStoreError> {
        let record = records::PendingOidcLoginRecord::from(login);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("PendingOidcLoginRecord serialization error: {e}"))?;

        let _: () = self
            .redis
            .with("session_store", "store_pending_oidc_login")
            .set(
                Self::redis_key_for_pending(state),
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
        let maybe_bytes = self
            .atomic_take(
                "take_pending_oidc_login",
                Self::redis_key_for_pending(state),
            )
            .await?;

        if let Some(bytes) = maybe_bytes {
            let record: records::PendingOidcLoginRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| anyhow!("PendingOidcLogin deserialization error: {e}"))?;
            Ok(Some(PendingOidcLogin::from(record)))
        } else {
            Ok(None)
        }
    }

    async fn store_pending_pkce_login(
        &self,
        upstream_state: &str,
        login: PendingPkceLogin,
    ) -> Result<(), SessionStoreError> {
        let expires_at = login.expires_at;
        let record = records::PendingPkceLoginRecord::from(login);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("PendingPkceLogin serialization error: {e}"))?;
        let _: () = self
            .redis
            .with("session_store", "store_pending_pkce_login")
            .set(
                Self::redis_key_for_pending_pkce(upstream_state),
                serialized,
                Some(Self::expiration(expires_at)),
                None,
                false,
            )
            .await?;
        Ok(())
    }

    async fn take_pending_pkce_login(
        &self,
        upstream_state: &str,
    ) -> Result<Option<PendingPkceLogin>, SessionStoreError> {
        let maybe_bytes = self
            .atomic_take(
                "take_pending_pkce_login",
                Self::redis_key_for_pending_pkce(upstream_state),
            )
            .await?;
        records::decode_pending_pkce(maybe_bytes)
    }

    async fn store_authorization_code(
        &self,
        code: &AuthorizationCode,
        entry: PkceAuthorizationCode,
    ) -> Result<(), SessionStoreError> {
        let expires_at = entry.expires_at;
        let record = records::PkceAuthorizationCodeRecord::try_from(entry)?;
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("PkceAuthorizationCode serialization error: {e}"))?;
        let _: () = self
            .redis
            .with("session_store", "store_authorization_code")
            .set(
                Self::redis_key_for_authorization_code(code.secret()),
                serialized,
                Some(Self::expiration(expires_at)),
                None,
                false,
            )
            .await?;
        Ok(())
    }

    async fn take_authorization_code(
        &self,
        code: &str,
    ) -> Result<Option<PkceAuthorizationCode>, SessionStoreError> {
        let maybe_bytes = self
            .atomic_take(
                "take_authorization_code",
                Self::redis_key_for_authorization_code(code),
            )
            .await?;
        records::decode_authorization_code(maybe_bytes)
    }

    async fn store_bearer_credential(
        &self,
        token: &BearerToken,
        credential: PkceBearerCredential,
    ) -> Result<(), SessionStoreError> {
        let expires_at = credential.expires_at;
        let record = records::PkceBearerCredentialRecord::try_from(credential)?;
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("PkceBearerCredential serialization error: {e}"))?;
        let _: () = self
            .redis
            .with("session_store", "store_bearer_credential")
            .set(
                Self::redis_key_for_bearer(token.secret()),
                serialized,
                Some(Self::expiration(expires_at)),
                None,
                false,
            )
            .await?;
        Ok(())
    }

    async fn get_bearer_credential(
        &self,
        token: &str,
        expected_binding: &PkceBinding,
    ) -> Result<Option<PkceBearerCredential>, SessionStoreError> {
        let maybe_bytes: Option<Bytes> = self
            .redis
            .with("session_store", "get_bearer_credential")
            .get(&Self::redis_key_for_bearer(token))
            .await?;
        records::decode_bearer_credential(maybe_bytes, expected_binding)
    }

    async fn store_authenticated_session(
        &self,
        session_id: &SessionId,
        session: OidcSession,
    ) -> Result<(), SessionStoreError> {
        let ttl_secs = (session.expires_at - chrono::Utc::now()).num_milliseconds();
        let expiration = Expiration::PX(ttl_secs.max(1));

        let record = records::OidcSessionRecord::try_from(session)?;
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("OidcSessionRecord serialization error: {e}"))?;

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

    async fn store_mcp_pending_auth(
        &self,
        state: &str,
        pending: McpPendingAuth,
    ) -> Result<(), SessionStoreError> {
        let record = records::McpPendingAuthRecord::from(pending);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("McpPendingAuth serialization error: {e}"))?;

        let _: () = self
            .redis
            .with("session_store", "store_mcp_pending_auth")
            .set(
                Self::redis_key_for_mcp_pending(state),
                serialized,
                Some(self.pending_login_expiration.clone()),
                None,
                false,
            )
            .await?;

        Ok(())
    }

    async fn take_mcp_pending_auth(
        &self,
        state: &str,
    ) -> Result<Option<McpPendingAuth>, SessionStoreError> {
        let key = Self::redis_key_for_mcp_pending(state);
        let maybe_bytes: Option<Bytes> = self
            .redis
            .with("session_store", "take_mcp_pending_auth")
            .get(&key)
            .await?;

        if let Some(bytes) = maybe_bytes {
            let record: records::McpPendingAuthRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| anyhow!("McpPendingAuth deserialization error: {e}"))?;

            let _: i32 = self
                .redis
                .with("session_store", "del_mcp_pending_auth")
                .del(&key)
                .await?;
            Ok(Some(McpPendingAuth::from(record)))
        } else {
            Ok(None)
        }
    }

    async fn store_mcp_proxy_code(
        &self,
        code: &str,
        entry: McpProxyCodeEntry,
    ) -> Result<(), SessionStoreError> {
        let record = records::McpProxyCodeEntryRecord::from(entry);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| anyhow!("McpProxyCodeEntry serialization error: {e}"))?;

        let _: () = self
            .redis
            .with("session_store", "store_mcp_proxy_code")
            .set(
                Self::redis_key_for_mcp_proxy_code(code),
                serialized,
                Some(self.pending_login_expiration.clone()),
                None,
                false,
            )
            .await?;

        Ok(())
    }

    async fn take_mcp_proxy_code(
        &self,
        code: &str,
    ) -> Result<Option<McpProxyCodeEntry>, SessionStoreError> {
        let key = Self::redis_key_for_mcp_proxy_code(code);
        let maybe_bytes: Option<Bytes> = self
            .redis
            .with("session_store", "take_mcp_proxy_code")
            .get(&key)
            .await?;

        if let Some(bytes) = maybe_bytes {
            let record: records::McpProxyCodeEntryRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| anyhow!("McpProxyCodeEntry deserialization error: {e}"))?;

            let _: i32 = self
                .redis
                .with("session_store", "del_mcp_proxy_code")
                .del(&key)
                .await?;
            Ok(Some(McpProxyCodeEntry::from(record)))
        } else {
            Ok(None)
        }
    }
}

pub struct SqliteSessionStore {
    pool: SqlitePool,
    pending_login_expiration: i64,
    cleanup_cancel: CancellationToken,
}

impl Drop for SqliteSessionStore {
    fn drop(&mut self) {
        self.cleanup_cancel.cancel();
    }
}

impl SqliteSessionStore {
    pub async fn new(
        pool: SqlitePool,
        pending_login_expiration: i64,
        cleanup_interval: Duration,
    ) -> anyhow::Result<Self> {
        Self::init(&pool).await?;
        let cleanup_cancel = CancellationToken::new();
        Self::spawn_expiration_task(pool.clone(), cleanup_interval, cleanup_cancel.clone());
        Ok(Self {
            pool,
            pending_login_expiration,
            cleanup_cancel,
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

                CREATE TABLE IF NOT EXISTS oidc_pkce_pending (
                    state_digest TEXT PRIMARY KEY,
                    value BLOB NOT NULL,
                    expires_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS oidc_pkce_authorization_code (
                    code_digest TEXT PRIMARY KEY,
                    value BLOB NOT NULL,
                    expires_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS oidc_pkce_bearer (
                    token_digest TEXT PRIMARY KEY,
                    value BLOB NOT NULL,
                    expires_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS mcp_pending_auth (
                    state TEXT PRIMARY KEY,
                    value BLOB NOT NULL,
                    expires_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS mcp_proxy_code (
                    code TEXT PRIMARY KEY,
                    value BLOB NOT NULL,
                    expires_at INTEGER NOT NULL
                );
                "#,
            ))
            .await?;

        Ok(())
    }

    fn spawn_expiration_task(
        db_pool: SqlitePool,
        cleanup_interval: Duration,
        cancel: CancellationToken,
    ) {
        task::spawn(
            async move {
                let mut interval = tokio::time::interval(cleanup_interval);

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            tracing::debug!("OIDC cleanup task cancelled");
                            break;
                        }

                        _ = interval.tick() => {
                            let now = Self::current_time();

                            if let Err(e) =
                                Self::cleanup_expired_oidc_pending_login(db_pool.clone(), now).await
                            {
                                error!("Failed to expire oidc pending logins: {}", e);
                            }

                            if let Err(e) =
                                Self::cleanup_expired_oidc_session(db_pool.clone(), now).await
                            {
                                error!("Failed to expire oidc sessions: {}", e);
                            }

                            if let Err(e) =
                                Self::cleanup_expired_pkce_records(db_pool.clone(), now).await
                            {
                                error!("Failed to expire OIDC PKCE records: {}", e);
                            }

                            if let Err(e) =
                                Self::cleanup_expired_mcp_pending_auth(db_pool.clone(), now).await
                            {
                                error!("Failed to expire mcp pending auths: {}", e);
                            }

                            if let Err(e) =
                                Self::cleanup_expired_mcp_proxy_code(db_pool.clone(), now).await
                            {
                                error!("Failed to expire mcp proxy codes: {}", e);
                            }
                        }
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

    async fn cleanup_expired_pkce_records(
        pool: SqlitePool,
        current_time: i64,
    ) -> anyhow::Result<()> {
        let current_time = current_time * 1000;
        for table in [
            "oidc_pkce_pending",
            "oidc_pkce_authorization_code",
            "oidc_pkce_bearer",
        ] {
            pool.with_rw("session_store", "cleanup_expired_pkce_records")
                .execute(
                    sqlx::query(&format!("DELETE FROM {table} WHERE expires_at <= ?"))
                        .bind(current_time),
                )
                .await?;
        }
        Ok(())
    }

    async fn cleanup_expired_mcp_pending_auth(
        pool: SqlitePool,
        current_time: i64,
    ) -> anyhow::Result<()> {
        let query =
            sqlx::query("DELETE FROM mcp_pending_auth WHERE expires_at < ?;").bind(current_time);

        pool.with_rw("session_store", "cleanup_expired_mcp_pending_auth")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn cleanup_expired_mcp_proxy_code(
        pool: SqlitePool,
        current_time: i64,
    ) -> anyhow::Result<()> {
        let query =
            sqlx::query("DELETE FROM mcp_proxy_code WHERE expires_at < ?;").bind(current_time);

        pool.with_rw("session_store", "cleanup_expired_mcp_proxy_code")
            .execute(query)
            .await?;

        Ok(())
    }

    pub fn current_time() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn current_time_millis() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn store_pending_oidc_login(
        &self,
        state: &str,
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
            .with_rw("session_store", "take_pending_oidc_login")
            .fetch_optional(
                sqlx::query(
                    "DELETE FROM oidc_pending_login WHERE state = ? AND expires_at > ? RETURNING value",
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
            Ok(Some(login))
        } else {
            Ok(None)
        }
    }

    async fn store_pending_pkce_login(
        &self,
        upstream_state: &str,
        login: PendingPkceLogin,
    ) -> Result<(), SessionStoreError> {
        let expires_at = login.expires_at.timestamp_millis();
        let serialized =
            golem_common::serialization::serialize(&records::PendingPkceLoginRecord::from(login))
                .map_err(|e| anyhow!("PendingPkceLogin serialization error: {e}"))?;
        self.pool
            .with_rw("session_store", "store_pending_pkce_login")
            .execute(
                sqlx::query("INSERT OR REPLACE INTO oidc_pkce_pending (state_digest, value, expires_at) VALUES (?, ?, ?)")
                    .bind(credential_digest("pending", upstream_state))
                    .bind(serialized)
                    .bind(expires_at),
            )
            .await?;
        Ok(())
    }

    async fn take_pending_pkce_login(
        &self,
        upstream_state: &str,
    ) -> Result<Option<PendingPkceLogin>, SessionStoreError> {
        let row = self
            .pool
            .with_rw("session_store", "take_pending_pkce_login")
            .fetch_optional(
                sqlx::query("DELETE FROM oidc_pkce_pending WHERE state_digest = ? AND expires_at > ? RETURNING value")
                    .bind(credential_digest("pending", upstream_state))
                    .bind(Self::current_time_millis()),
            )
            .await?;
        records::decode_pending_pkce(row.map(|row| Bytes::from(row.get::<Vec<u8>, _>(0))))
    }

    async fn store_authorization_code(
        &self,
        code: &AuthorizationCode,
        entry: PkceAuthorizationCode,
    ) -> Result<(), SessionStoreError> {
        let expires_at = entry.expires_at.timestamp_millis();
        let serialized = golem_common::serialization::serialize(
            &records::PkceAuthorizationCodeRecord::try_from(entry)?,
        )
        .map_err(|e| anyhow!("PkceAuthorizationCode serialization error: {e}"))?;
        self.pool
            .with_rw("session_store", "store_authorization_code")
            .execute(
                sqlx::query("INSERT OR REPLACE INTO oidc_pkce_authorization_code (code_digest, value, expires_at) VALUES (?, ?, ?)")
                    .bind(credential_digest("authorization-code", code.secret()))
                    .bind(serialized)
                    .bind(expires_at),
            )
            .await?;
        Ok(())
    }

    async fn take_authorization_code(
        &self,
        code: &str,
    ) -> Result<Option<PkceAuthorizationCode>, SessionStoreError> {
        let row = self
            .pool
            .with_rw("session_store", "take_authorization_code")
            .fetch_optional(
                sqlx::query("DELETE FROM oidc_pkce_authorization_code WHERE code_digest = ? AND expires_at > ? RETURNING value")
                    .bind(credential_digest("authorization-code", code))
                    .bind(Self::current_time_millis()),
            )
            .await?;
        records::decode_authorization_code(row.map(|row| Bytes::from(row.get::<Vec<u8>, _>(0))))
    }

    async fn store_bearer_credential(
        &self,
        token: &BearerToken,
        credential: PkceBearerCredential,
    ) -> Result<(), SessionStoreError> {
        let expires_at = credential.expires_at.timestamp_millis();
        let serialized = golem_common::serialization::serialize(
            &records::PkceBearerCredentialRecord::try_from(credential)?,
        )
        .map_err(|e| anyhow!("PkceBearerCredential serialization error: {e}"))?;
        self.pool
            .with_rw("session_store", "store_bearer_credential")
            .execute(
                sqlx::query("INSERT OR REPLACE INTO oidc_pkce_bearer (token_digest, value, expires_at) VALUES (?, ?, ?)")
                    .bind(credential_digest("bearer", token.secret()))
                    .bind(serialized)
                    .bind(expires_at),
            )
            .await?;
        Ok(())
    }

    async fn get_bearer_credential(
        &self,
        token: &str,
        expected_binding: &PkceBinding,
    ) -> Result<Option<PkceBearerCredential>, SessionStoreError> {
        let row = self
            .pool
            .with_ro("session_store", "get_bearer_credential")
            .fetch_optional(
                sqlx::query(
                    "SELECT value FROM oidc_pkce_bearer WHERE token_digest = ? AND expires_at > ?",
                )
                .bind(credential_digest("bearer", token))
                .bind(Self::current_time_millis()),
            )
            .await?;
        records::decode_bearer_credential(
            row.map(|row| Bytes::from(row.get::<Vec<u8>, _>(0))),
            expected_binding,
        )
    }

    async fn store_authenticated_session(
        &self,
        session_id: &SessionId,
        session: OidcSession,
    ) -> Result<(), SessionStoreError> {
        let expires_at = session.expires_at.timestamp();

        let record = records::OidcSessionRecord::try_from(session)?;
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

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

    async fn store_mcp_pending_auth(
        &self,
        state: &str,
        pending: McpPendingAuth,
    ) -> Result<(), SessionStoreError> {
        let record = records::McpPendingAuthRecord::from(pending);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

        let expiry = Utc::now()
            .checked_add_signed(TimeDelta::seconds(self.pending_login_expiration))
            .ok_or_else(|| anyhow!("Failed to compute expiry"))?
            .timestamp();

        self.pool
            .with_rw("session_store", "store_mcp_pending_auth")
            .execute(
                sqlx::query("INSERT OR REPLACE INTO mcp_pending_auth (state, value, expires_at) VALUES (?, ?, ?)")
                    .bind(state)
                    .bind(serialized)
                    .bind(expiry),
            )
            .await?;

        Ok(())
    }

    async fn take_mcp_pending_auth(
        &self,
        state: &str,
    ) -> Result<Option<McpPendingAuth>, SessionStoreError> {
        let row = self
            .pool
            .with_rw("session_store", "take_mcp_pending_auth")
            .fetch_optional(
                sqlx::query(
                    "DELETE FROM mcp_pending_auth WHERE state = ? AND expires_at > ? RETURNING value",
                )
                .bind(state)
                .bind(Self::current_time()),
            )
            .await?;

        if let Some(row) = row {
            let bytes: Vec<u8> = row.get(0);
            let record: records::McpPendingAuthRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

            Ok(Some(McpPendingAuth::from(record)))
        } else {
            Ok(None)
        }
    }

    async fn store_mcp_proxy_code(
        &self,
        code: &str,
        entry: McpProxyCodeEntry,
    ) -> Result<(), SessionStoreError> {
        let record = records::McpProxyCodeEntryRecord::from(entry);
        let serialized = golem_common::serialization::serialize(&record)
            .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

        let expiry = Utc::now()
            .checked_add_signed(TimeDelta::seconds(self.pending_login_expiration))
            .ok_or_else(|| anyhow!("Failed to compute expiry"))?
            .timestamp();

        self.pool
            .with_rw("session_store", "store_mcp_proxy_code")
            .execute(
                sqlx::query("INSERT OR REPLACE INTO mcp_proxy_code (code, value, expires_at) VALUES (?, ?, ?)")
                    .bind(code)
                    .bind(serialized)
                    .bind(expiry),
            )
            .await?;

        Ok(())
    }

    async fn take_mcp_proxy_code(
        &self,
        code: &str,
    ) -> Result<Option<McpProxyCodeEntry>, SessionStoreError> {
        let row = self
            .pool
            .with_rw("session_store", "take_mcp_proxy_code")
            .fetch_optional(
                sqlx::query(
                    "DELETE FROM mcp_proxy_code WHERE code = ? AND expires_at > ? RETURNING value",
                )
                .bind(code)
                .bind(Self::current_time()),
            )
            .await?;

        if let Some(row) = row {
            let bytes: Vec<u8> = row.get(0);
            let record: records::McpProxyCodeEntryRecord =
                golem_common::serialization::deserialize(&bytes)
                    .map_err(|e| SessionStoreError::InternalError(anyhow::anyhow!(e)))?;

            Ok(Some(McpProxyCodeEntry::from(record)))
        } else {
            Ok(None)
        }
    }
}

mod records {
    use super::SessionStoreError;
    use crate::custom_api::model::OidcSession;
    use crate::custom_api::oidc::model::{
        McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin, PendingPkceLogin,
        PkceAuthorizationCode, PkceBearerCredential, PkceBinding,
    };
    use anyhow::anyhow;
    use bytes::Bytes;
    use chrono::{DateTime, Utc};
    use desert_rust::BinaryCodec;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::security_scheme::SecuritySchemeId;
    use golem_common::model::security_scheme::SecuritySchemeRevision;
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

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct PkceBindingRecord {
        pub security_scheme_id: SecuritySchemeId,
        pub security_scheme_revision: SecuritySchemeRevision,
        pub environment_id: EnvironmentId,
        pub api_origin: String,
    }

    impl From<PkceBinding> for PkceBindingRecord {
        fn from(value: PkceBinding) -> Self {
            Self {
                security_scheme_id: value.security_scheme_id,
                security_scheme_revision: value.security_scheme_revision,
                environment_id: value.environment_id,
                api_origin: value.api_origin,
            }
        }
    }

    impl From<PkceBindingRecord> for PkceBinding {
        fn from(value: PkceBindingRecord) -> Self {
            Self {
                security_scheme_id: value.security_scheme_id,
                security_scheme_revision: value.security_scheme_revision,
                environment_id: value.environment_id,
                api_origin: value.api_origin,
            }
        }
    }

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct PendingPkceLoginRecord {
        pub binding: PkceBindingRecord,
        pub redirect_uri: String,
        pub frontend_state: String,
        pub code_challenge: String,
        pub upstream_nonce: String,
        pub expires_at: DateTime<Utc>,
    }

    impl From<PendingPkceLogin> for PendingPkceLoginRecord {
        fn from(value: PendingPkceLogin) -> Self {
            Self {
                binding: value.binding.into(),
                redirect_uri: value.redirect_uri,
                frontend_state: value.frontend_state,
                code_challenge: value.code_challenge,
                upstream_nonce: value.upstream_nonce.secret().clone(),
                expires_at: value.expires_at,
            }
        }
    }

    impl From<PendingPkceLoginRecord> for PendingPkceLogin {
        fn from(value: PendingPkceLoginRecord) -> Self {
            Self {
                binding: value.binding.into(),
                redirect_uri: value.redirect_uri,
                frontend_state: value.frontend_state,
                code_challenge: value.code_challenge,
                upstream_nonce: Nonce::new(value.upstream_nonce),
                expires_at: value.expires_at,
            }
        }
    }

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct PkceAuthorizationCodeRecord {
        pub binding: PkceBindingRecord,
        pub principal: OidcSessionRecord,
        pub redirect_uri: String,
        pub code_challenge: String,
        pub expires_at: DateTime<Utc>,
    }

    impl TryFrom<PkceAuthorizationCode> for PkceAuthorizationCodeRecord {
        type Error = SessionStoreError;

        fn try_from(value: PkceAuthorizationCode) -> Result<Self, Self::Error> {
            Ok(Self {
                binding: value.binding.into(),
                principal: value.principal.try_into()?,
                redirect_uri: value.redirect_uri,
                code_challenge: value.code_challenge,
                expires_at: value.expires_at,
            })
        }
    }

    impl TryFrom<PkceAuthorizationCodeRecord> for PkceAuthorizationCode {
        type Error = SessionStoreError;

        fn try_from(value: PkceAuthorizationCodeRecord) -> Result<Self, Self::Error> {
            Ok(Self {
                binding: value.binding.into(),
                principal: value.principal.try_into()?,
                redirect_uri: value.redirect_uri,
                code_challenge: value.code_challenge,
                expires_at: value.expires_at,
            })
        }
    }

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct PkceBearerCredentialRecord {
        pub binding: PkceBindingRecord,
        pub principal: OidcSessionRecord,
        pub expires_at: DateTime<Utc>,
    }

    impl TryFrom<PkceBearerCredential> for PkceBearerCredentialRecord {
        type Error = SessionStoreError;

        fn try_from(value: PkceBearerCredential) -> Result<Self, Self::Error> {
            Ok(Self {
                binding: value.binding.into(),
                principal: value.principal.try_into()?,
                expires_at: value.expires_at,
            })
        }
    }

    impl TryFrom<PkceBearerCredentialRecord> for PkceBearerCredential {
        type Error = SessionStoreError;

        fn try_from(value: PkceBearerCredentialRecord) -> Result<Self, Self::Error> {
            Ok(Self {
                binding: value.binding.into(),
                principal: value.principal.try_into()?,
                expires_at: value.expires_at,
            })
        }
    }

    pub fn decode_pending_pkce(
        bytes: Option<Bytes>,
    ) -> Result<Option<PendingPkceLogin>, SessionStoreError> {
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let record: PendingPkceLoginRecord = golem_common::serialization::deserialize(&bytes)
            .map_err(|e| anyhow!("PendingPkceLogin deserialization error: {e}"))?;
        let login = PendingPkceLogin::from(record);
        Ok((login.expires_at > Utc::now()).then_some(login))
    }

    pub fn decode_authorization_code(
        bytes: Option<Bytes>,
    ) -> Result<Option<PkceAuthorizationCode>, SessionStoreError> {
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let record: PkceAuthorizationCodeRecord = golem_common::serialization::deserialize(&bytes)
            .map_err(|e| anyhow!("PkceAuthorizationCode deserialization error: {e}"))?;
        let code = PkceAuthorizationCode::try_from(record)?;
        Ok((code.expires_at > Utc::now()).then_some(code))
    }

    pub fn decode_bearer_credential(
        bytes: Option<Bytes>,
        expected_binding: &PkceBinding,
    ) -> Result<Option<PkceBearerCredential>, SessionStoreError> {
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let record: PkceBearerCredentialRecord =
            golem_common::serialization::deserialize(&bytes)
                .map_err(|e| anyhow!("PkceBearerCredential deserialization error: {e}"))?;
        let credential = PkceBearerCredential::try_from(record)?;
        Ok(
            (credential.expires_at > Utc::now() && credential.binding == *expected_binding)
                .then_some(credential),
        )
    }

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct McpPendingAuthRecord {
        pub client_redirect_uri: String,
        pub client_state: Option<String>,
    }

    impl From<McpPendingAuth> for McpPendingAuthRecord {
        fn from(value: McpPendingAuth) -> Self {
            Self {
                client_redirect_uri: value.client_redirect_uri,
                client_state: value.client_state,
            }
        }
    }

    impl From<McpPendingAuthRecord> for McpPendingAuth {
        fn from(value: McpPendingAuthRecord) -> Self {
            Self {
                client_redirect_uri: value.client_redirect_uri,
                client_state: value.client_state,
            }
        }
    }

    #[derive(Debug, BinaryCodec)]
    #[desert(evolution())]
    pub struct McpProxyCodeEntryRecord {
        pub id_token: String,
        pub refresh_token: Option<String>,
        pub expires_in: Option<u64>,
        pub token_type: String,
    }

    impl From<McpProxyCodeEntry> for McpProxyCodeEntryRecord {
        fn from(value: McpProxyCodeEntry) -> Self {
            Self {
                id_token: value.id_token,
                refresh_token: value.refresh_token,
                expires_in: value.expires_in,
                token_type: value.token_type,
            }
        }
    }

    impl From<McpProxyCodeEntryRecord> for McpProxyCodeEntry {
        fn from(value: McpProxyCodeEntryRecord) -> Self {
            Self {
                id_token: value.id_token,
                refresh_token: value.refresh_token,
                expires_in: value.expires_in,
                token_type: value.token_type,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{credential_digest, records};
    use crate::custom_api::OidcSession;
    use crate::custom_api::oidc::model::{AuthorizationCode, BearerToken, PendingOidcLogin};
    use chrono::{TimeDelta, Utc};
    use golem_common::model::security_scheme::SecuritySchemeId;
    use openidconnect::core::CoreIdTokenClaims;
    use openidconnect::{
        Audience, EmptyAdditionalClaims, IssuerUrl, Nonce, Scope, StandardClaims, SubjectIdentifier,
    };
    use std::collections::HashSet;
    use std::str::FromStr;
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

    #[test]
    fn opaque_credentials_are_random_redacted_and_domain_separated() {
        let first = AuthorizationCode::generate();
        let second = AuthorizationCode::generate();
        assert_ne!(first, second);
        assert_eq!(format!("{first:?}"), "*******");
        assert_eq!(
            BearerToken::from_str(first.secret()).unwrap().secret(),
            first.secret()
        );

        let code_digest = credential_digest("authorization-code", first.secret());
        let bearer_digest = credential_digest("bearer", first.secret());
        assert_ne!(code_digest, first.secret());
        assert_ne!(code_digest, bearer_digest);
        assert_eq!(code_digest.len(), 64);
    }
}
