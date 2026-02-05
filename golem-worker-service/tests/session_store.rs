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

use chrono::{TimeDelta, Utc};
use golem_common::config::{DbSqliteConfig, RedisConfig};
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_common::redis::RedisPool;
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_service_base::db::sqlite::SqlitePool;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_worker_service::custom_api::model::OidcSession;
use golem_worker_service::custom_api::security::model::{PendingOidcLogin, SessionId};
use golem_worker_service::custom_api::security::session_store::SessionStore;
use golem_worker_service::custom_api::security::session_store::{
    RedisSessionStore, SqliteSessionStore,
};
use openidconnect::core::CoreIdTokenClaims;
use openidconnect::{
    Audience, EmptyAdditionalClaims, IssuerUrl, Nonce, Scope, StandardClaims, SubjectIdentifier,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::{define_matrix_dimension, test, test_dep};
use tokio::time::sleep;
use tracing::Level;
use uuid::Uuid;

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("worker-service-session-store-tests")
                .with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
async fn sqlite_store_file() -> NamedTempFile {
    NamedTempFile::new().unwrap()
}

#[test_dep]
async fn sqlite_pool(db_file: &NamedTempFile) -> SqlitePool {
    let db_config = DbSqliteConfig {
        database: db_file.path().to_string_lossy().to_string(),
        max_connections: 10,
        foreign_keys: false,
    };

    SqlitePool::configured(&db_config).await.unwrap()
}

async fn sqlite_store(sqlite_pool: &SqlitePool, expiration_secs: i64) -> SqliteSessionStore {
    SqliteSessionStore::new(
        sqlite_pool.clone(),
        expiration_secs,
        Duration::from_secs(3600),
    )
    .await
    .unwrap()
}

#[test_dep(tagged_as = "sqlite")]
async fn sqlite_store_default(
    _tracing: &Tracing,
    sqlite_pool: &SqlitePool,
) -> Box<dyn SessionStore> {
    Box::new(sqlite_store(sqlite_pool, 60).await)
}

#[test_dep(tagged_as = "sqlite_fast_expiry")]
async fn sqlite_store_fast_expiry(
    _tracing: &Tracing,
    sqlite_pool: &SqlitePool,
) -> Box<dyn SessionStore> {
    Box::new(sqlite_store(sqlite_pool, 0).await)
}

#[test_dep]
async fn redis() -> Arc<dyn Redis> {
    Arc::new(SpawnedRedis::new(
        6379,
        "".to_string(),
        Level::INFO,
        Level::ERROR,
    ))
}

#[test_dep]
async fn redis_pool(redis: &Arc<dyn Redis>) -> RedisPool {
    RedisPool::configured(&RedisConfig {
        host: redis.public_host(),
        port: redis.public_port(),
        ..Default::default()
    })
    .await
    .unwrap()
}

async fn redis_store(redis_pool: &RedisPool, expiration_millis: i64) -> RedisSessionStore {
    let expiration = fred::types::Expiration::PX(expiration_millis);
    RedisSessionStore::new(redis_pool.clone(), expiration)
}

#[test_dep(tagged_as = "redis")]
async fn redis_store_default(_tracing: &Tracing, redis_pool: &RedisPool) -> Box<dyn SessionStore> {
    Box::new(redis_store(redis_pool, 6000).await)
}

#[test_dep(tagged_as = "redis_fast_expiry")]
async fn redis_store_fast_expiry(
    _tracing: &Tracing,
    redis_pool: &RedisPool,
) -> Box<dyn SessionStore> {
    Box::new(redis_store(redis_pool, 100).await)
}

define_matrix_dimension!(session_store: Box<dyn SessionStore> -> "redis", "sqlite");
define_matrix_dimension!(session_store_fast_expiry: Box<dyn SessionStore> -> "redis_fast_expiry", "sqlite_fast_expiry");

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
async fn pending_login_store_and_take(#[dimension(session_store)] store: &Box<dyn SessionStore>) {
    let login = sample_pending_login();

    store
        .store_pending_oidc_login("state1".into(), login.clone())
        .await
        .unwrap();

    let fetched = store
        .take_pending_oidc_login("state1")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(fetched.original_uri, login.original_uri);

    let missing = store.take_pending_oidc_login("state1").await.unwrap();

    assert!(missing.is_none());
}

#[test]
async fn pending_login_expires(
    #[dimension(session_store_fast_expiry)] store: &Box<dyn SessionStore>,
) {
    let login = sample_pending_login();

    store
        .store_pending_oidc_login("state_expired".into(), login)
        .await
        .unwrap();

    sleep(Duration::from_millis(100)).await;

    let fetched = store
        .take_pending_oidc_login("state_expired")
        .await
        .unwrap();

    assert!(fetched.is_none());
}

#[test]
async fn authenticated_session_roundtrip(
    #[dimension(session_store)] store: &Box<dyn SessionStore>,
) {
    let session_id = SessionId(Uuid::now_v7());

    let session = sample_session(Utc::now() + TimeDelta::seconds(30));

    store
        .store_authenticated_session(&session_id, session.clone())
        .await
        .unwrap();

    let fetched = store
        .get_authenticated_session(&session_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(fetched.subject, session.subject);
}
