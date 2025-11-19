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

use super::identity_provider_metadata::GolemIdentityProviderMetadata;
use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl, Scope};

#[derive(Debug, Clone, PartialEq)]
pub struct SecuritySchemeWithProviderMetadata {
    pub security_scheme: SecurityScheme,
    pub provider_metadata: GolemIdentityProviderMetadata,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq, derive_more::Display)]
pub struct SecuritySchemeIdentifier(String);

// SecurityScheme shouldn't have Serialize or Deserialize
#[derive(Debug, Clone)]
pub struct SecurityScheme {
    pub scheme_identifier: SecuritySchemeIdentifier,
    pub provider_type: Provider,
    pub client_id: ClientId,
    pub client_secret: ClientSecret, // secret type macros and therefore already redacted
    pub redirect_url: RedirectUrl,
    pub scopes: Vec<Scope>,
}

impl PartialEq for SecurityScheme {
    fn eq(&self, other: &Self) -> bool {
        self.provider_type == other.provider_type
            && self.scheme_identifier == other.scheme_identifier
            && self.client_id == other.client_id
            && self.client_secret.secret() == other.client_secret.secret()
            && self.redirect_url == other.redirect_url
            && self.scopes == other.scopes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Provider {
    Google,
    Facebook,
    Microsoft,
    Gitlab,
}

impl Provider {
    pub fn issue_url(&self) -> Result<IssuerUrl, String> {
        match self {
            Provider::Google => IssuerUrl::new("https://accounts.google.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Google, {err}")),
            Provider::Facebook => IssuerUrl::new("https://www.facebook.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Facebook, {err}")),
            Provider::Microsoft => IssuerUrl::new("https://login.microsoftonline.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Microsoft, {err}")),
            Provider::Gitlab => IssuerUrl::new("https://gitlab.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Gitlab, {err}")),
        }
    }
}
