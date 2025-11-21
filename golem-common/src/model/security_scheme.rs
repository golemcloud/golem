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

use super::environment::EnvironmentId;
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid,
};
use derive_more::Display;
use openidconnect::IssuerUrl;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

newtype_uuid!(SecuritySchemeId);

declare_revision!(SecuritySchemeRevision);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    pub struct SecuritySchemeName(pub String);
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
    pub enum Provider {
        Google,
        Facebook,
        Microsoft,
        Gitlab,
    }
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

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "google" => Ok(Provider::Google),
            "facebook" => Ok(Provider::Facebook),
            "microsoft" => Ok(Provider::Microsoft),
            "gitlab" => Ok(Provider::Gitlab),
            _ => Err(format!("Invalid provider: {s}")),
        }
    }
}

mod protobuf {
    use super::Provider;

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
}
