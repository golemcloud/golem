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
//! spelling only.
//!
//! A URL is `https://`, then a host, then an optional port, then an optional path:
//!
//! - the host is a lower-case DNS name (labels of `a-z`, `0-9` and `-`, 1 to 63 characters, no
//!   label starting or ending with `-`, at most 253 characters, no trailing dot), an IPv4
//!   address, or an IPv6 address in brackets as the `url` crate writes it;
//! - the port is from 1 to 65535 with no leading zero, and never the default `443`;
//! - the path has non-empty segments of `A-Z a-z 0-9 - . _ ~` only (not `.` or `..`), with or
//!   without one final `/`;
//! - there is no `%`, no other character, no user info, no query and no fragment.
//!
//! A bare host may be written with or without the final `/`, and the two are two different
//! issuers: identities compare byte for byte, so `https://id.acme.example` and
//! `https://id.acme.example/` never match. Pick one spelling and use it everywhere. A value that
//! differs only by case or by the default port is refused with the spelling it should have.
//!
//! The other form is `urn:golem:issuer:<uuid>`, with the UUID in lower case with hyphens, as
//! [`GolemIssuer::generate`] writes it. A generated value that was stored is accepted back
//! unchanged.
//!
//! AWS IAM documents for an OIDC provider URL that it "should not contain a port number" and
//! that it has at most 255 characters. The parser allows a port, because a self-hosted
//! installation may need one; an installation whose issuer AWS must trust should use a URL
//! without a port. This is documented, not checked here.
//!
//! The setting reader [`deserialize_setting`] treats a missing, empty or white-space-only value
//! as not set.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::net::Ipv4Addr;
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

        parse_https_issuer(text)?;
        Ok(Self(text.to_string()))
    }
}

/// Checks `text` against the strict `https` rule from the module comment.
///
/// User info, query and fragment are refused first, with a message that does not repeat the
/// value, because the value may hold a password.
fn parse_https_issuer(text: &str) -> Result<(), GolemIssuerError> {
    if text.contains(['@', '?', '#']) {
        return Err(GolemIssuerError::UnsupportedUrlPart);
    }
    let Some(after_scheme) = text.strip_prefix("https://") else {
        return Err(GolemIssuerError::Unsupported(text.to_string()));
    };

    let (authority, path) = match after_scheme.find('/') {
        Some(index) => after_scheme.split_at(index),
        None => (after_scheme, ""),
    };

    let (host, port) = split_host_and_port(text, authority)?;
    let normal_host = check_host(text, host)?;
    let default_port = match port {
        Some(port) => check_port(text, port)? == 443,
        None => false,
    };
    check_path(text, path)?;

    if normal_host != host || default_port {
        let port_part = match port {
            Some(port) if !default_port => format!(":{port}"),
            _ => String::new(),
        };
        return Err(GolemIssuerError::NotNormalForm {
            value: text.to_string(),
            normal_form: format!("https://{normal_host}{port_part}{path}"),
        });
    }
    Ok(())
}

fn split_host_and_port<'a>(
    text: &str,
    authority: &'a str,
) -> Result<(&'a str, Option<&'a str>), GolemIssuerError> {
    let invalid_host = || GolemIssuerError::InvalidHost {
        value: text.to_string(),
        reason: "the host must be a DNS name, an IPv4 address or a bracketed IPv6 address",
    };
    if authority.starts_with('[') {
        let end = authority.find(']').ok_or_else(invalid_host)?;
        let (host, rest) = authority.split_at(end + 1);
        return match rest {
            "" => Ok((host, None)),
            _ => match rest.strip_prefix(':') {
                Some(port) => Ok((host, Some(port))),
                None => Err(invalid_host()),
            },
        };
    }
    match authority.split_once(':') {
        Some((host, port)) => Ok((host, Some(port))),
        None => Ok((authority, None)),
    }
}

/// Returns the host in its normal spelling (lower case), or the reason it is refused.
fn check_host(text: &str, host: &str) -> Result<String, GolemIssuerError> {
    let invalid = |reason: &'static str| GolemIssuerError::InvalidHost {
        value: text.to_string(),
        reason,
    };

    if host.starts_with('[') {
        // The URL parser writes the one spelling of an IPv6 address that this accepts.
        let written = Url::parse(&format!("https://{host}"))
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .ok_or_else(|| invalid("the bracketed host is not an IPv6 address"))?;
        return if written == host.to_ascii_lowercase() {
            Ok(written)
        } else {
            Err(invalid(
                "an IPv6 address must be written in lower case and in its shortest form",
            ))
        };
    }

    let lower = host.to_ascii_lowercase();
    if lower.is_empty() {
        return Err(invalid("the host is empty"));
    }
    if lower.len() > 253 {
        return Err(invalid("a host may have at most 253 characters"));
    }
    let labels: Vec<&str> = lower.split('.').collect();
    for label in &labels {
        if label.is_empty() {
            return Err(invalid(
                "a host may have no empty label (no leading, trailing or double dot)",
            ));
        }
        if label.len() > 63 {
            return Err(invalid("a host label may have at most 63 characters"));
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid(
                "the host must be a DNS name of letters, digits and - (or an IP address)",
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(invalid("a host label may not start or end with -"));
        }
    }

    // A last label that is a number makes the URL parser read the host as an IPv4 address.
    let last = labels[labels.len() - 1];
    let hex_number = last
        .strip_prefix("0x")
        .is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_hexdigit()));
    if last.bytes().all(|b| b.is_ascii_digit()) || hex_number {
        return match lower.parse::<Ipv4Addr>() {
            Ok(ip) if ip.to_string() == lower && host == lower => Ok(lower),
            _ => Err(invalid(
                "a host that ends in a number must be an IPv4 address of four plain decimal parts",
            )),
        };
    }
    Ok(lower)
}

fn check_port(text: &str, port: &str) -> Result<u16, GolemIssuerError> {
    let invalid = |reason: &'static str| GolemIssuerError::InvalidPort {
        value: text.to_string(),
        reason,
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("a port is digits only"));
    }
    if port.starts_with('0') {
        return Err(invalid("a port is from 1 to 65535 with no leading zero"));
    }
    match port.parse::<u16>() {
        Ok(port) => Ok(port),
        Err(_) => Err(invalid("a port is from 1 to 65535 with no leading zero")),
    }
}

/// `path` is empty, or starts with `/`: one final `/` is allowed, and every segment is
/// non-empty and holds only `A-Z a-z 0-9 - . _ ~`.
fn check_path(text: &str, path: &str) -> Result<(), GolemIssuerError> {
    if path.is_empty() || path == "/" {
        return Ok(());
    }
    let invalid = |reason: &'static str| GolemIssuerError::InvalidPath {
        value: text.to_string(),
        reason,
    };
    let body = &path[1..];
    let body = body.strip_suffix('/').unwrap_or(body);
    for segment in body.split('/') {
        if segment.is_empty() {
            return Err(invalid("a path segment may not be empty (no // in a path)"));
        }
        if !segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
        {
            return Err(invalid(
                "a path may hold only letters, digits and - . _ ~ (no % escapes)",
            ));
        }
        if segment == "." || segment == ".." {
            return Err(invalid("a path may not hold the segments . or .."));
        }
    }
    Ok(())
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

/// Reads an optional `golem_issuer` setting: a missing value, an empty string or a string of
/// only white space is "not set" (`None`), and any other value must parse as a [`GolemIssuer`].
///
/// Use it as `#[serde(default, deserialize_with = "golem_common::model::golem_issuer::deserialize_setting")]`.
pub fn deserialize_setting<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<GolemIssuer>, D::Error> {
    match Option::<String>::deserialize(deserializer)? {
        None => Ok(None),
        Some(text) if text.trim().is_empty() => Ok(None),
        Some(text) => text.parse().map(Some).map_err(serde::de::Error::custom),
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
    /// Carries no value on purpose: the value may hold a password.
    #[error("the Golem issuer must have no user name, password, query or fragment")]
    UnsupportedUrlPart,
    #[error("the Golem issuer {value:?} has an invalid host: {reason}")]
    InvalidHost { value: String, reason: &'static str },
    #[error("the Golem issuer {value:?} has an invalid port: {reason}")]
    InvalidPort { value: String, reason: &'static str },
    #[error("the Golem issuer {value:?} has an invalid path: {reason}")]
    InvalidPath { value: String, reason: &'static str },
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
        "https://id.acme.example:1",
        "https://id.acme.example:65535/",
        "https://10.1.2.3",
        "https://10.1.2.3:9000/golem",
        "https://[2001:db8::1]:8443",
        "https://[::1]",
        "https://[2001:db8::1]/golem/",
        "https://localhost",
        "https://a-b.c1.example",
        "https://xn--bcher-kva.example",
        "https://id.acme.example/Golem-1.0/a_b/c~d",
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
        logs_assert(|lines: &[&str]| {
            let at_warn = lines.iter().any(|line| {
                line.contains("WARN")
                    && line.contains("Identity proofs to outside receivers need an https issuer")
            });
            if at_warn {
                Ok(())
            } else {
                Err(format!("no WARN line with the message in {lines:?}"))
            }
        });
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
            "https://id.acme.example?".to_string(),
            "https://id.acme.example#".to_string(),
            "https://@id.acme.example".to_string(),
            "https://id.acme.example/%7e".to_string(),
            "https://id.acme.example/%7E".to_string(),
            "https://id.acme.example/%41".to_string(),
            "https://a{b}.example".to_string(),
            "https://a\"b.example".to_string(),
            "https://a`b.example".to_string(),
            "https://id.acme.example/a|b".to_string(),
            "https://id.acme.example/a^b".to_string(),
            "https://id.acme.example/[x]".to_string(),
            "https://id.acme.example/a b".to_string(),
            "https://id.acme.example/a*b".to_string(),
            "https://id.acme.example/a!b".to_string(),
            "https://id.acme.example/a:b".to_string(),
            "https://id.acme.example/./a".to_string(),
            "https://id.acme.example/a/./b".to_string(),
            "https://id.acme.example/..".to_string(),
            "https://id.acme.example//".to_string(),
            "https://id.acme.example//a".to_string(),
            "https://id.acme.example/a//b".to_string(),
            "https://id.acme.example/a//".to_string(),
            "https://!.example".to_string(),
            "https://*.example".to_string(),
            "https://*.".to_string(),
            "https://my_host.example".to_string(),
            "https://id.acme.example.".to_string(),
            "https://.id.acme.example".to_string(),
            "https://id..acme.example".to_string(),
            "https://-id.acme.example".to_string(),
            "https://id-.acme.example".to_string(),
            "https://".to_string(),
            "https:///golem".to_string(),
            "https://id.acme.example:0".to_string(),
            "https://id.acme.example:0/".to_string(),
            "https://id.acme.example:08443".to_string(),
            "https://id.acme.example:65536".to_string(),
            "https://id.acme.example:".to_string(),
            "https://id.acme.example:80a".to_string(),
            "https://id.acme.example:-1".to_string(),
            "https://id.acme.example:+8443".to_string(),
            "https://id.acme.example:8443:9".to_string(),
            "https://[::1".to_string(),
            "https://[::1]x".to_string(),
            "https://[::1]:0".to_string(),
            "https://[0:0:0:0:0:0:0:1]".to_string(),
            "https://[::FFFF:1.2.3.4]".to_string(),
            "https://[2001:DB8::1]".to_string(),
            "https://[nonsense]".to_string(),
            "https://10.1.2".to_string(),
            "https://010.1.2.3".to_string(),
            "https://256.1.2.3".to_string(),
            "https://0x7f.1".to_string(),
            "https://1.2.3.4.5".to_string(),
            "https://2130706433".to_string(),
            "https://exa mple.com".to_string(),
            "https://id.acme.example\t".to_string(),
            "https://id.acme.example/\n".to_string(),
            "https://id.acme.example\\golem".to_string(),
            format!("https://{}.example", "a".repeat(64)),
            format!("https://{}", vec!["a".repeat(63); 4].join(".")),
            format!("https://id.acme.example/{}%", "a".repeat(10)),
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

    #[test]
    fn a_case_or_default_port_slip_names_the_value_it_should_be() {
        for (text, normal_form) in [
            ("https://ID.acme.example", "https://id.acme.example"),
            (
                "https://Id.Acme.Example/Golem",
                "https://id.acme.example/Golem",
            ),
            ("https://id.acme.example:443", "https://id.acme.example"),
            ("https://id.acme.example:443/", "https://id.acme.example/"),
            (
                "https://ID.acme.example:443/golem/",
                "https://id.acme.example/golem/",
            ),
        ] {
            assert_eq!(
                text.parse::<GolemIssuer>(),
                Err(GolemIssuerError::NotNormalForm {
                    value: text.to_string(),
                    normal_form: normal_form.to_string()
                }),
                "{text}"
            );
        }
    }

    #[test]
    fn a_host_of_253_characters_and_labels_of_63_are_accepted() {
        let host = [63, 63, 63, 61].map(|n| "a".repeat(n)).join(".");
        assert_eq!(host.len(), 253);
        let text = format!("https://{host}");
        assert!(text.parse::<GolemIssuer>().is_ok());
    }

    #[test]
    fn a_bare_host_with_and_without_a_slash_are_two_issuers() {
        let without: GolemIssuer = "https://id.acme.example".parse().unwrap();
        let with: GolemIssuer = "https://id.acme.example/".parse().unwrap();
        assert_ne!(without, with);
    }

    #[test]
    fn a_refusal_never_contains_the_password() {
        for text in [
            "https://svc:Sup3rS3cret@id.acme.example",
            "https://svc:Sup3rS3cret@id.acme.example/golem",
            "https://:Sup3rS3cret@id.acme.example",
            "https://id.acme.example/?token=Sup3rS3cret",
            "https://id.acme.example/#Sup3rS3cret",
            "http://svc:Sup3rS3cret@id.acme.example",
            " https://svc:Sup3rS3cret@ID.acme.example:443",
            "svc:Sup3rS3cret@id.acme.example",
        ] {
            let error = text.parse::<GolemIssuer>().unwrap_err().to_string();
            assert!(!error.contains("Sup3rS3cret"), "{text}: {error}");
        }
    }

    #[test]
    fn the_setting_reader_treats_missing_empty_and_blank_as_not_set() {
        #[derive(Debug, Deserialize)]
        struct Config {
            #[serde(default, deserialize_with = "deserialize_setting")]
            golem_issuer: Option<GolemIssuer>,
        }
        let read = |json: &str| serde_json::from_str::<Config>(json).map(|c| c.golem_issuer);

        assert_eq!(read("{}").unwrap(), None);
        assert_eq!(read(r#"{"golem_issuer":null}"#).unwrap(), None);
        assert_eq!(read(r#"{"golem_issuer":""}"#).unwrap(), None);
        assert_eq!(read(r#"{"golem_issuer":"   "}"#).unwrap(), None);
        assert_eq!(read(r#"{"golem_issuer":" \t\n "}"#).unwrap(), None);
        assert_eq!(
            read(r#"{"golem_issuer":"https://id.acme.example"}"#).unwrap(),
            Some("https://id.acme.example".parse().unwrap())
        );
        let generated = GolemIssuer::generate();
        assert_eq!(
            read(&format!(r#"{{"golem_issuer":"{generated}"}}"#)).unwrap(),
            Some(generated)
        );
        for refused in [
            "http://id.acme.example",
            "https://ID.acme.example",
            "https://id.acme.example/%7e",
            " https://id.acme.example",
            "local",
        ] {
            let json = format!(r#"{{"golem_issuer":"{refused}"}}"#);
            assert!(read(&json).is_err(), "{refused:?} was accepted");
        }
        assert!(read(r#"{"golem_issuer":5}"#).is_err());
    }
}
