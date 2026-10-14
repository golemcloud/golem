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
use crate::custom_api::model::OidcSession;
use crate::custom_api::oidc::model::{
    AuthorizationCode, BearerToken, McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin,
    PendingPkceLogin, PkceAuthorizationCode, PkceBearerCredential, PkceBinding, SessionId,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use fred::types::Expiration;
use golem_common::redis::RedisPool;

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
