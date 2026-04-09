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
            Provider::Gitlab(_) => {
                Ok(IssuerUrl::new("https://gitlab.com".to_string()).unwrap())
            }
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
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn provider_serialize_known() {
        let json = serde_json::to_value(&Provider::Google(Empty {})).unwrap();
        assert_eq!(json["type"], "google");
        let json = serde_json::to_value(&Provider::Facebook(Empty {})).unwrap();
        assert_eq!(json["type"], "facebook");
        let json = serde_json::to_value(&Provider::Microsoft(Empty {})).unwrap();
        assert_eq!(json["type"], "microsoft");
        let json = serde_json::to_value(&Provider::Gitlab(Empty {})).unwrap();
        assert_eq!(json["type"], "gitlab");
    }

    #[test]
    fn provider_serialize_custom() {
        let provider = Provider::Custom(CustomProvider {
            name: "my-keycloak".to_string(),
            issuer_url: "https://keycloak.example.com/realms/myrealm".to_string(),
        });
        let json = serde_json::to_value(&provider).unwrap();
        assert_eq!(json["type"], "custom");
        assert_eq!(json["name"], "my-keycloak");
        assert_eq!(
            json["issuerUrl"],
            "https://keycloak.example.com/realms/myrealm"
        );
    }

    #[test]
    fn provider_deserialize_known() {
        let json = r#"{"type": "google"}"#;
        assert_eq!(
            serde_json::from_str::<Provider>(json).unwrap(),
            Provider::Google(Empty {})
        );
        let json = r#"{"type": "facebook"}"#;
        assert_eq!(
            serde_json::from_str::<Provider>(json).unwrap(),
            Provider::Facebook(Empty {})
        );
        let json = r#"{"type": "microsoft"}"#;
        assert_eq!(
            serde_json::from_str::<Provider>(json).unwrap(),
            Provider::Microsoft(Empty {})
        );
        let json = r#"{"type": "gitlab"}"#;
        assert_eq!(
            serde_json::from_str::<Provider>(json).unwrap(),
            Provider::Gitlab(Empty {})
        );
    }

    #[test]
    fn provider_deserialize_custom() {
        let json = r#"{"type": "custom", "name": "my-keycloak", "issuerUrl": "https://keycloak.example.com/realms/myrealm"}"#;
        let provider = serde_json::from_str::<Provider>(json).unwrap();
        assert_eq!(
            provider,
            Provider::Custom(CustomProvider {
                name: "my-keycloak".to_string(),
                issuer_url: "https://keycloak.example.com/realms/myrealm".to_string(),
            })
        );
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
        assert!(
            Provider::custom(
                "test".into(),
                "https://keycloak.example.com/realms/myrealm".into()
            )
            .is_ok()
        );
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
        let provider = Provider::custom(
            "test".into(),
            "https://keycloak.example.com/realms/test".into(),
        )
        .unwrap();
        assert!(provider.validate_issuer_url_strict().is_ok());
    }

    #[test]
    fn provider_strict_validation_passes_for_known() {
        assert!(Provider::Google(Empty {}).validate_issuer_url_strict().is_ok());
    }

    #[test]
    fn provider_issuer_url_known() {
        assert!(Provider::Google(Empty {}).issuer_url().is_ok());
        assert!(Provider::Facebook(Empty {}).issuer_url().is_ok());
        assert!(Provider::Microsoft(Empty {}).issuer_url().is_ok());
        assert!(Provider::Gitlab(Empty {}).issuer_url().is_ok());
    }

    #[test]
    fn provider_issuer_url_custom() {
        let provider = Provider::Custom(CustomProvider {
            name: "test".to_string(),
            issuer_url: "https://keycloak.example.com/realms/test".to_string(),
        });
        let url = provider.issuer_url().unwrap();
        assert_eq!(
            url.url().as_str(),
            "https://keycloak.example.com/realms/test"
        );
    }

    #[test]
    fn provider_display() {
        assert_eq!(Provider::Google(Empty {}).to_string(), "google");
        assert_eq!(
            Provider::Custom(CustomProvider {
                name: "my-kc".to_string(),
                issuer_url: "https://kc.example.com".to_string(),
            })
            .to_string(),
            "custom:my-kc"
        );
    }

    #[test]
    fn provider_kind_round_trip() {
        assert_eq!(Provider::Google(Empty {}).kind(), ProviderKind::Google);
        assert_eq!(
            Provider::Custom(CustomProvider {
                name: "x".into(),
                issuer_url: "https://x.com".into(),
            })
            .kind(),
            ProviderKind::Custom
        );
    }

    #[test]
    fn provider_serde_round_trip() {
        for provider in [
            Provider::Google(Empty {}),
            Provider::Facebook(Empty {}),
            Provider::Microsoft(Empty {}),
            Provider::Gitlab(Empty {}),
        ] {
            let json = serde_json::to_string(&provider).unwrap();
            let back: Provider = serde_json::from_str(&json).unwrap();
            assert_eq!(provider, back);
        }

        let custom = Provider::Custom(CustomProvider {
            name: "test".into(),
            issuer_url: "https://test.example.com".into(),
        });
        let json = serde_json::to_string(&custom).unwrap();
        let back: Provider = serde_json::from_str(&json).unwrap();
        assert_eq!(custom, back);
    }
}

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
                Provider::Custom(custom) => {
                    GrpcProviderKind::Custom(GrpcCustomProvider {
                        name: custom.name,
                        issuer_url: custom.issuer_url,
                    })
                }
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
