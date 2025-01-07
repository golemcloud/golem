// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl, Scope};
use poem_openapi::Enum;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

// SecurityScheme shouldn't have Serialize or Deserialize
#[derive(Debug, Clone)]
pub struct SecurityScheme {
    provider_type: Provider,
    scheme_identifier: SecuritySchemeIdentifier,
    client_id: ClientId,
    client_secret: ClientSecret, // secret type macros and therefore already redacted
    redirect_url: RedirectUrl,
    scopes: Vec<Scope>,
}

impl SecurityScheme {
    pub fn new(
        provider_type: Provider,
        scheme_identifier: SecuritySchemeIdentifier,
        client_id: ClientId,
        client_secret: ClientSecret,
        redirect_url: RedirectUrl,
        scopes: Vec<Scope>,
    ) -> Self {
        SecurityScheme {
            provider_type,
            scheme_identifier,
            client_id,
            client_secret,
            redirect_url,
            scopes,
        }
    }
}

// May be relaxed to just a string as we make it more configurable
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Enum)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Google,
    Facebook,
    Microsoft,
    Gitlab,
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "google" => Ok(Provider::Google),
            "facebook" => Ok(Provider::Facebook),
            "microsoft" => Ok(Provider::Microsoft),
            "gitlab" => Ok(Provider::Gitlab),
            _ => Err(format!("Invalid provider: {}", s)),
        }
    }
}

impl From<Provider> for golem_api_grpc::proto::golem::apidefinition::Provider {
    fn from(value: Provider) -> Self {
        match value {
            Provider::Google => golem_api_grpc::proto::golem::apidefinition::Provider {
                provider: Some(
                    golem_api_grpc::proto::golem::apidefinition::provider::Provider::Google(
                        golem_api_grpc::proto::golem::apidefinition::Google {},
                    ),
                ),
            },
            Provider::Facebook => golem_api_grpc::proto::golem::apidefinition::Provider {
                provider: Some(
                    golem_api_grpc::proto::golem::apidefinition::provider::Provider::Facebook(
                        golem_api_grpc::proto::golem::apidefinition::Facebook {},
                    ),
                ),
            },
            Provider::Microsoft => golem_api_grpc::proto::golem::apidefinition::Provider {
                provider: Some(
                    golem_api_grpc::proto::golem::apidefinition::provider::Provider::Microsoft(
                        golem_api_grpc::proto::golem::apidefinition::Microsoft {},
                    ),
                ),
            },
            Provider::Gitlab => golem_api_grpc::proto::golem::apidefinition::Provider {
                provider: Some(
                    golem_api_grpc::proto::golem::apidefinition::provider::Provider::Gitlab(
                        golem_api_grpc::proto::golem::apidefinition::Gitlab {},
                    ),
                ),
            },
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::Provider> for Provider {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::Provider,
    ) -> Result<Self, String> {
        let provider = value.provider.ok_or("Provider name missing".to_string())?;
        match provider {
            golem_api_grpc::proto::golem::apidefinition::provider::Provider::Google(_) => {
                Ok(Provider::Google)
            }
            golem_api_grpc::proto::golem::apidefinition::provider::Provider::Facebook(_) => {
                Ok(Provider::Facebook)
            }
            golem_api_grpc::proto::golem::apidefinition::provider::Provider::Microsoft(_) => {
                Ok(Provider::Microsoft)
            }
            golem_api_grpc::proto::golem::apidefinition::provider::Provider::Gitlab(_) => {
                Ok(Provider::Gitlab)
            }
        }
    }
}

impl Provider {
    pub fn issue_url(&self) -> Result<IssuerUrl, String> {
        match self {
            Provider::Google => IssuerUrl::new("https://accounts.google.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Google, {}", err)),
            Provider::Facebook => IssuerUrl::new("https://www.facebook.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Facebook, {}", err)),
            Provider::Microsoft => IssuerUrl::new("https://login.microsoftonline.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Microsoft, {}", err)),
            Provider::Gitlab => IssuerUrl::new("https://gitlab.com".to_string())
                .map_err(|err| format!("Invalid Issuer URL for Gitlab, {}", err)),
        }
    }
}

impl Display for Provider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Google => write!(f, "google"),
            Provider::Facebook => write!(f, "facebook"),
            Provider::Microsoft => write!(f, "microsoft"),
            Provider::Gitlab => write!(f, "gitlab"),
        }
    }
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

impl SecurityScheme {
    pub fn provider_type(&self) -> Provider {
        self.provider_type.clone()
    }

    pub fn scheme_identifier(&self) -> SecuritySchemeIdentifier {
        self.scheme_identifier.clone()
    }

    pub fn scopes(&self) -> Vec<Scope> {
        self.scopes.clone()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderName(String);

impl Display for ProviderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ProviderName {
    pub fn new(value: String) -> ProviderName {
        ProviderName(value)
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub struct SecuritySchemeIdentifier(String);

impl SecuritySchemeIdentifier {
    pub fn new(value: String) -> Self {
        SecuritySchemeIdentifier(value)
    }
}

impl Display for SecuritySchemeIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SecurityScheme {
    pub fn redirect_url(&self) -> RedirectUrl {
        self.redirect_url.clone()
    }

    pub fn client_id(&self) -> &ClientId {
        &self.client_id
    }

    pub fn client_secret(&self) -> &ClientSecret {
        &self.client_secret
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::SecurityScheme> for SecurityScheme {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::SecurityScheme,
    ) -> Result<Self, Self::Error> {
        let client_id = ClientId::new(value.client_id);
        let client_secret = ClientSecret::new(value.client_secret);
        let provider_proto = value.provider.ok_or("Provider name missing".to_string())?;

        let provider = Provider::try_from(provider_proto)?;

        let scheme_identifier = SecuritySchemeIdentifier::new(value.scheme_identifier);
        let redirect_url = RedirectUrl::new(value.redirect_url)
            .map_err(|err| format!("Invalid RedirectURL. {}", err))?;

        let scopes: Vec<Scope> = value.scopes.iter().map(|x| Scope::new(x.clone())).collect();

        Ok(SecurityScheme {
            provider_type: provider,
            client_secret,
            client_id,
            scheme_identifier,
            redirect_url,
            scopes,
        })
    }
}

impl From<SecurityScheme> for golem_api_grpc::proto::golem::apidefinition::SecurityScheme {
    fn from(value: SecurityScheme) -> Self {
        golem_api_grpc::proto::golem::apidefinition::SecurityScheme {
            provider: Some(golem_api_grpc::proto::golem::apidefinition::Provider::from(
                value.provider_type,
            )),
            scheme_identifier: value.scheme_identifier.to_string(),
            client_id: value.client_id.to_string(),
            client_secret: value.client_secret.secret().clone(),
            redirect_url: value.redirect_url.to_string(),
            scopes: value.scopes.iter().map(|x| x.to_string()).collect(),
        }
    }
}
