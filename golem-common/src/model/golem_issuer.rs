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

//! The installation's Golem issuer: the issuer of the identities of its agents, tools and
//! accounts, and of their identity proofs.
//!
//! The identifier is an `https` URL, or `urn:golem:issuer:<uuid>` when the operator sets none and
//! Golem generates one. An outside receiver can fetch keys only from an `https` issuer.
//!
//! Identities compare their issuer byte for byte, so the parser accepts each identifier in one
//! spelling only: a URL as the URL parser writes it (a missing `/` after the host is allowed), and
//! the UUID in lowercase with hyphens, as [`GolemIssuer::generate`] writes it. A generated value
//! that was stored is accepted back unchanged.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use tracing::warn;
use url::Url;
use uuid::Uuid;

pub const GENERATED_GOLEM_ISSUER_PREFIX: &str = "urn:golem:issuer:";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GolemIssuer(String);

impl GolemIssuer {
    /// A new `urn:golem:issuer:<uuid>` identifier with a random UUID.
    pub fn generate() -> Self {
        Self(format!(
            "{GENERATED_GOLEM_ISSUER_PREFIX}{}",
            Uuid::new_v4().hyphenated()
        ))
    }

    /// The configured issuer, or a generated one with a warning when none is configured.
    ///
    /// Call this once per start and keep the result: every call without a configured issuer
    /// generates a different one.
    pub fn configured_or_generated(configured: Option<GolemIssuer>) -> Self {
        configured.unwrap_or_else(|| {
            let generated = Self::generate();
            warn!(
                "No Golem issuer is configured (golem_issuer), so this process generated {generated}. \
                 Identity proofs to outside receivers need an https issuer, and every worker \
                 service and executor of one installation needs the same issuer: set golem_issuer \
                 to an https URL on its own host"
            );
            generated
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True for a `urn:golem:issuer:<uuid>` identifier, false for an `https` URL.
    pub fn is_generated(&self) -> bool {
        self.0.starts_with(GENERATED_GOLEM_ISSUER_PREFIX)
    }
}

impl Display for GolemIssuer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for GolemIssuer {
    type Err = GolemIssuerError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Some(uuid_text) = text.strip_prefix(GENERATED_GOLEM_ISSUER_PREFIX) {
            return match Uuid::parse_str(uuid_text) {
                Ok(uuid) if uuid.hyphenated().to_string() == uuid_text => {
                    Ok(Self(text.to_string()))
                }
                _ => Err(GolemIssuerError::InvalidGeneratedIdentifier(
                    text.to_string(),
                )),
            };
        }

        let url = match Url::parse(text) {
            Ok(url) if url.scheme() == "https" => url,
            _ => return Err(GolemIssuerError::Unsupported(text.to_string())),
        };
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(GolemIssuerError::UnsupportedUrlPart(text.to_string()));
        }

        // The URL parser adds a `/` after a bare host, so both spellings are its own.
        let written = url.as_str();
        let bare_host = url.path() == "/" && !text.ends_with('/');
        let normal_form = if bare_host {
            written.trim_end_matches('/')
        } else {
            written
        };
        if text != normal_form {
            return Err(GolemIssuerError::NotNormalForm {
                value: text.to_string(),
                normal_form: normal_form.to_string(),
            });
        }
        Ok(Self(text.to_string()))
    }
}

impl Serialize for GolemIssuer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for GolemIssuer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GolemIssuerError {
    #[error("the Golem issuer {0:?} is neither an https URL nor urn:golem:issuer:<uuid>")]
    Unsupported(String),
    #[error(
        "the Golem issuer {0:?} must end in a UUID in lowercase with hyphens after urn:golem:issuer:"
    )]
    InvalidGeneratedIdentifier(String),
    #[error("the Golem issuer {0:?} must have no user name, password, query or fragment")]
    UnsupportedUrlPart(String),
    #[error("the Golem issuer {value:?} must be written as {normal_form:?}")]
    NotNormalForm { value: String, normal_form: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use test_r::test;
    use tracing_test::traced_test;
    use uuid::Uuid;

    const ACCEPTED_URLS: &[&str] = &[
        "https://id.acme.example",
        "https://id.acme.example/",
        "https://id.acme.example/golem",
        "https://id.acme.example/golem/",
        "https://id.acme.example:8443/golem/issuer",
        "https://10.1.2.3",
        "https://[2001:db8::1]:8443",
    ];

    #[test]
    fn a_configured_https_url_is_the_issuer_exactly_as_written() {
        for text in ACCEPTED_URLS {
            let issuer: GolemIssuer = text
                .parse()
                .unwrap_or_else(|error| panic!("{text:?} was refused: {error}"));
            assert_eq!(issuer.as_str(), *text);
            assert_eq!(issuer.to_string(), *text);
            assert!(!issuer.is_generated(), "{text:?}");
        }
    }

    #[test]
    fn a_generated_issuer_is_a_urn_with_a_lowercase_random_uuid() {
        let issuer = GolemIssuer::generate();
        let uuid_text = issuer
            .as_str()
            .strip_prefix("urn:golem:issuer:")
            .unwrap_or_else(|| panic!("{issuer} has no urn:golem:issuer: prefix"));
        let uuid = Uuid::parse_str(uuid_text).unwrap();
        assert_eq!(uuid.hyphenated().to_string(), uuid_text);
        assert_eq!(uuid.get_version(), Some(uuid::Version::Random));
        assert!(issuer.is_generated());
        assert_ne!(GolemIssuer::generate(), issuer);
    }

    #[test]
    fn a_stored_generated_issuer_is_accepted_as_configuration() {
        let generated = GolemIssuer::generate();
        let configured: GolemIssuer = generated.as_str().parse().unwrap();
        assert_eq!(configured, generated);
        assert!(configured.is_generated());
    }

    #[test]
    #[traced_test]
    fn no_value_gives_a_generated_issuer_and_the_warning() {
        let issuer = GolemIssuer::configured_or_generated(None);
        assert!(issuer.is_generated());
        assert!(logs_contain(issuer.as_str()));
        assert!(logs_contain(
            "Identity proofs to outside receivers need an https issuer"
        ));
    }

    #[test]
    #[traced_test]
    fn a_configured_value_is_kept_without_a_warning() {
        let configured: GolemIssuer = "https://id.acme.example".parse().unwrap();
        let issuer = GolemIssuer::configured_or_generated(Some(configured.clone()));
        assert_eq!(issuer, configured);
        assert!(!logs_contain("Identity proofs"));
    }

    #[test]
    fn every_other_value_is_refused() {
        let uuid = "0f3a5c21-8d4e-4b6f-9a1c-2e7d4b6a9c10";
        let refused = [
            "".to_string(),
            "id.acme.example".to_string(),
            "http://id.acme.example".to_string(),
            "ftp://id.acme.example".to_string(),
            "local-worker-service".to_string(),
            "HTTPS://id.acme.example".to_string(),
            "https://ID.acme.example".to_string(),
            "https://id.acme.example:443".to_string(),
            "https://id.acme.example/a/../b".to_string(),
            " https://id.acme.example".to_string(),
            "https://id.acme.example ".to_string(),
            "https://user:secret@id.acme.example".to_string(),
            "https://id.acme.example/?tenant=a".to_string(),
            "https://id.acme.example/#a".to_string(),
            "urn:golem:issuer:".to_string(),
            "urn:golem:issuer:local".to_string(),
            format!("urn:golem:issuer:{}", uuid.to_uppercase()),
            format!("urn:golem:issuer:{}", uuid.replace('-', "")),
            format!("urn:golem:issuer:{{{uuid}}}"),
            format!("urn:golem:issuer:{uuid}/x"),
            format!("urn:golem:other:{uuid}"),
            format!("URN:golem:issuer:{uuid}"),
        ];
        for text in &refused {
            assert!(
                text.parse::<GolemIssuer>().is_err(),
                "{text:?} was accepted"
            );
        }
    }

    #[test]
    fn a_refused_value_fails_the_configuration_and_names_the_normal_form() {
        #[derive(Debug, Deserialize)]
        struct Config {
            golem_issuer: Option<GolemIssuer>,
        }

        let read = |json: &str| serde_json::from_str::<Config>(json).map(|c| c.golem_issuer);

        assert_eq!(read("{}").unwrap(), None);
        assert_eq!(
            read(r#"{"golem_issuer":"https://id.acme.example"}"#).unwrap(),
            Some("https://id.acme.example".parse().unwrap())
        );
        let error = read(r#"{"golem_issuer":"http://id.acme.example"}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("http://id.acme.example"), "{error}");
        let error = read(r#"{"golem_issuer":"https://ID.acme.example:443"}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains(r#""https://id.acme.example""#), "{error}");
    }
}
