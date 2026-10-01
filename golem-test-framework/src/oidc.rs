// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default)]
pub enum OidcTokenFailure {
    #[default]
    None,
    ProviderError,
    MissingIdToken,
    InvalidSignature,
    InvalidIssuer,
    InvalidAudience,
    InvalidNonce,
    Expired,
}

#[derive(Clone)]
struct FixtureState {
    issuer: String,
    private_key: Arc<RsaPrivateKey>,
    transactions: Arc<Mutex<HashMap<String, String>>>,
    next_failure: Arc<Mutex<OidcTokenFailure>>,
}

pub struct OidcFixture {
    pub issuer: Url,
    next_failure: Arc<Mutex<OidcTokenFailure>>,
    abort: AbortHandle,
}

impl OidcFixture {
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let issuer = format!("http://{}/", listener.local_addr()?);
        let private_key = Arc::new(RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?);
        let next_failure = Arc::new(Mutex::new(OidcTokenFailure::None));
        let state = FixtureState {
            issuer: issuer.clone(),
            private_key,
            transactions: Arc::new(Mutex::new(HashMap::new())),
            next_failure: next_failure.clone(),
        };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/jwks", get(jwks))
            .with_state(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok(Self {
            issuer: Url::parse(&issuer)?,
            next_failure,
            abort: task.abort_handle(),
        })
    }

    pub fn fail_next_token(&self, failure: OidcTokenFailure) {
        *self.next_failure.lock().unwrap() = failure;
    }

    pub fn client_without_redirects() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }
}

impl Drop for OidcFixture {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

async fn discovery(State(state): State<FixtureState>) -> Json<Value> {
    Json(json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{}authorize", state.issuer),
        "token_endpoint": format!("{}token", state.issuer),
        "jwks_uri": format!("{}jwks", state.issuer),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "scopes_supported": ["openid", "email", "profile"]
    }))
}

#[derive(Deserialize)]
struct AuthorizationQuery {
    redirect_uri: String,
    state: String,
    nonce: String,
}

async fn authorize(
    State(state): State<FixtureState>,
    Query(query): Query<AuthorizationQuery>,
) -> Response {
    let code = Uuid::new_v4().to_string();
    state
        .transactions
        .lock()
        .unwrap()
        .insert(code.clone(), query.nonce);
    let Ok(mut redirect) = Url::parse(&query.redirect_uri) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    redirect
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &query.state);
    Redirect::temporary(redirect.as_str()).into_response()
}

#[derive(Deserialize)]
struct TokenForm {
    code: String,
}

#[derive(Serialize)]
struct Claims {
    iss: String,
    aud: String,
    sub: String,
    exp: u64,
    iat: u64,
    nonce: String,
    email: String,
}

async fn token(State(state): State<FixtureState>, Form(form): Form<TokenForm>) -> Response {
    let Some(nonce) = state.transactions.lock().unwrap().remove(&form.code) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        )
            .into_response();
    };
    let failure = std::mem::take(&mut *state.next_failure.lock().unwrap());
    if matches!(failure, OidcTokenFailure::ProviderError) {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":"temporarily_unavailable"})),
        )
            .into_response();
    }
    if matches!(failure, OidcTokenFailure::MissingIdToken) {
        return Json(json!({
            "access_token": "fixture-access-token",
            "token_type": "Bearer",
            "expires_in": 300
        }))
        .into_response();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = Claims {
        iss: if matches!(failure, OidcTokenFailure::InvalidIssuer) {
            "https://invalid.example".into()
        } else {
            state.issuer.clone()
        },
        aud: if matches!(failure, OidcTokenFailure::InvalidAudience) {
            "invalid-client".into()
        } else {
            "fixture-client".into()
        },
        sub: "fixture-user".into(),
        exp: if matches!(failure, OidcTokenFailure::Expired) {
            now - 60
        } else {
            now + 300
        },
        iat: now,
        nonce: if matches!(failure, OidcTokenFailure::InvalidNonce) {
            "invalid-nonce".into()
        } else {
            nonce
        },
        email: "fixture-user@example.com".into(),
    };
    let key = if matches!(failure, OidcTokenFailure::InvalidSignature) {
        RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap()
    } else {
        (*state.private_key).clone()
    };
    let pem = key.to_pkcs1_pem(Default::default()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture-key".into());
    let id_token = encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap();
    Json(json!({
        "access_token": "fixture-access-token",
        "token_type": "Bearer",
        "expires_in": 300,
        "scope": "openid email profile",
        "id_token": id_token
    }))
    .into_response()
}

async fn jwks(State(state): State<FixtureState>) -> Json<Value> {
    let public = RsaPublicKey::from(state.private_key.as_ref());
    Json(json!({"keys":[{
        "kty":"RSA",
        "use":"sig",
        "kid":"fixture-key",
        "alg":"RS256",
        "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
        "e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
    }]}))
}
