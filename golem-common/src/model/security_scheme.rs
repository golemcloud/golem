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

use openidconnect::IssuerUrl;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub use crate::base_model::security_scheme::*;

impl Provider {
    pub fn issuer_url(&self) -> Result<IssuerUrl, String> {
        match self {
            Provider::Google => Ok(IssuerUrl::new("https://accounts.google.com".to_string()).unwrap()),
            Provider::Facebook => Ok(IssuerUrl::new("https://www.facebook.com".to_string()).unwrap()),
            Provider::Microsoft => {
                Ok(IssuerUrl::new("https://login.microsoftonline.com".to_string()).unwrap())
            }
            Provider::Gitlab => Ok(IssuerUrl::new("https://gitlab.com".to_string()).unwrap()),
            Provider::Custom { issuer_url, .. } => IssuerUrl::new(issuer_url.clone())
                .map_err(|e| format!("Invalid custom provider issuer URL: {e}")),
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
            Provider::Custom { name, .. } => write!(f, "custom:{name}"),
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
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn provider_serialize_known() {
        let json = serde_json::to_value(&Provider::Google).unwrap();
        assert_eq!(json["type"], "google");
        let json = serde_json::to_value(&Provider::Facebook).unwrap();
        assert_eq!(json["type"], "facebook");
        let json = serde_json::to_value(&Provider::Microsoft).unwrap();
        assert_eq!(json["type"], "microsoft");
        let json = serde_json::to_value(&Provider::Gitlab).unwrap();
        assert_eq!(json["type"], "gitlab");
    }

    #[test]
    fn provider_serialize_custom() {
        let provider = Provider::Custom {
            name: "my-keycloak".to_string(),
            issuer_url: "https://keycloak.example.com/realms/myrealm".to_string(),
        };
        let json = serde_json::to_value(&provider).unwrap();
        assert_eq!(json["type"], "custom");
        assert_eq!(json["name"], "my-keycloak");
        assert_eq!(json["issuerUrl"], "https://keycloak.example.com/realms/myrealm");
    }

    #[test]
    fn provider_deserialize_known() {
        let json = r#"{"type": "google"}"#;
        assert_eq!(serde_json::from_str::<Provider>(json).unwrap(), Provider::Google);
        let json = r#"{"type": "facebook"}"#;
        assert_eq!(serde_json::from_str::<Provider>(json).unwrap(), Provider::Facebook);
        let json = r#"{"type": "microsoft"}"#;
        assert_eq!(serde_json::from_str::<Provider>(json).unwrap(), Provider::Microsoft);
        let json = r#"{"type": "gitlab"}"#;
        assert_eq!(serde_json::from_str::<Provider>(json).unwrap(), Provider::Gitlab);
    }

    #[test]
    fn provider_deserialize_custom() {
        let json = r#"{"type": "custom", "name": "my-keycloak", "issuerUrl": "https://keycloak.example.com/realms/myrealm"}"#;
        let provider = serde_json::from_str::<Provider>(json).unwrap();
        assert_eq!(provider, Provider::Custom {
            name: "my-keycloak".to_string(),
            issuer_url: "https://keycloak.example.com/realms/myrealm".to_string(),
        });
    }

    #[test]
    fn provider_deserialize_custom_missing_name() {
        let json = r#"{"type": "custom", "issuerUrl": "https://example.com"}"#;
        assert!(serde_json::from_str::<Provider>(json).is_err());
    }

    #[test]
    fn provider_deserialize_custom_missing_issuer_url() {
        let json = r#"{"type": "custom", "name": "foo"}"#;
        assert!(serde_json::from_str::<Provider>(json).is_err());
    }

    #[test]
    fn provider_custom_validation_allows_http() {
        assert!(Provider::custom("test".into(), "http://example.com".into()).is_ok());
    }

    #[test]
    fn provider_custom_validation_allows_localhost() {
        assert!(Provider::custom("test".into(), "https://localhost/auth".into()).is_ok());
    }

    #[test]
    fn provider_custom_validation_allows_query() {
        assert!(Provider::custom("test".into(), "https://example.com?foo=bar".into()).is_ok());
    }

    #[test]
    fn provider_custom_validation_accepts_valid_url() {
        assert!(Provider::custom("test".into(), "https://keycloak.example.com/realms/myrealm".into()).is_ok());
    }

    #[test]
    fn provider_strict_validation_rejects_http() {
        let provider = Provider::custom("test".into(), "http://example.com".into()).unwrap();
        assert!(provider.validate_issuer_url_strict().is_err());
    }

    #[test]
    fn provider_strict_validation_rejects_localhost() {
        let provider = Provider::custom("test".into(), "https://localhost/auth".into()).unwrap();
        assert!(provider.validate_issuer_url_strict().is_err());
    }

    #[test]
    fn provider_strict_validation_accepts_valid_url() {
        let provider = Provider::custom("test".into(), "https://keycloak.example.com/realms/test".into()).unwrap();
        assert!(provider.validate_issuer_url_strict().is_ok());
    }

    #[test]
    fn provider_strict_validation_passes_for_known() {
        assert!(Provider::Google.validate_issuer_url_strict().is_ok());
    }

    #[test]
    fn provider_issuer_url_known() {
        assert!(Provider::Google.issuer_url().is_ok());
        assert!(Provider::Facebook.issuer_url().is_ok());
        assert!(Provider::Microsoft.issuer_url().is_ok());
        assert!(Provider::Gitlab.issuer_url().is_ok());
    }

    #[test]
    fn provider_issuer_url_custom() {
        let provider = Provider::Custom {
            name: "test".to_string(),
            issuer_url: "https://keycloak.example.com/realms/test".to_string(),
        };
        let url = provider.issuer_url().unwrap();
        assert_eq!(url.url().as_str(), "https://keycloak.example.com/realms/test");
    }

    #[test]
    fn provider_display() {
        assert_eq!(Provider::Google.to_string(), "google");
        assert_eq!(Provider::Custom {
            name: "my-kc".to_string(),
            issuer_url: "https://kc.example.com".to_string(),
        }.to_string(), "custom:my-kc");
    }

    #[test]
    fn provider_kind_round_trip() {
        assert_eq!(Provider::Google.kind(), ProviderKind::Google);
        assert_eq!(Provider::Custom {
            name: "x".into(),
            issuer_url: "https://x.com".into(),
        }.kind(), ProviderKind::Custom);
    }

    #[test]
    fn provider_serde_round_trip() {
        for provider in [Provider::Google, Provider::Facebook, Provider::Microsoft, Provider::Gitlab] {
            let json = serde_json::to_string(&provider).unwrap();
            let back: Provider = serde_json::from_str(&json).unwrap();
            assert_eq!(provider, back);
        }

        let custom = Provider::Custom {
            name: "test".into(),
            issuer_url: "https://test.example.com".into(),
        };
        let json = serde_json::to_string(&custom).unwrap();
        let back: Provider = serde_json::from_str(&json).unwrap();
        assert_eq!(custom, back);
    }
}

mod protobuf {
    use super::Provider;
    use golem_api_grpc::proto::golem::registry::security_scheme_provider::Kind as GrpcProviderKind;
    use golem_api_grpc::proto::golem::registry::security_scheme_provider::{
        CustomProvider, Facebook, Gitlab, Google, Microsoft,
    };
    use golem_api_grpc::proto::golem::registry::SecuritySchemeProvider as GrpcProvider;

    impl From<Provider> for GrpcProvider {
        fn from(value: Provider) -> Self {
            let kind = match value {
                Provider::Google => GrpcProviderKind::Google(Google {}),
                Provider::Facebook => GrpcProviderKind::Facebook(Facebook {}),
                Provider::Microsoft => GrpcProviderKind::Microsoft(Microsoft {}),
                Provider::Gitlab => GrpcProviderKind::Gitlab(Gitlab {}),
                Provider::Custom { name, issuer_url } => {
                    GrpcProviderKind::Custom(CustomProvider { name, issuer_url })
                }
            };
            GrpcProvider { kind: Some(kind) }
        }
    }

    impl TryFrom<GrpcProvider> for Provider {
        type Error = String;

        fn try_from(value: GrpcProvider) -> Result<Self, String> {
            match value.kind.ok_or("SecuritySchemeProvider.kind is missing")? {
                GrpcProviderKind::Google(_) => Ok(Self::Google),
                GrpcProviderKind::Facebook(_) => Ok(Self::Facebook),
                GrpcProviderKind::Microsoft(_) => Ok(Self::Microsoft),
                GrpcProviderKind::Gitlab(_) => Ok(Self::Gitlab),
                GrpcProviderKind::Custom(c) => Self::custom(c.name, c.issuer_url),
            }
        }
    }
}
