// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use crate::repo::model::mcp_oauth::{
    McpOAuthAuthorization, McpOAuthExchangeClaim, McpOAuthFlowSecrets, McpOAuthGrant,
    McpOAuthGrantKey, McpOAuthGrantStatus, McpOAuthRefreshClaim, McpOAuthTokens,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult, SqlDateTime};
use indoc::indoc;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::FromRow;
use uuid::Uuid;

#[async_trait]
pub trait McpOAuthGrantRepo: Send + Sync {
    async fn begin_authorization(
        &self,
        key: McpOAuthGrantKey,
        state_hash: Vec<u8>,
        expires_at: SqlDateTime,
        flow: McpOAuthFlowSecrets,
    ) -> RepoResult<McpOAuthAuthorization>;

    async fn claim_callback(
        &self,
        environment_id: Uuid,
        state_hash: &[u8],
        now: SqlDateTime,
    ) -> RepoResult<Option<McpOAuthExchangeClaim>>;

    async fn publish_exchange(
        &self,
        key: &McpOAuthGrantKey,
        generation: Uuid,
        tokens: McpOAuthTokens,
    ) -> RepoResult<bool>;

    async fn load(&self, key: &McpOAuthGrantKey) -> RepoResult<Option<McpOAuthGrant>>;

    async fn claim_refresh(
        &self,
        key: &McpOAuthGrantKey,
        expected_generation: Uuid,
    ) -> RepoResult<Option<McpOAuthRefreshClaim>>;

    async fn publish_refresh(
        &self,
        key: &McpOAuthGrantKey,
        generation: Uuid,
        tokens: McpOAuthTokens,
    ) -> RepoResult<bool>;

    async fn fail_exchange(&self, key: &McpOAuthGrantKey, generation: Uuid) -> RepoResult<bool>;
    async fn fail_refresh(&self, key: &McpOAuthGrantKey, generation: Uuid) -> RepoResult<bool>;
    async fn expire_access_token(
        &self,
        key: &McpOAuthGrantKey,
        expected_generation: Uuid,
        now: SqlDateTime,
    ) -> RepoResult<bool>;
    async fn revoke(&self, key: &McpOAuthGrantKey) -> RepoResult<McpOAuthAuthorization>;
}

pub struct DbMcpOAuthGrantRepo<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> DbMcpOAuthGrantRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    fn ro(&self, operation: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro("mcp-oauth-grant", operation)
    }

    fn rw(&self, operation: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw("mcp-oauth-grant", operation)
    }
}

#[derive(FromRow)]
struct GrantRow {
    environment_id: Uuid,
    security_scheme_id: Uuid,
    security_scheme_revision: i64,
    credential_owner_account_id: Uuid,
    resource_url: String,
    generation: Uuid,
    status: String,
    flow_secrets: Option<Vec<u8>>,
    token_secrets: Option<Vec<u8>>,
}

fn encode<T: Serialize>(value: &T) -> RepoResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| RepoError::InternalError(error.into()))
}

fn decode<T: DeserializeOwned>(value: Vec<u8>) -> RepoResult<T> {
    serde_json::from_slice(&value)
        .map_err(|_| RepoError::InternalError(anyhow::anyhow!("invalid private OAuth payload")))
}

impl GrantRow {
    fn key(&self) -> McpOAuthGrantKey {
        McpOAuthGrantKey {
            environment_id: self.environment_id,
            security_scheme_id: self.security_scheme_id,
            security_scheme_revision: self.security_scheme_revision,
            credential_owner_account_id: self.credential_owner_account_id,
            resource_url: self.resource_url.clone(),
        }
    }

    fn into_grant(self) -> RepoResult<McpOAuthGrant> {
        let key = self.key();
        let status = McpOAuthGrantStatus::parse(&self.status).map_err(RepoError::InternalError)?;
        let tokens = if status == McpOAuthGrantStatus::Granted {
            self.token_secrets.map(decode).transpose()?
        } else {
            None
        };
        Ok(McpOAuthGrant {
            key,
            generation: self.generation,
            status,
            tokens,
        })
    }
}

const RETURNING: &str = "environment_id, security_scheme_id, security_scheme_revision, credential_owner_account_id, resource_url, generation, status, flow_secrets, token_secrets";

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl McpOAuthGrantRepo for DbMcpOAuthGrantRepo<PostgresPool> {
    async fn begin_authorization(
        &self,
        key: McpOAuthGrantKey,
        state_hash: Vec<u8>,
        expires_at: SqlDateTime,
        flow: McpOAuthFlowSecrets,
    ) -> RepoResult<McpOAuthAuthorization> {
        let generation = Uuid::new_v4();
        let flow = encode(&flow)?;
        self.rw("begin_authorization")
            .execute(
                sqlx::query(indoc! {r#"
                    INSERT INTO mcp_oauth_grants
                        (environment_id, security_scheme_id, security_scheme_revision, credential_owner_account_id,
                         resource_url, generation, status, state_hash, consent_expires_at, flow_secrets, token_secrets)
                    VALUES ($1, $2, $3, $4, $5, $6, 'pending-consent', $7, $8, $9, NULL)
                    ON CONFLICT (environment_id, security_scheme_id, security_scheme_revision,
                                 credential_owner_account_id, resource_url) DO UPDATE SET
                        generation = $6, status = 'pending-consent', state_hash = $7,
                        consent_expires_at = $8, flow_secrets = $9, token_secrets = NULL
                "#})
                .bind(key.environment_id)
                .bind(key.security_scheme_id)
                .bind(key.security_scheme_revision)
                .bind(key.credential_owner_account_id)
                .bind(&key.resource_url)
                .bind(generation)
                .bind(state_hash)
                .bind(expires_at)
                .bind(flow),
            )
            .await?;
        Ok(McpOAuthAuthorization { key, generation })
    }

    async fn claim_callback(
        &self,
        environment_id: Uuid,
        state_hash: &[u8],
        now: SqlDateTime,
    ) -> RepoResult<Option<McpOAuthExchangeClaim>> {
        let sql = format!(
            indoc! {r#"
            UPDATE mcp_oauth_grants SET status = 'exchanging'
            WHERE state_hash = $1 AND status = 'pending-consent' AND consent_expires_at > $2
              AND environment_id = $3
            RETURNING {}
        "#},
            RETURNING
        );
        let row: Option<GrantRow> = self
            .rw("claim_callback")
            .fetch_optional_as(
                sqlx::query_as(&sql)
                    .bind(state_hash)
                    .bind(now)
                    .bind(environment_id),
            )
            .await?;
        row.map(|row| {
            let flow = row
                .flow_secrets
                .clone()
                .ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "claimed OAuth flow has no private payload"
                    ))
                })
                .and_then(decode)?;
            Ok(McpOAuthExchangeClaim {
                key: row.key(),
                generation: row.generation,
                flow,
            })
        })
        .transpose()
    }

    async fn publish_exchange(
        &self,
        key: &McpOAuthGrantKey,
        generation: Uuid,
        tokens: McpOAuthTokens,
    ) -> RepoResult<bool> {
        self.publish(key, generation, "exchanging", tokens, "publish_exchange")
            .await
    }

    async fn load(&self, key: &McpOAuthGrantKey) -> RepoResult<Option<McpOAuthGrant>> {
        let sql = format!(
            "SELECT {RETURNING} FROM mcp_oauth_grants WHERE environment_id = $1 AND security_scheme_id = $2 AND security_scheme_revision = $3 AND credential_owner_account_id = $4 AND resource_url = $5"
        );
        let row: Option<GrantRow> = self
            .ro("load")
            .fetch_optional_as(
                sqlx::query_as(&sql)
                    .bind(key.environment_id)
                    .bind(key.security_scheme_id)
                    .bind(key.security_scheme_revision)
                    .bind(key.credential_owner_account_id)
                    .bind(&key.resource_url),
            )
            .await?;
        row.map(GrantRow::into_grant).transpose()
    }

    async fn claim_refresh(
        &self,
        key: &McpOAuthGrantKey,
        expected_generation: Uuid,
    ) -> RepoResult<Option<McpOAuthRefreshClaim>> {
        let sql = format!(
            "UPDATE mcp_oauth_grants SET status = 'refreshing', generation = $7 WHERE environment_id = $1 AND security_scheme_id = $2 AND security_scheme_revision = $3 AND credential_owner_account_id = $4 AND resource_url = $5 AND generation = $6 AND status = 'granted' AND token_secrets IS NOT NULL RETURNING {RETURNING}"
        );
        let row: Option<GrantRow> = self
            .rw("claim_refresh")
            .fetch_optional_as(
                sqlx::query_as(&sql)
                    .bind(key.environment_id)
                    .bind(key.security_scheme_id)
                    .bind(key.security_scheme_revision)
                    .bind(key.credential_owner_account_id)
                    .bind(&key.resource_url)
                    .bind(expected_generation)
                    .bind(Uuid::new_v4()),
            )
            .await?;
        row.map(|row| {
            let tokens = row
                .token_secrets
                .clone()
                .ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "refresh claim has no private token payload"
                    ))
                })
                .and_then(decode)?;
            Ok(McpOAuthRefreshClaim {
                key: row.key(),
                generation: row.generation,
                tokens,
            })
        })
        .transpose()
    }

    async fn publish_refresh(
        &self,
        key: &McpOAuthGrantKey,
        generation: Uuid,
        tokens: McpOAuthTokens,
    ) -> RepoResult<bool> {
        self.publish(key, generation, "refreshing", tokens, "publish_refresh")
            .await
    }

    async fn fail_exchange(&self, key: &McpOAuthGrantKey, generation: Uuid) -> RepoResult<bool> {
        self.fail(key, generation, "exchanging", "fail_exchange")
            .await
    }

    async fn fail_refresh(&self, key: &McpOAuthGrantKey, generation: Uuid) -> RepoResult<bool> {
        self.fail(key, generation, "refreshing", "fail_refresh")
            .await
    }

    async fn expire_access_token(
        &self,
        key: &McpOAuthGrantKey,
        expected_generation: Uuid,
        now: SqlDateTime,
    ) -> RepoResult<bool> {
        let Some(grant) = self.load(key).await? else {
            return Ok(false);
        };
        if grant.generation != expected_generation || grant.status != McpOAuthGrantStatus::Granted {
            return Ok(false);
        }
        let Some(mut tokens) = grant.tokens else {
            return Ok(false);
        };
        tokens.expires_at = Some(now.into_utc());
        let token_secrets = encode(&tokens)?;
        let result = self
            .rw("expire_access_token")
            .fetch_optional(
                sqlx::query(indoc! {r#"
            UPDATE mcp_oauth_grants SET generation = $1, token_secrets = $2
            WHERE environment_id = $3 AND security_scheme_id = $4 AND security_scheme_revision = $5
              AND credential_owner_account_id = $6 AND resource_url = $7 AND generation = $8
              AND status = 'granted'
            RETURNING generation
        "#})
                .bind(Uuid::new_v4())
                .bind(token_secrets)
                .bind(key.environment_id)
                .bind(key.security_scheme_id)
                .bind(key.security_scheme_revision)
                .bind(key.credential_owner_account_id)
                .bind(&key.resource_url)
                .bind(expected_generation),
            )
            .await?;
        Ok(result.is_some())
    }

    async fn revoke(&self, key: &McpOAuthGrantKey) -> RepoResult<McpOAuthAuthorization> {
        let generation = Uuid::new_v4();
        self.rw("revoke").execute(sqlx::query(indoc! {r#"
            INSERT INTO mcp_oauth_grants
                (environment_id, security_scheme_id, security_scheme_revision, credential_owner_account_id,
                 resource_url, generation, status, state_hash, consent_expires_at, flow_secrets, token_secrets)
            VALUES ($1, $2, $3, $4, $5, $6, 'revoked', NULL, NULL, NULL, NULL)
            ON CONFLICT (environment_id, security_scheme_id, security_scheme_revision,
                         credential_owner_account_id, resource_url) DO UPDATE SET
                generation = $6, status = 'revoked', state_hash = NULL,
                consent_expires_at = NULL, flow_secrets = NULL, token_secrets = NULL
        "#}).bind(key.environment_id).bind(key.security_scheme_id).bind(key.security_scheme_revision)
            .bind(key.credential_owner_account_id).bind(&key.resource_url).bind(generation)).await?;
        Ok(McpOAuthAuthorization {
            key: key.clone(),
            generation,
        })
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbMcpOAuthGrantRepo<PostgresPool> {
    async fn publish(
        &self,
        key: &McpOAuthGrantKey,
        generation: Uuid,
        expected_status: &str,
        tokens: McpOAuthTokens,
        operation: &'static str,
    ) -> RepoResult<bool> {
        let token_secrets = encode(&tokens)?;
        let result = self.rw(operation).fetch_optional(sqlx::query(indoc! {r#"
            UPDATE mcp_oauth_grants SET status = 'granted', state_hash = NULL,
                consent_expires_at = NULL, flow_secrets = NULL, token_secrets = $1
            WHERE environment_id = $2 AND security_scheme_id = $3 AND security_scheme_revision = $4
              AND credential_owner_account_id = $5 AND resource_url = $6 AND generation = $7 AND status = $8
            RETURNING generation
        "#}).bind(token_secrets).bind(key.environment_id).bind(key.security_scheme_id)
            .bind(key.security_scheme_revision).bind(key.credential_owner_account_id)
            .bind(&key.resource_url).bind(generation).bind(expected_status)).await?;
        Ok(result.is_some())
    }

    async fn fail(
        &self,
        key: &McpOAuthGrantKey,
        generation: Uuid,
        expected_status: &str,
        operation: &'static str,
    ) -> RepoResult<bool> {
        let result = self.rw(operation).fetch_optional(sqlx::query(indoc! {r#"
            UPDATE mcp_oauth_grants SET status = 'reauthorization-required', state_hash = NULL,
                consent_expires_at = NULL, flow_secrets = NULL, token_secrets = NULL
            WHERE environment_id = $1 AND security_scheme_id = $2 AND security_scheme_revision = $3
              AND credential_owner_account_id = $4 AND resource_url = $5 AND generation = $6 AND status = $7
            RETURNING generation
        "#}).bind(key.environment_id).bind(key.security_scheme_id).bind(key.security_scheme_revision)
            .bind(key.credential_owner_account_id).bind(&key.resource_url).bind(generation).bind(expected_status)).await?;
        Ok(result.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use golem_common::config::DbSqliteConfig;
    use golem_service_base::db;
    use golem_service_base::migration::{Migrations, MigrationsDir};
    use std::path::PathBuf;
    use test_r::test;

    async fn setup() -> (SqlitePool, McpOAuthGrantKey, PathBuf) {
        let path = std::env::temp_dir().join(format!("mcp-oauth-{}.db", Uuid::new_v4()));
        let config = DbSqliteConfig {
            database: path.to_string_lossy().into_owned(),
            max_connections: 3,
            foreign_keys: true,
        };
        let migrations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("db/migration");
        db::sqlite::migrate(&config, MigrationsDir::new(migrations).sqlite_migrations())
            .await
            .unwrap();
        let pool = SqlitePool::configured(&config).await.unwrap();
        let owner = Uuid::new_v4();
        let app = Uuid::new_v4();
        let environment = Uuid::new_v4();
        let scheme = Uuid::new_v4();
        let now = SqlDateTime::now();
        let mut api = pool.with_rw("mcp-oauth-test", "seed");
        api.execute(sqlx::query("INSERT INTO accounts (account_id,email,created_at,updated_at,deleted_at,modified_by,current_revision_id) VALUES ($1,$2,$3,$3,NULL,$1,1)").bind(owner).bind(format!("{owner}@test.invalid")).bind(now.clone())).await.unwrap();
        api.execute(sqlx::query("INSERT INTO applications (application_id,name,account_id,created_at,updated_at,deleted_at,modified_by,current_revision_id) VALUES ($1,'app',$2,$3,$3,NULL,$2,1)").bind(app).bind(owner).bind(now.clone())).await.unwrap();
        api.execute(sqlx::query("INSERT INTO environments (environment_id,name,application_id,created_at,updated_at,deleted_at,modified_by,current_revision_id) VALUES ($1,'env',$2,$3,$3,NULL,$4,1)").bind(environment).bind(app).bind(now.clone()).bind(owner)).await.unwrap();
        api.execute(sqlx::query("INSERT INTO security_schemes (security_scheme_id,environment_id,name,created_at,updated_at,deleted_at,modified_by,current_revision_id) VALUES ($1,$2,'oauth',$3,$3,NULL,$4,1)").bind(scheme).bind(environment).bind(now.clone()).bind(owner)).await.unwrap();
        api.execute(sqlx::query("INSERT INTO security_scheme_revisions (security_scheme_id,revision_id,provider_type,client_id,client_secret,redirect_url,scopes,created_at,created_by,deleted) VALUES ($1,1,'custom','client','secret','https://callback','[]',$2,$3,false)").bind(scheme).bind(now).bind(owner)).await.unwrap();
        (
            pool,
            McpOAuthGrantKey {
                environment_id: environment,
                security_scheme_id: scheme,
                security_scheme_revision: 1,
                credential_owner_account_id: owner,
                resource_url: "https://mcp.example/resource".into(),
            },
            path,
        )
    }

    fn flow(marker: &str) -> McpOAuthFlowSecrets {
        McpOAuthFlowSecrets {
            pkce_verifier: format!("pkce-{marker}"),
            session: crate::repo::model::mcp_oauth::McpOAuthSession {
                target: crate::repo::model::mcp_oauth::McpImportTarget::Deployed(
                    golem_common::model::mcp_import::McpImportSource {
                        environment_id: golem_common::model::environment::EnvironmentId(
                            Uuid::new_v4(),
                        ),
                        deployment_revision: 1_i64.try_into().unwrap(),
                        import_index: 0,
                        upstream_tool_name: String::new(),
                    },
                ),
                authorized_by: Uuid::new_v4(),
                server: golem_mcp_import::oauth::AuthorizationServerMetadata {
                    issuer: "https://issuer".into(),
                    authorization_endpoint: "https://issuer/authorize".into(),
                    token_endpoint: "https://issuer/token".into(),
                    response_types_supported: vec!["code".into()],
                    code_challenge_methods_supported: Some(vec!["S256".into()]),
                    token_endpoint_auth_methods_supported: None,
                    authorization_response_iss_parameter_supported: true,
                },
                scopes: vec!["tools".into()],
            },
        }
    }

    fn tokens(marker: &str) -> McpOAuthTokens {
        McpOAuthTokens {
            session: flow(marker).session,
            access_token: format!("access-{marker}"),
            refresh_token: Some(format!("refresh-{marker}")),
            expires_at: Some(Utc::now() + Duration::hours(1)),
            scopes: vec!["tools".into()],
        }
    }

    #[test]
    async fn sqlite_oauth_grant_lifecycle_and_fencing() {
        let (pool, key, path) = setup().await;
        let repo = DbMcpOAuthGrantRepo::new(pool.clone());
        let expired = repo
            .begin_authorization(
                key.clone(),
                b"expired".to_vec(),
                (Utc::now() - Duration::seconds(1)).into(),
                flow("expired"),
            )
            .await
            .unwrap();
        assert!(
            repo.claim_callback(key.environment_id, b"expired", Utc::now().into())
                .await
                .unwrap()
                .is_none()
        );
        let auth = repo
            .begin_authorization(
                key.clone(),
                b"state".to_vec(),
                (Utc::now() + Duration::minutes(5)).into(),
                flow("one"),
            )
            .await
            .unwrap();
        assert_ne!(expired.generation, auth.generation);
        assert!(
            repo.claim_callback(Uuid::new_v4(), b"state", Utc::now().into())
                .await
                .unwrap()
                .is_none()
        );
        let claim = repo
            .claim_callback(key.environment_id, b"state", Utc::now().into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claim.flow.pkce_verifier, "pkce-one");
        assert!(
            repo.claim_callback(key.environment_id, b"state", Utc::now().into())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            repo.publish_exchange(&key, auth.generation, tokens("one"))
                .await
                .unwrap()
        );

        let (left, right) = tokio::join!(
            repo.claim_refresh(&key, auth.generation),
            repo.claim_refresh(&key, auth.generation)
        );
        let (left, right) = (left.unwrap(), right.unwrap());
        assert_eq!(
            usize::from(left.is_some()) + usize::from(right.is_some()),
            1
        );
        let first_refresh = left.or(right).unwrap();
        assert!(
            repo.claim_refresh(&key, auth.generation)
                .await
                .unwrap()
                .is_none()
        );
        assert!(repo.load(&key).await.unwrap().unwrap().tokens.is_none());
        assert!(
            repo.publish_refresh(&key, first_refresh.generation, tokens("two"))
                .await
                .unwrap()
        );
        assert_eq!(
            repo.load(&key)
                .await
                .unwrap()
                .unwrap()
                .tokens
                .unwrap()
                .access_token,
            "access-two"
        );

        let current = repo.load(&key).await.unwrap().unwrap();
        let refresh_token = current.tokens.as_ref().unwrap().refresh_token.clone();
        assert!(
            repo.expire_access_token(&key, current.generation, SqlDateTime::now())
                .await
                .unwrap()
        );
        let expired = repo.load(&key).await.unwrap().unwrap();
        assert_ne!(expired.generation, current.generation);
        assert_eq!(
            expired.tokens.as_ref().unwrap().refresh_token,
            refresh_token
        );
        assert!(expired.tokens.unwrap().expires_at.unwrap() <= Utc::now());
        assert!(
            !repo
                .expire_access_token(&key, current.generation, SqlDateTime::now())
                .await
                .unwrap()
        );

        let refresh = repo
            .claim_refresh(&key, expired.generation)
            .await
            .unwrap()
            .unwrap();
        let revoked = repo.revoke(&key).await.unwrap();
        assert_ne!(refresh.generation, revoked.generation);
        assert!(
            !repo
                .publish_refresh(&key, refresh.generation, tokens("late"))
                .await
                .unwrap()
        );
        assert_eq!(
            repo.load(&key).await.unwrap().unwrap().status,
            McpOAuthGrantStatus::Revoked
        );

        let newer = repo
            .begin_authorization(
                key.clone(),
                b"new".to_vec(),
                (Utc::now() + Duration::minutes(5)).into(),
                flow("new"),
            )
            .await
            .unwrap();
        assert!(
            !repo
                .publish_exchange(&key, auth.generation, tokens("late-exchange"))
                .await
                .unwrap()
        );
        assert_eq!(
            repo.load(&key).await.unwrap().unwrap().generation,
            newer.generation
        );

        let mut isolated_keys = Vec::new();
        let mut isolated = key.clone();
        isolated.resource_url.push_str("/other");
        isolated_keys.push(isolated);
        let mut isolated = key.clone();
        isolated.credential_owner_account_id = Uuid::new_v4();
        isolated_keys.push(isolated);
        let mut isolated = key.clone();
        isolated.environment_id = Uuid::new_v4();
        isolated_keys.push(isolated);
        let mut isolated = key.clone();
        isolated.security_scheme_id = Uuid::new_v4();
        isolated_keys.push(isolated);
        let mut isolated = key.clone();
        isolated.security_scheme_revision += 1;
        isolated_keys.push(isolated);
        for isolated in isolated_keys {
            assert!(repo.load(&isolated).await.unwrap().is_none());
        }
        drop(repo);
        drop(pool);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    async fn sqlite_refresh_claim_fences_subsequent_refresh_cycles() {
        let (pool, key, path) = setup().await;
        let repo = DbMcpOAuthGrantRepo::new(pool.clone());
        let auth = repo
            .begin_authorization(
                key.clone(),
                b"state".to_vec(),
                (Utc::now() + Duration::minutes(5)).into(),
                flow("initial"),
            )
            .await
            .unwrap();
        repo.claim_callback(key.environment_id, b"state", Utc::now().into())
            .await
            .unwrap()
            .unwrap();
        assert!(
            repo.publish_exchange(&key, auth.generation, tokens("initial"))
                .await
                .unwrap()
        );
        let first = repo
            .claim_refresh(&key, auth.generation)
            .await
            .unwrap()
            .unwrap();
        assert!(
            repo.publish_refresh(&key, first.generation, tokens("first"))
                .await
                .unwrap()
        );
        assert!(
            repo.claim_refresh(&key, auth.generation)
                .await
                .unwrap()
                .is_none(),
            "a waiter holding an obsolete grant must not start another refresh"
        );
        let second = repo
            .claim_refresh(&key, first.generation)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !repo
                .publish_refresh(&key, first.generation, tokens("late-first"))
                .await
                .unwrap()
        );
        assert!(!repo.fail_refresh(&key, first.generation).await.unwrap());
        assert!(
            repo.publish_refresh(&key, second.generation, tokens("second"))
                .await
                .unwrap()
        );
        assert_eq!(
            repo.load(&key)
                .await
                .unwrap()
                .unwrap()
                .tokens
                .unwrap()
                .access_token,
            "access-second"
        );
        drop(repo);
        drop(pool);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn oauth_secret_debug_is_redacted() {
        let rendered = format!("{:?} {:?}", flow("do-not-print"), tokens("do-not-print"));
        assert!(!rendered.contains("do-not-print"));
        assert!(rendered.contains("redacted"));
        let error = decode::<McpOAuthTokens>(
            br#"{"access_token":"token", "scopes":"do-not-print"}"#.to_vec(),
        )
        .unwrap_err();
        assert!(!format!("{error:?}").contains("do-not-print"));
    }
}
