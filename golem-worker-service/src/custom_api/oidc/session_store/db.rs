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

use super::{SessionStore, SessionStoreError, credential_digest, records};
use crate::config::{PostgresSessionStoreConfig, SqliteSessionStoreConfig};
use crate::custom_api::model::OidcSession;
use crate::custom_api::oidc::model::{
    AuthorizationCode, BearerToken, McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin,
    PendingPkceLogin, PkceAuthorizationCode, PkceBearerCredential, PkceBinding, SessionId,
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{TimeDelta, Utc};
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::include_dir;
use sqlx::Row;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, error};

static DB_MIGRATIONS: include_dir::Dir =
    include_dir!("$CARGO_MANIFEST_DIR/db/migration/session_store");

const SVC_NAME: &str = "session_store";

const DELETE_EXPIRED_SQL: [(&str, &str); 7] = [
    (
        "oidc_pending_login",
        "DELETE FROM oidc_pending_login WHERE expires_at <= $1",
    ),
    (
        "oidc_session",
        "DELETE FROM oidc_session WHERE expires_at <= $1",
    ),
    (
        "oidc_pkce_pending",
        "DELETE FROM oidc_pkce_pending WHERE expires_at <= $1",
    ),
    (
        "oidc_pkce_authorization_code",
        "DELETE FROM oidc_pkce_authorization_code WHERE expires_at <= $1",
    ),
    (
        "oidc_pkce_bearer",
        "DELETE FROM oidc_pkce_bearer WHERE expires_at <= $1",
    ),
    (
        "mcp_pending_auth",
        "DELETE FROM mcp_pending_auth WHERE expires_at <= $1",
    ),
    (
        "mcp_proxy_code",
        "DELETE FROM mcp_proxy_code WHERE expires_at <= $1",
    ),
];

pub type SqliteSessionStore = DbSessionStore<SqlitePool>;
pub type PostgresSessionStore = DbSessionStore<PostgresPool>;

/// Session store on a relational database. Every `expires_at` column holds Unix milliseconds.
/// Reads and takes ignore expired records, and a background task deletes them periodically.
pub struct DbSessionStore<P> {
    pool: P,
    pending_login_expiration: TimeDelta,
    cleanup_cancel: CancellationToken,
}

impl<P> Drop for DbSessionStore<P> {
    fn drop(&mut self) {
        self.cleanup_cancel.cancel();
    }
}

impl DbSessionStore<SqlitePool> {
    pub async fn configured(config: &SqliteSessionStoreConfig) -> anyhow::Result<Self> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::sqlite::migrate(
            &config.sqlite_config,
            migrations.sqlite_migrations(),
        )
        .await
        .context("SQLite session store migration failed")?;

        let pool = SqlitePool::configured(&config.sqlite_config)
            .await
            .context("SQLite session store pool initialization failed")?;

        Self::new(
            pool,
            config.pending_login_expiration,
            config.cleanup_interval,
        )
    }
}

impl DbSessionStore<PostgresPool> {
    pub async fn configured(config: &PostgresSessionStoreConfig) -> anyhow::Result<Self> {
        let postgres_config = config.postgres_config_with_schema();

        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::postgres::migrate(
            &postgres_config,
            migrations.postgres_migrations(),
        )
        .await
        .context("Postgres session store migration failed")?;

        let pool = PostgresPool::configured(&postgres_config)
            .await
            .context("Postgres session store pool initialization failed")?;

        Self::new(
            pool,
            config.pending_login_expiration,
            config.cleanup_interval,
        )
    }
}

fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

fn decode_record<R: desert_rust::BinaryDeserializer>(
    bytes: &[u8],
    name: &str,
) -> Result<R, SessionStoreError> {
    golem_common::serialization::deserialize(bytes)
        .map_err(|e| SessionStoreError::InternalError(anyhow!("{name} deserialization error: {e}")))
}

fn encode_record<R: desert_rust::BinarySerializer>(
    record: &R,
    name: &str,
) -> Result<Vec<u8>, SessionStoreError> {
    golem_common::serialization::serialize(record)
        .map_err(|e| SessionStoreError::InternalError(anyhow!("{name} serialization error: {e}")))
}

#[trait_gen(SqlitePool -> SqlitePool, PostgresPool)]
impl DbSessionStore<SqlitePool> {
    fn new(
        pool: SqlitePool,
        pending_login_expiration: Duration,
        cleanup_interval: Duration,
    ) -> anyhow::Result<Self> {
        let pending_login_expiration = TimeDelta::from_std(pending_login_expiration)
            .context("pending_login_expiration is out of range")?;
        let cleanup_cancel = CancellationToken::new();
        Self::spawn_cleanup_task(pool.clone(), cleanup_interval, cleanup_cancel.clone());
        Ok(Self {
            pool,
            pending_login_expiration,
            cleanup_cancel,
        })
    }

    fn spawn_cleanup_task(pool: SqlitePool, cleanup_interval: Duration, cancel: CancellationToken) {
        tokio::spawn(
            async move {
                let mut interval = tokio::time::interval(cleanup_interval);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = interval.tick() => Self::delete_expired(&pool, now_millis()).await,
                    }
                }
            }
            .in_current_span(),
        );
    }

    /// Deletes the expired records of every table. A failure on one table does not stop the
    /// cleanup of the others.
    async fn delete_expired(pool: &SqlitePool, now_millis: i64) {
        for (table, sql) in DELETE_EXPIRED_SQL {
            if let Err(err) = pool
                .with_rw(SVC_NAME, "delete_expired")
                .execute(sqlx::query(sql).bind(now_millis))
                .await
            {
                error!(table, error = %err, "Failed to delete expired session store records");
            }
        }
    }

    fn pending_login_expires_at(&self) -> Result<i64, SessionStoreError> {
        Utc::now()
            .checked_add_signed(self.pending_login_expiration)
            .map(|expires_at| expires_at.timestamp_millis())
            .ok_or_else(|| anyhow!("Failed to compute expiry").into())
    }

    async fn upsert(
        &self,
        api_name: &'static str,
        sql: &'static str,
        key: String,
        value: Vec<u8>,
        expires_at: i64,
    ) -> Result<(), SessionStoreError> {
        self.pool
            .with_rw(SVC_NAME, api_name)
            .execute(sqlx::query(sql).bind(key).bind(value).bind(expires_at))
            .await?;
        Ok(())
    }

    /// Atomically deletes and returns the unexpired record for a key taken from a request.
    async fn take(
        &self,
        api_name: &'static str,
        sql: &'static str,
        key: String,
    ) -> Result<Option<Bytes>, SessionStoreError> {
        // Postgres rejects NUL in TEXT values, so no stored key contains one. Answer without
        // querying, so that such a request is a miss on every engine instead of a database error.
        if key.contains('\0') {
            return Ok(None);
        }
        let row = self
            .pool
            .with_rw(SVC_NAME, api_name)
            .fetch_optional(sqlx::query(sql).bind(key).bind(now_millis()))
            .await?;
        Ok(row.map(|row| Bytes::from(row.get::<Vec<u8>, _>(0))))
    }
}

#[trait_gen(SqlitePool -> SqlitePool, PostgresPool)]
#[async_trait]
impl SessionStore for DbSessionStore<SqlitePool> {
    async fn store_pending_oidc_login(
        &self,
        state: &str,
        login: PendingOidcLogin,
    ) -> Result<(), SessionStoreError> {
        let value = encode_record(
            &records::PendingOidcLoginRecord::from(login),
            "PendingOidcLogin",
        )?;
        self.upsert(
            "store_pending_oidc_login",
            "INSERT INTO oidc_pending_login (state, value, expires_at) VALUES ($1, $2, $3)
             ON CONFLICT (state) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            state.to_string(),
            value,
            self.pending_login_expires_at()?,
        )
        .await
    }

    async fn take_pending_oidc_login(
        &self,
        state: &str,
    ) -> Result<Option<PendingOidcLogin>, SessionStoreError> {
        let bytes = self
            .take(
                "take_pending_oidc_login",
                "DELETE FROM oidc_pending_login WHERE state = $1 AND expires_at > $2 RETURNING value",
                state.to_string(),
            )
            .await?;
        bytes
            .map(|bytes| {
                decode_record::<records::PendingOidcLoginRecord>(&bytes, "PendingOidcLogin")
                    .map(PendingOidcLogin::from)
            })
            .transpose()
    }

    async fn store_pending_pkce_login(
        &self,
        upstream_state: &str,
        login: PendingPkceLogin,
    ) -> Result<(), SessionStoreError> {
        let expires_at = login.expires_at.timestamp_millis();
        let value = encode_record(
            &records::PendingPkceLoginRecord::from(login),
            "PendingPkceLogin",
        )?;
        self.upsert(
            "store_pending_pkce_login",
            "INSERT INTO oidc_pkce_pending (state_digest, value, expires_at) VALUES ($1, $2, $3)
             ON CONFLICT (state_digest) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            credential_digest("pending", upstream_state),
            value,
            expires_at,
        )
        .await
    }

    async fn take_pending_pkce_login(
        &self,
        upstream_state: &str,
    ) -> Result<Option<PendingPkceLogin>, SessionStoreError> {
        let bytes = self
            .take(
                "take_pending_pkce_login",
                "DELETE FROM oidc_pkce_pending WHERE state_digest = $1 AND expires_at > $2 RETURNING value",
                credential_digest("pending", upstream_state),
            )
            .await?;
        records::decode_pending_pkce(bytes)
    }

    async fn store_authorization_code(
        &self,
        code: &AuthorizationCode,
        entry: PkceAuthorizationCode,
    ) -> Result<(), SessionStoreError> {
        let expires_at = entry.expires_at.timestamp_millis();
        let value = encode_record(
            &records::PkceAuthorizationCodeRecord::try_from(entry)?,
            "PkceAuthorizationCode",
        )?;
        self.upsert(
            "store_authorization_code",
            "INSERT INTO oidc_pkce_authorization_code (code_digest, value, expires_at) VALUES ($1, $2, $3)
             ON CONFLICT (code_digest) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            credential_digest("authorization-code", code.secret()),
            value,
            expires_at,
        )
        .await
    }

    async fn take_authorization_code(
        &self,
        code: &str,
    ) -> Result<Option<PkceAuthorizationCode>, SessionStoreError> {
        let bytes = self
            .take(
                "take_authorization_code",
                "DELETE FROM oidc_pkce_authorization_code WHERE code_digest = $1 AND expires_at > $2 RETURNING value",
                credential_digest("authorization-code", code),
            )
            .await?;
        records::decode_authorization_code(bytes)
    }

    async fn store_bearer_credential(
        &self,
        token: &BearerToken,
        credential: PkceBearerCredential,
    ) -> Result<(), SessionStoreError> {
        let expires_at = credential.expires_at.timestamp_millis();
        let value = encode_record(
            &records::PkceBearerCredentialRecord::try_from(credential)?,
            "PkceBearerCredential",
        )?;
        self.upsert(
            "store_bearer_credential",
            "INSERT INTO oidc_pkce_bearer (token_digest, value, expires_at) VALUES ($1, $2, $3)
             ON CONFLICT (token_digest) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            credential_digest("bearer", token.secret()),
            value,
            expires_at,
        )
        .await
    }

    async fn get_bearer_credential(
        &self,
        token: &str,
        expected_binding: &PkceBinding,
    ) -> Result<Option<PkceBearerCredential>, SessionStoreError> {
        let row = self
            .pool
            .with_ro(SVC_NAME, "get_bearer_credential")
            .fetch_optional(
                sqlx::query(
                    "SELECT value FROM oidc_pkce_bearer WHERE token_digest = $1 AND expires_at > $2",
                )
                .bind(credential_digest("bearer", token))
                .bind(now_millis()),
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
        let expires_at = session.expires_at.timestamp_millis();
        let value = encode_record(
            &records::OidcSessionRecord::try_from(session)?,
            "OidcSession",
        )?;
        self.pool
            .with_rw(SVC_NAME, "store_authenticated_session")
            .execute(
                sqlx::query(
                    "INSERT INTO oidc_session (session_id, value, expires_at) VALUES ($1, $2, $3)
                     ON CONFLICT (session_id) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
                )
                .bind(session_id.0)
                .bind(value)
                .bind(expires_at),
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
            .with_ro(SVC_NAME, "get_authenticated_session")
            .fetch_optional(
                sqlx::query(
                    "SELECT value FROM oidc_session WHERE session_id = $1 AND expires_at > $2",
                )
                .bind(session_id.0)
                .bind(now_millis()),
            )
            .await?;
        row.map(|row| {
            let bytes: Vec<u8> = row.get(0);
            decode_record::<records::OidcSessionRecord>(&bytes, "OidcSession")
                .and_then(OidcSession::try_from)
        })
        .transpose()
    }

    async fn store_mcp_pending_auth(
        &self,
        state: &str,
        pending: McpPendingAuth,
    ) -> Result<(), SessionStoreError> {
        let value = encode_record(
            &records::McpPendingAuthRecord::from(pending),
            "McpPendingAuth",
        )?;
        self.upsert(
            "store_mcp_pending_auth",
            "INSERT INTO mcp_pending_auth (state, value, expires_at) VALUES ($1, $2, $3)
             ON CONFLICT (state) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            state.to_string(),
            value,
            self.pending_login_expires_at()?,
        )
        .await
    }

    async fn take_mcp_pending_auth(
        &self,
        state: &str,
    ) -> Result<Option<McpPendingAuth>, SessionStoreError> {
        let bytes = self
            .take(
                "take_mcp_pending_auth",
                "DELETE FROM mcp_pending_auth WHERE state = $1 AND expires_at > $2 RETURNING value",
                state.to_string(),
            )
            .await?;
        bytes
            .map(|bytes| {
                decode_record::<records::McpPendingAuthRecord>(&bytes, "McpPendingAuth")
                    .map(McpPendingAuth::from)
            })
            .transpose()
    }

    async fn store_mcp_proxy_code(
        &self,
        code: &str,
        entry: McpProxyCodeEntry,
    ) -> Result<(), SessionStoreError> {
        let value = encode_record(
            &records::McpProxyCodeEntryRecord::from(entry),
            "McpProxyCodeEntry",
        )?;
        self.upsert(
            "store_mcp_proxy_code",
            "INSERT INTO mcp_proxy_code (code, value, expires_at) VALUES ($1, $2, $3)
             ON CONFLICT (code) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at",
            code.to_string(),
            value,
            self.pending_login_expires_at()?,
        )
        .await
    }

    async fn take_mcp_proxy_code(
        &self,
        code: &str,
    ) -> Result<Option<McpProxyCodeEntry>, SessionStoreError> {
        let bytes = self
            .take(
                "take_mcp_proxy_code",
                "DELETE FROM mcp_proxy_code WHERE code = $1 AND expires_at > $2 RETURNING value",
                code.to_string(),
            )
            .await?;
        bytes
            .map(|bytes| {
                decode_record::<records::McpProxyCodeEntryRecord>(&bytes, "McpProxyCodeEntry")
                    .map(McpProxyCodeEntry::from)
            })
            .transpose()
    }
}
