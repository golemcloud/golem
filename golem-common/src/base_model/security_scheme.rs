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

use crate::base_model::Empty;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::validate_lower_kebab_case_identifier;
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid,
};
use derive_more::Display;
use std::str::FromStr;

newtype_uuid!(
    SecuritySchemeId,
    golem_api_grpc::proto::golem::registry::SecuritySchemeId
);

declare_revision!(SecuritySchemeRevision);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct SecuritySchemeName(pub String);
}

impl TryFrom<String> for SecuritySchemeName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_lower_kebab_case_identifier("Security Scheme", &value)?;
        Ok(SecuritySchemeName(value))
    }
}

impl FromStr for SecuritySchemeName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}

declare_structs! {
    pub struct SecuritySchemeCreation {
        pub name: SecuritySchemeName,
        pub provider_type: Provider,
        pub client_id: String,
        pub client_secret: String,
        pub redirect_url: String,
        pub scopes: Vec<String>,
    }

    pub struct SecuritySchemeUpdate {
        pub current_revision: SecuritySchemeRevision,
        pub provider_type: Option<Provider>,
        pub client_id: Option<String>,
        pub client_secret: Option<String>,
        pub redirect_url: Option<String>,
        pub scopes: Option<Vec<String>>,
    }

    pub struct SecuritySchemeDto {
        pub id: SecuritySchemeId,
        pub revision: SecuritySchemeRevision,
        pub name: SecuritySchemeName,
        pub environment_id: EnvironmentId,
        pub provider_type: Provider,
        pub client_id: String,
        pub redirect_url: String,
        pub scopes: Vec<String>,
    }
}

declare_enums! {
    pub enum ProviderKind {
        Google,
        Facebook,
        Microsoft,
        Gitlab,
        Custom,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct CustomProvider {
    pub name: String,
    pub issuer_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum Provider {
    Google(Empty),
    Facebook(Empty),
    Microsoft(Empty),
    Gitlab(Empty),
    Custom(CustomProvider),
}

impl Provider {
    pub fn kind(&self) -> ProviderKind {
        match self {
            Provider::Google(_) => ProviderKind::Google,
            Provider::Facebook(_) => ProviderKind::Facebook,
            Provider::Microsoft(_) => ProviderKind::Microsoft,
            Provider::Gitlab(_) => ProviderKind::Gitlab,
            Provider::Custom(_) => ProviderKind::Custom,
        }
    }

    pub fn custom(name: String, issuer_url: String) -> Result<Self, String> {
        let url = url::Url::parse(&issuer_url).map_err(|e| format!("Invalid issuer URL: {e}"))?;
        if url.host_str().is_none() {
            return Err("Issuer URL must have a host".to_string());
        }
        if url.scheme() != "https" && url.scheme() != "http" {
            return Err("Issuer URL must use http or https scheme".to_string());
        }
        Ok(Provider::Custom(CustomProvider { name, issuer_url }))
    }

    pub fn validate_issuer_url_strict(&self) -> Result<(), String> {
        match self {
            Provider::Custom(CustomProvider { issuer_url, .. }) => {
                let url =
                    url::Url::parse(issuer_url).map_err(|e| format!("Invalid issuer URL: {e}"))?;
                if url.scheme() != "https" {
                    return Err(
                        "Custom provider issuer URL must use https in production".to_string()
                    );
                }
                if url.query().is_some() {
                    return Err(
                        "Custom provider issuer URL must not contain query parameters".to_string(),
                    );
                }
                if url.fragment().is_some() {
                    return Err(
                        "Custom provider issuer URL must not contain a fragment".to_string()
                    );
                }
                if url.password().is_some() || !url.username().is_empty() {
                    return Err(
                        "Custom provider issuer URL must not contain credentials".to_string()
                    );
                }
                let host = url.host_str().unwrap_or("");
                if host == "localhost"
                    || host == "127.0.0.1"
                    || host == "::1"
                    || host == "0.0.0.0"
                    || host == "169.254.169.254"
                {
                    return Err(
                        "Custom provider issuer URL must not point to a local or metadata address"
                            .to_string(),
                    );
                }
                if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                    match ip {
                        std::net::IpAddr::V4(v4) => {
                            if v4.is_loopback()
                                || v4.is_private()
                                || v4.is_link_local()
                                || v4.is_unspecified()
                            {
                                return Err("Custom provider issuer URL must not point to a private or local address".to_string());
                            }
                        }
                        std::net::IpAddr::V6(v6) => {
                            if v6.is_loopback() || v6.is_unspecified() {
                                return Err("Custom provider issuer URL must not point to a private or local address".to_string());
                            }
                        }
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
