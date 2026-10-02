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

use crate::custom_api::model::OidcSession;
use chrono::{DateTime, TimeDelta, Utc};
use golem_common::model::auth::TokenSecret;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{SecuritySchemeId, SecuritySchemeRevision};
use openidconnect::{CsrfToken, Nonce};
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

pub const AUTHORIZATION_CODE_LIFETIME: TimeDelta = TimeDelta::seconds(60);
pub const BEARER_TOKEN_LIFETIME: TimeDelta = TimeDelta::hours(1);
pub const PENDING_PKCE_LOGIN_LIFETIME: TimeDelta = TimeDelta::minutes(10);

#[derive(Debug, Clone)]
pub struct SessionId(pub Uuid);

#[derive(Debug, Clone)]
pub struct PendingOidcLogin {
    pub scheme_id: SecuritySchemeId,
    pub original_uri: String,
    pub nonce: Nonce,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceBinding {
    pub security_scheme_id: SecuritySchemeId,
    pub security_scheme_revision: SecuritySchemeRevision,
    pub environment_id: EnvironmentId,
    pub api_origin: String,
}

#[derive(Debug, Clone)]
pub struct PendingPkceLogin {
    pub binding: PkceBinding,
    pub redirect_uri: String,
    pub frontend_state: String,
    pub code_challenge: String,
    pub upstream_nonce: Nonce,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PkceAuthorizationCode {
    pub binding: PkceBinding,
    pub principal: OidcSession,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PkceBearerCredential {
    pub binding: PkceBinding,
    pub principal: OidcSession,
    pub expires_at: DateTime<Utc>,
}

macro_rules! opaque_credential {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(TokenSecret);

        impl $name {
            pub fn generate() -> Self {
                Self(TokenSecret::new())
            }

            pub fn secret(&self) -> &str {
                self.0.secret()
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = &'static str;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self(TokenSecret::from_str(value)?))
            }
        }
    };
}

opaque_credential!(AuthorizationCode);
opaque_credential!(BearerToken);

pub struct AuthorizationUrl {
    pub url: Url,
    pub csrf_state: CsrfToken,
    pub nonce: Nonce,
}

/// Stored during MCP OAuth proxy `/authorize` — captures the MCP client's
/// redirect_uri and state so we can redirect back after the provider callback.
#[derive(Debug, Clone)]
pub struct McpPendingAuth {
    pub client_redirect_uri: String,
    pub client_state: Option<String>,
}

/// Stored during MCP OAuth proxy `/callback` — holds the raw tokens obtained
/// from the provider, indexed by a proxy authorization code that the MCP client
/// will exchange via `/token`.
#[derive(Debug, Clone)]
pub struct McpProxyCodeEntry {
    pub id_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: String,
}
