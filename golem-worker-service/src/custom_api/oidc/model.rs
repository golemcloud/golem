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

use golem_common::model::security_scheme::SecuritySchemeId;
use openidconnect::{CsrfToken, Nonce};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionId(pub Uuid);

#[derive(Debug, Clone)]
pub struct PendingOidcLogin {
    pub scheme_id: SecuritySchemeId,
    pub original_uri: String,
    pub nonce: Nonce,
}

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
