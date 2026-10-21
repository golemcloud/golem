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

use super::*;
use test_r::test;

#[test]
fn provider_serialize_known() {
    let json = serde_json::to_value(Provider::Google(Empty {})).unwrap();
    assert_eq!(json["type"], "Google");
    let json = serde_json::to_value(Provider::Facebook(Empty {})).unwrap();
    assert_eq!(json["type"], "Facebook");
    let json = serde_json::to_value(Provider::Microsoft(Empty {})).unwrap();
    assert_eq!(json["type"], "Microsoft");
    let json = serde_json::to_value(Provider::Gitlab(Empty {})).unwrap();
    assert_eq!(json["type"], "Gitlab");
}

#[test]
fn provider_serialize_custom() {
    let provider = Provider::Custom(CustomProvider {
        name: "my-keycloak".to_string(),
        issuer_url: "https://keycloak.example.com/realms/myrealm".to_string(),
    });
    let json = serde_json::to_value(&provider).unwrap();
    assert_eq!(json["type"], "Custom");
    assert_eq!(json["name"], "my-keycloak");
    assert_eq!(
        json["issuerUrl"],
        "https://keycloak.example.com/realms/myrealm"
    );
}

#[test]
fn provider_deserialize_known() {
    let json = r#"{"type": "Google"}"#;
    assert_eq!(
        serde_json::from_str::<Provider>(json).unwrap(),
        Provider::Google(Empty {})
    );
    let json = r#"{"type": "Facebook"}"#;
    assert_eq!(
        serde_json::from_str::<Provider>(json).unwrap(),
        Provider::Facebook(Empty {})
    );
    let json = r#"{"type": "Microsoft"}"#;
    assert_eq!(
        serde_json::from_str::<Provider>(json).unwrap(),
        Provider::Microsoft(Empty {})
    );
    let json = r#"{"type": "Gitlab"}"#;
    assert_eq!(
        serde_json::from_str::<Provider>(json).unwrap(),
        Provider::Gitlab(Empty {})
    );
}

#[test]
fn provider_deserialize_custom() {
    let json = r#"{"type": "Custom", "name": "my-keycloak", "issuerUrl": "https://keycloak.example.com/realms/myrealm"}"#;
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
    let json = r#"{"type": "Custom", "issuerUrl": "https://example.com"}"#;
    assert!(serde_json::from_str::<Provider>(json).is_err());
}

#[test]
fn provider_deserialize_custom_missing_issuer_url() {
    let json = r#"{"type": "Custom", "name": "foo"}"#;
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
    assert!(
        Provider::Google(Empty {})
            .validate_issuer_url_strict()
            .is_ok()
    );
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
fn provider_serialization_poem_serde_equivalence_known() {
    use poem_openapi::types::ToJSON;

    for provider in [
        Provider::Google(Empty {}),
        Provider::Facebook(Empty {}),
        Provider::Microsoft(Empty {}),
        Provider::Gitlab(Empty {}),
    ] {
        let serialized = provider.to_json_string();
        let deserialized: Provider = serde_json::from_str(&serialized).unwrap();
        assert_eq!(provider, deserialized);
    }
}

#[test]
fn provider_serialization_poem_serde_equivalence_custom() {
    use poem_openapi::types::ToJSON;

    let provider = Provider::Custom(CustomProvider {
        name: "my-keycloak".to_string(),
        issuer_url: "https://keycloak.example.com/realms/myrealm".to_string(),
    });
    let serialized = provider.to_json_string();
    let deserialized: Provider = serde_json::from_str(&serialized).unwrap();
    assert_eq!(provider, deserialized);
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
