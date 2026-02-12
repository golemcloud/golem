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
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_worker_service::custom_api::model::OidcSession;
use golem_worker_service::custom_api::oidc::model::{PendingOidcLogin, SessionId};
use golem_worker_service::custom_api::oidc::session_store::SessionStore;
use openidconnect::core::CoreIdTokenClaims;
use openidconnect::{
    Audience, EmptyAdditionalClaims, IssuerUrl, Nonce, Scope, StandardClaims, SubjectIdentifier,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use test_r::{define_matrix_dimension, inherit_test_dep, test};
use tokio::time::sleep;
use uuid::Uuid;

inherit_test_dep!(#[tagged_as("redis")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("sqlite")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("redis_fast_expiry")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("sqlite_fast_expiry")] Arc<dyn SessionStore>);

define_matrix_dimension!(session_store: Arc<dyn SessionStore> -> "redis", "sqlite");
define_matrix_dimension!(session_store_fast_expiry: Arc<dyn SessionStore> -> "redis_fast_expiry", "sqlite_fast_expiry");

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
async fn pending_login_store_and_take(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let login = sample_pending_login();

    store
        .store_pending_oidc_login("state1", login.clone())
        .await?;

    let fetched = store.take_pending_oidc_login("state1").await?.unwrap();

    assert_eq!(fetched.original_uri, login.original_uri);

    let missing = store.take_pending_oidc_login("state1").await.unwrap();

    assert!(missing.is_none());

    Ok(())
}

#[test]
async fn pending_login_expires(
    #[dimension(session_store_fast_expiry)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let login = sample_pending_login();

    store
        .store_pending_oidc_login("state_expired", login)
        .await?;

    sleep(Duration::from_millis(100)).await;

    let fetched = store.take_pending_oidc_login("state_expired").await?;

    assert!(fetched.is_none());

    Ok(())
}

#[test]
async fn authenticated_session_roundtrip(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let session_id = SessionId(Uuid::now_v7());

    let session = sample_session(Utc::now() + TimeDelta::seconds(30));

    store
        .store_authenticated_session(&session_id, session.clone())
        .await?;

    let fetched = store.get_authenticated_session(&session_id).await?.unwrap();

    assert_eq!(fetched.subject, session.subject);
    Ok(())
}

#[test]
async fn authenticated_session_expiry(
    #[dimension(session_store_fast_expiry)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let session_id = SessionId(Uuid::now_v7());
    let session = sample_session(Utc::now() + chrono::Duration::milliseconds(50));

    store
        .store_authenticated_session(&session_id, session)
        .await?;

    // Wait past expiry
    sleep(Duration::from_millis(100)).await;

    let fetched = store.get_authenticated_session(&session_id).await.unwrap();
    assert!(fetched.is_none(), "Session should have expired");

    Ok(())
}

#[test]
async fn multiple_pending_logins(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let login1 = sample_pending_login();
    let login2 = PendingOidcLogin {
        scheme_id: login1.scheme_id,
        original_uri: "https://another.example.com".to_string(),
        nonce: Nonce::new("nonce456".into()),
    };

    store
        .store_pending_oidc_login("state1", login1.clone())
        .await?;

    store
        .store_pending_oidc_login("state2", login2.clone())
        .await?;

    let fetched1 = store.take_pending_oidc_login("state1").await?.unwrap();

    let fetched2 = store.take_pending_oidc_login("state2").await?.unwrap();

    assert_eq!(fetched1.original_uri, login1.original_uri);
    assert_eq!(fetched2.original_uri, login2.original_uri);

    Ok(())
}

#[test]
async fn authenticated_session_overwrite(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let session_id = SessionId(Uuid::now_v7());
    let session1 = sample_session(Utc::now() + chrono::Duration::seconds(60));
    let session2 = sample_session(Utc::now() + chrono::Duration::seconds(120));

    store
        .store_authenticated_session(&session_id, session1.clone())
        .await?;

    // Overwrite session
    store
        .store_authenticated_session(&session_id, session2.clone())
        .await?;

    let fetched = store.get_authenticated_session(&session_id).await?.unwrap();

    assert_eq!(fetched.expires_at, session2.expires_at);

    Ok(())
}

#[test]
async fn take_nonexistent_pending_login_returns_none(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let fetched = store.take_pending_oidc_login("nonexistent").await?;
    assert!(
        fetched.is_none(),
        "Fetching nonexistent pending login should return None"
    );

    Ok(())
}

#[test]
async fn get_nonexistent_authenticated_session_returns_none(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let session_id = SessionId(Uuid::now_v7());
    let fetched = store.get_authenticated_session(&session_id).await?;
    assert!(
        fetched.is_none(),
        "Fetching nonexistent authenticated session should return None"
    );

    Ok(())
}

#[test]
async fn pending_login_multiple_take_attempts(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let login = sample_pending_login();

    store
        .store_pending_oidc_login("state_multi", login.clone())
        .await?;

    let first_take = store.take_pending_oidc_login("state_multi").await?;
    assert!(first_take.is_some(), "First take should return the login");

    let second_take = store.take_pending_oidc_login("state_multi").await?;
    assert!(second_take.is_none(), "Second take should return None");

    Ok(())
}
