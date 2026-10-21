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
use openidconnect::IssuerUrl;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub use crate::base_model::security_scheme::*;

impl Provider {
    pub fn issuer_url(&self) -> Result<IssuerUrl, String> {
        match self {
            Provider::Google(_) => {
                Ok(IssuerUrl::new("https://accounts.google.com".to_string()).unwrap())
            }
            Provider::Facebook(_) => {
                Ok(IssuerUrl::new("https://www.facebook.com".to_string()).unwrap())
            }
            Provider::Microsoft(_) => {
                Ok(IssuerUrl::new("https://login.microsoftonline.com".to_string()).unwrap())
            }
            Provider::Gitlab(_) => Ok(IssuerUrl::new("https://gitlab.com".to_string()).unwrap()),
            Provider::Custom(CustomProvider { issuer_url, .. }) => {
                IssuerUrl::new(issuer_url.clone())
                    .map_err(|e| format!("Invalid custom provider issuer URL: {e}"))
            }
        }
    }
}

impl Display for Provider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Google(_) => write!(f, "google"),
            Provider::Facebook(_) => write!(f, "facebook"),
            Provider::Microsoft(_) => write!(f, "microsoft"),
            Provider::Gitlab(_) => write!(f, "gitlab"),
            Provider::Custom(CustomProvider { name, .. }) => write!(f, "custom:{name}"),
        }
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "google" => Ok(Provider::Google(Empty {})),
            "facebook" => Ok(Provider::Facebook(Empty {})),
            "microsoft" => Ok(Provider::Microsoft(Empty {})),
            "gitlab" => Ok(Provider::Gitlab(Empty {})),
            _ => Err(format!(
                "Invalid provider: {s}. Use Provider::Custom for custom providers"
            )),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "google" => Ok(ProviderKind::Google),
            "facebook" => Ok(ProviderKind::Facebook),
            "microsoft" => Ok(ProviderKind::Microsoft),
            "gitlab" => Ok(ProviderKind::Gitlab),
            "custom" => Ok(ProviderKind::Custom),
            _ => Err(format!(
                "Invalid provider kind: {s}. Valid values: google, facebook, microsoft, gitlab, custom"
            )),
        }
    }
}

#[cfg(test)]
mod tests;

mod protobuf {
    use super::{Empty, Provider};
    use golem_api_grpc::proto::golem::registry::SecuritySchemeProvider as GrpcProvider;
    use golem_api_grpc::proto::golem::registry::security_scheme_provider::Kind as GrpcProviderKind;
    use golem_api_grpc::proto::golem::registry::security_scheme_provider::{
        CustomProvider as GrpcCustomProvider, Facebook, Gitlab, Google, Microsoft,
    };

    impl From<Provider> for GrpcProvider {
        fn from(value: Provider) -> Self {
            let kind = match value {
                Provider::Google(_) => GrpcProviderKind::Google(Google {}),
                Provider::Facebook(_) => GrpcProviderKind::Facebook(Facebook {}),
                Provider::Microsoft(_) => GrpcProviderKind::Microsoft(Microsoft {}),
                Provider::Gitlab(_) => GrpcProviderKind::Gitlab(Gitlab {}),
                Provider::Custom(custom) => GrpcProviderKind::Custom(GrpcCustomProvider {
                    name: custom.name,
                    issuer_url: custom.issuer_url,
                }),
            };
            GrpcProvider { kind: Some(kind) }
        }
    }

    impl TryFrom<GrpcProvider> for Provider {
        type Error = String;

        fn try_from(value: GrpcProvider) -> Result<Self, String> {
            match value.kind.ok_or("SecuritySchemeProvider.kind is missing")? {
                GrpcProviderKind::Google(_) => Ok(Self::Google(Empty {})),
                GrpcProviderKind::Facebook(_) => Ok(Self::Facebook(Empty {})),
                GrpcProviderKind::Microsoft(_) => Ok(Self::Microsoft(Empty {})),
                GrpcProviderKind::Gitlab(_) => Ok(Self::Gitlab(Empty {})),
                GrpcProviderKind::Custom(c) => Self::custom(c.name, c.issuer_url),
            }
        }
    }
}
