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

use golem_common::model::account::AccountId;
use golem_common::model::auth::{TokenId, TokenWithSecret};
use golem_common::model::login::OAuth2Provider;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct ExternalLogin {
    pub external_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub verified_emails: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OAuth2Token {
    pub provider: OAuth2Provider,
    pub external_id: String,
    pub account_id: AccountId,
    pub token_id: Option<TokenId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebflowKind {
    /// Browser-based flow: the callback immediately returns the token via redirect.
    /// The redirect URL receives the token secret as a `token` query parameter.
    Browser { redirect: url::Url },
    /// CLI flow: the callback stores the token in the session; the client polls for it.
    /// The server redirects the browser to the configured `cli_redirect` URL.
    Cli,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuth2WebflowStateMetadata {
    pub provider: OAuth2Provider,
    pub kind: WebflowKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuth2WebflowState {
    pub metadata: OAuth2WebflowStateMetadata,
    pub token: Option<TokenWithSecret>,
}
