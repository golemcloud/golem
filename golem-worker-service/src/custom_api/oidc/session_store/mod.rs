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

mod db;
mod redis;

pub use db::{DbSessionStore, PostgresSessionStore, SqliteSessionStore};
pub use redis::RedisSessionStore;

use super::model::{
    AuthorizationCode, BearerToken, McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin,
    PendingPkceLogin, PkceAuthorizationCode, PkceBearerCredential, PkceBinding, SessionId,
};
use crate::custom_api::model::OidcSession;
use async_trait::async_trait;
use golem_common::error_forwarding;
use golem_common::redis::RedisError;
use golem_service_base::repo::RepoError;

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

fn credential_digest(kind: &str, secret: &str) -> String {
    let context = format!("golem oidc pkce {kind}");
    blake3::derive_key(&context, secret.as_bytes())
        .as_slice()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
