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

use super::{BearerWriteFailureStores, SessionStorePair};
use chrono::{TimeDelta, Utc};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{SecuritySchemeId, SecuritySchemeRevision};
use golem_worker_service::custom_api::model::OidcSession;
use golem_worker_service::custom_api::oidc::model::{
    AuthorizationCode, BearerToken, PendingOidcLogin, PendingPkceLogin, PkceAuthorizationCode,
    PkceBearerCredential, PkceBinding, SessionId,
};
use golem_worker_service::custom_api::oidc::session_store::SessionStore;
use openidconnect::core::CoreIdTokenClaims;
use openidconnect::{
    Audience, EmptyAdditionalClaims, IssuerUrl, Nonce, Scope, StandardClaims, SubjectIdentifier,
};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use test_r::{define_matrix_dimension, inherit_test_dep, test, timeout};
use tokio::time::sleep;
use uuid::Uuid;

inherit_test_dep!(#[tagged_as("redis")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("sqlite")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("redis_fast_expiry")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("sqlite_fast_expiry")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("redis_tls")] Arc<dyn SessionStore>);
inherit_test_dep!(
    #[tagged_as("redis_pair")]
    SessionStorePair
);
inherit_test_dep!(
    #[tagged_as("sqlite_pair")]
    SessionStorePair
);
inherit_test_dep!(
    #[tagged_as("redis_tls_pair")]
    SessionStorePair
);
inherit_test_dep!(
    #[tagged_as("redis_bearer_write_failure")]
    BearerWriteFailureStores
);
inherit_test_dep!(
    #[tagged_as("sqlite_bearer_write_failure")]
    BearerWriteFailureStores
);

define_matrix_dimension!(session_store: Arc<dyn SessionStore> -> "redis", "sqlite", "redis_tls");
define_matrix_dimension!(session_store_fast_expiry: Arc<dyn SessionStore> -> "redis_fast_expiry", "sqlite_fast_expiry");
define_matrix_dimension!(session_store_pair: SessionStorePair -> "redis_pair", "sqlite_pair", "redis_tls_pair");
define_matrix_dimension!(bearer_write_failure_stores: BearerWriteFailureStores -> "redis_bearer_write_failure", "sqlite_bearer_write_failure");

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

fn sample_binding() -> PkceBinding {
    PkceBinding {
        security_scheme_id: SecuritySchemeId::new(),
        security_scheme_revision: SecuritySchemeRevision::INITIAL,
        environment_id: EnvironmentId::new(),
        api_origin: "https://api.example".into(),
    }
}

fn sample_pending_pkce(
    binding: PkceBinding,
    expires_at: chrono::DateTime<Utc>,
) -> PendingPkceLogin {
    PendingPkceLogin {
        binding,
        redirect_uri: "https://frontend.example/callback".into(),
        frontend_state: "frontend-state".into(),
        code_challenge: "challenge".into(),
        upstream_nonce: Nonce::new("upstream-nonce".into()),
        expires_at,
    }
}

fn sample_authorization_code(
    binding: PkceBinding,
    expires_at: chrono::DateTime<Utc>,
) -> PkceAuthorizationCode {
    PkceAuthorizationCode {
        binding,
        principal: sample_session(Utc::now() + TimeDelta::hours(2)),
        redirect_uri: "https://frontend.example/callback".into(),
        code_challenge: "challenge".into(),
        expires_at,
    }
}

fn sample_bearer(binding: PkceBinding, expires_at: chrono::DateTime<Utc>) -> PkceBearerCredential {
    PkceBearerCredential {
        binding,
        principal: sample_session(Utc::now() + TimeDelta::hours(2)),
        expires_at,
    }
}

#[test]
#[timeout("30s")]
async fn pending_login_store_and_take(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let login = sample_pending_login();

    let state = Uuid::now_v7().to_string();

    store
        .store_pending_oidc_login(&state, login.clone())
        .await?;

    let fetched = store.take_pending_oidc_login(&state).await?.unwrap();

    assert_eq!(fetched.original_uri, login.original_uri);

    let missing = store.take_pending_oidc_login(&state).await.unwrap();

    assert!(missing.is_none());

    Ok(())
}

#[test]
#[timeout("30s")]
async fn pending_login_expires(
    #[dimension(session_store_fast_expiry)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let login = sample_pending_login();

    let state = Uuid::now_v7().to_string();

    store.store_pending_oidc_login(&state, login).await?;

    sleep(Duration::from_millis(200)).await;

    let fetched = store.take_pending_oidc_login(&state).await?;

    assert!(fetched.is_none());

    Ok(())
}

#[test]
#[timeout("30s")]
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
#[timeout("30s")]
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
#[timeout("30s")]
async fn multiple_pending_logins(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let state1 = Uuid::now_v7().to_string();
    let state2 = Uuid::now_v7().to_string();

    let login1 = sample_pending_login();
    let login2 = PendingOidcLogin {
        scheme_id: login1.scheme_id,
        original_uri: "https://another.example.com".to_string(),
        nonce: Nonce::new("nonce456".into()),
    };

    store
        .store_pending_oidc_login(&state1, login1.clone())
        .await?;

    store
        .store_pending_oidc_login(&state2, login2.clone())
        .await?;

    let fetched1 = store.take_pending_oidc_login(&state1).await?.unwrap();

    let fetched2 = store.take_pending_oidc_login(&state2).await?.unwrap();

    assert_eq!(fetched1.original_uri, login1.original_uri);
    assert_eq!(fetched2.original_uri, login2.original_uri);

    Ok(())
}

#[test]
#[timeout("30s")]
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
#[timeout("30s")]
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
#[timeout("30s")]
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
#[timeout("30s")]
async fn pending_login_multiple_take_attempts(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let state = Uuid::now_v7().to_string();
    let login = sample_pending_login();

    store
        .store_pending_oidc_login(&state, login.clone())
        .await?;

    let first_take = store.take_pending_oidc_login(&state).await?;
    assert!(first_take.is_some(), "First take should return the login");

    let second_take = store.take_pending_oidc_login(&state).await?;
    assert!(second_take.is_none(), "Second take should return None");

    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_records_roundtrip_delete_and_keep_namespaces_isolated(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let binding = sample_binding();
    let expires_at = Utc::now() + TimeDelta::minutes(5);
    let upstream_state = Uuid::now_v7().to_string();
    store
        .store_pending_pkce_login(
            &upstream_state,
            sample_pending_pkce(binding.clone(), expires_at),
        )
        .await?;
    let pending = store
        .take_pending_pkce_login(&upstream_state)
        .await?
        .unwrap();
    assert_eq!(pending.binding, binding);
    assert_eq!(pending.frontend_state, "frontend-state");
    assert_eq!(pending.upstream_nonce.secret(), "upstream-nonce");
    assert!(
        store
            .take_pending_pkce_login(&upstream_state)
            .await?
            .is_none()
    );

    let code = AuthorizationCode::generate();
    let token = BearerToken::from_str(code.secret()).unwrap();
    store
        .store_authorization_code(
            &code,
            sample_authorization_code(binding.clone(), expires_at),
        )
        .await?;
    store
        .store_bearer_credential(&token, sample_bearer(binding.clone(), expires_at))
        .await?;

    let redeemed = store.take_authorization_code(code.secret()).await?.unwrap();
    assert_eq!(redeemed.binding, binding);
    assert_eq!(redeemed.principal.subject, "sub");
    assert!(
        store
            .take_authorization_code(code.secret())
            .await?
            .is_none()
    );

    let bearer = store
        .get_bearer_credential(token.secret(), &binding)
        .await?
        .unwrap();
    assert_eq!(bearer.principal.subject, "sub");
    assert!(
        store
            .get_bearer_credential(AuthorizationCode::generate().secret(), &binding)
            .await?
            .is_none()
    );

    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_records_enforce_expiry_without_cleanup(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let binding = sample_binding();
    let expired = Utc::now() - TimeDelta::milliseconds(1);
    let state = Uuid::now_v7().to_string();
    store
        .store_pending_pkce_login(&state, sample_pending_pkce(binding.clone(), expired))
        .await?;
    assert!(store.take_pending_pkce_login(&state).await?.is_none());

    let code = AuthorizationCode::generate();
    store
        .store_authorization_code(&code, sample_authorization_code(binding.clone(), expired))
        .await?;
    assert!(
        store
            .take_authorization_code(code.secret())
            .await?
            .is_none()
    );

    let token = BearerToken::generate();
    store
        .store_bearer_credential(&token, sample_bearer(binding.clone(), expired))
        .await?;
    assert!(
        store
            .get_bearer_credential(token.secret(), &binding)
            .await?
            .is_none()
    );

    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_credentials_reject_context_and_revision_mismatch(
    #[dimension(session_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let binding = sample_binding();
    let expires_at = Utc::now() + TimeDelta::minutes(5);
    let token = BearerToken::generate();
    store
        .store_bearer_credential(&token, sample_bearer(binding.clone(), expires_at))
        .await?;

    let mismatches = [
        PkceBinding {
            security_scheme_id: SecuritySchemeId::new(),
            ..binding.clone()
        },
        PkceBinding {
            security_scheme_revision: binding.security_scheme_revision.next().unwrap(),
            ..binding.clone()
        },
        PkceBinding {
            environment_id: EnvironmentId::new(),
            ..binding.clone()
        },
        PkceBinding {
            api_origin: "https://other-api.example".into(),
            ..binding.clone()
        },
    ];
    for mismatch in mismatches {
        assert!(
            store
                .get_bearer_credential(token.secret(), &mismatch)
                .await?
                .is_none()
        );
    }
    assert!(
        store
            .get_bearer_credential(token.secret(), &binding)
            .await?
            .is_some()
    );

    let code = AuthorizationCode::generate();
    store
        .store_authorization_code(
            &code,
            sample_authorization_code(binding.clone(), expires_at),
        )
        .await?;
    let consumed = store.take_authorization_code(code.secret()).await?.unwrap();
    assert_ne!(
        consumed.binding.security_scheme_revision,
        binding.security_scheme_revision.next().unwrap()
    );
    assert!(
        store
            .take_authorization_code(code.secret())
            .await?
            .is_none()
    );

    Ok(())
}

#[test]
#[timeout("30s")]
async fn independent_store_instances_have_one_atomic_consumer(
    #[dimension(session_store_pair)] stores: &SessionStorePair,
) -> anyhow::Result<()> {
    let cookie_state = Uuid::now_v7().to_string();
    stores
        .first
        .store_pending_oidc_login(&cookie_state, sample_pending_login())
        .await?;
    let (first, second) = tokio::join!(
        stores.first.take_pending_oidc_login(&cookie_state),
        stores.second.take_pending_oidc_login(&cookie_state)
    );
    assert_eq!(
        usize::from(first?.is_some()) + usize::from(second?.is_some()),
        1
    );

    let binding = sample_binding();
    let state = Uuid::now_v7().to_string();
    stores
        .first
        .store_pending_pkce_login(
            &state,
            sample_pending_pkce(binding.clone(), Utc::now() + TimeDelta::minutes(5)),
        )
        .await?;
    let (first, second) = tokio::join!(
        stores.first.take_pending_pkce_login(&state),
        stores.second.take_pending_pkce_login(&state)
    );
    assert_eq!(
        usize::from(first?.is_some()) + usize::from(second?.is_some()),
        1
    );

    let code = AuthorizationCode::generate();
    stores
        .first
        .store_authorization_code(
            &code,
            sample_authorization_code(binding, Utc::now() + TimeDelta::minutes(5)),
        )
        .await?;
    let (first, second) = tokio::join!(
        stores.first.take_authorization_code(code.secret()),
        stores.second.take_authorization_code(code.secret())
    );
    assert_eq!(
        usize::from(first?.is_some()) + usize::from(second?.is_some()),
        1
    );

    Ok(())
}

#[test]
#[timeout("30s")]
async fn bearer_storage_failure_after_consumption_requires_reauthorization(
    #[dimension(bearer_write_failure_stores)] stores: &BearerWriteFailureStores,
) -> anyhow::Result<()> {
    let binding = sample_binding();
    let code = AuthorizationCode::generate();
    let token = BearerToken::generate();
    stores
        .healthy
        .store_authorization_code(
            &code,
            sample_authorization_code(binding.clone(), Utc::now() + TimeDelta::minutes(5)),
        )
        .await?;

    let consumed = stores
        .healthy
        .take_authorization_code(code.secret())
        .await?
        .unwrap();
    let store_result = stores
        .failing
        .store_bearer_credential(
            &token,
            PkceBearerCredential {
                binding: consumed.binding,
                principal: consumed.principal,
                expires_at: Utc::now() + TimeDelta::hours(1),
            },
        )
        .await;
    assert!(
        store_result.is_err(),
        "backend must reject the bearer write"
    );
    assert!(
        stores
            .healthy
            .take_authorization_code(code.secret())
            .await?
            .is_none()
    );
    assert!(
        stores
            .healthy
            .get_bearer_credential(token.secret(), &binding)
            .await?
            .is_none()
    );

    Ok(())
}
