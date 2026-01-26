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

use openidconnect::IssuerUrl;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub use crate::base_model::security_scheme::*;

impl Provider {
    pub fn issuer_url(&self) -> IssuerUrl {
        match self {
            Provider::Google => IssuerUrl::new("https://accounts.google.com".to_string()).unwrap(),
            Provider::Facebook => IssuerUrl::new("https://www.facebook.com".to_string()).unwrap(),
            Provider::Microsoft => {
                IssuerUrl::new("https://login.microsoftonline.com".to_string()).unwrap()
            }
            Provider::Gitlab => IssuerUrl::new("https://gitlab.com".to_string()).unwrap(),
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

    impl From<Provider> for golem_api_grpc::proto::golem::registry::SecuritySchemeProvider {
        fn from(value: Provider) -> Self {
            match value {
                Provider::Google => Self::Google,
                Provider::Facebook => Self::Facebook,
                Provider::Gitlab => Self::Gitlab,
                Provider::Microsoft => Self::Microsoft,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::SecuritySchemeProvider> for Provider {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::registry::SecuritySchemeProvider,
        ) -> Result<Self, String> {
            use golem_api_grpc::proto::golem::registry::SecuritySchemeProvider as GrpcProvider;
            match value {
                GrpcProvider::Facebook => Ok(Self::Facebook),
                GrpcProvider::Gitlab => Ok(Self::Gitlab),
                GrpcProvider::Google => Ok(Self::Google),
                GrpcProvider::Microsoft => Ok(Self::Microsoft),
                GrpcProvider::Unspecified => Err("Unknown provider".to_string()),
            }
        }
    }
}
