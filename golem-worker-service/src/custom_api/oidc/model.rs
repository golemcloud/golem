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
