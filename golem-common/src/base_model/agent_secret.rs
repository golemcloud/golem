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

use super::optional_field_update::OptionalFieldUpdate;
#[cfg(feature = "full")]
use crate::base_model::canonicalize_agent_path;
use crate::base_model::card::{host_pattern_subsumes, normalize_host_pattern};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::render_config_path;
use crate::base_model::tool::ToolName;
use crate::base_model::{AgentFingerprint, AgentId};
use crate::schema::{SchemaGraph, SchemaValue};
use crate::{declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::str::FromStr;

newtype_uuid!(
    AgentSecretId,
    golem_api_grpc::proto::golem::registry::AgentSecretId
);

declare_revision!(AgentSecretRevision);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSecretOwner {
    Environment,
    Agent {
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
    },
    Tool {
        tool_name: ToolName,
    },
}

impl From<AgentSecretOwner> for golem_api_grpc::proto::golem::registry::AgentSecretOwner {
    fn from(value: AgentSecretOwner) -> Self {
        use golem_api_grpc::proto::golem::registry::{
            AgentSecretAgentOwner, agent_secret_owner::Owner,
        };
        Self {
            owner: Some(match value {
                AgentSecretOwner::Environment => Owner::Environment(()),
                AgentSecretOwner::Agent {
                    agent_id,
                    fingerprint,
                } => Owner::Agent(AgentSecretAgentOwner {
                    agent_id: Some(agent_id.into()),
                    fingerprint: Some(fingerprint.0.into()),
                }),
                AgentSecretOwner::Tool { tool_name } => Owner::ToolName(tool_name.to_string()),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::AgentSecretOwner> for AgentSecretOwner {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::AgentSecretOwner,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::registry::agent_secret_owner::Owner;
        match value.owner.ok_or("Missing secret owner")? {
            Owner::Environment(()) => Ok(Self::Environment),
            Owner::Agent(owner) => {
                let agent_id: AgentId =
                    owner.agent_id.ok_or("Missing owner agent ID")?.try_into()?;
                if !agent_id.is_issued() || agent_id.type_id().is_none() {
                    return Err("Invalid secret agent owner identity".to_string());
                }
                Ok(Self::Agent {
                    agent_id,
                    fingerprint: AgentFingerprint(
                        owner.fingerprint.ok_or("Missing owner fingerprint")?.into(),
                    ),
                })
            }
            Owner::ToolName(name) => Ok(Self::Tool {
                tool_name: ToolName::try_from(name.as_str())?,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSecretRevisionSelector {
    Current,
    Fixed(AgentSecretRevision),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSecretReadMode {
    Live,
    Historical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretDestinationScheme {
    Http,
    Https,
    Ws,
    Wss,
}

impl SecretDestinationScheme {
    fn default_port(self) -> u16 {
        match self {
            Self::Http | Self::Ws => 80,
            Self::Https | Self::Wss => 443,
        }
    }
}

impl FromStr for SecretDestinationScheme {
    type Err = SecretDestinationParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            "ws" => Ok(Self::Ws),
            "wss" => Ok(Self::Wss),
            _ => Err(SecretDestinationParseError),
        }
    }
}

impl Display for SecretDestinationScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Ws => "ws",
            Self::Wss => "wss",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum SecretDestination {
    Any,
    Origin {
        scheme: SecretDestinationScheme,
        host: String,
        port: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Invalid secret destination: expected * or an http, https, ws or wss origin")]
pub struct SecretDestinationParseError;

fn normalize_destination_host(host: &str) -> Result<String, SecretDestinationParseError> {
    let host = normalize_host_pattern(host, host).map_err(|_| SecretDestinationParseError)?;
    // The card matcher defines wildcard semantics, but does not validate URL host syntax.
    url::Host::parse(&host.replace('*', "wildcard")).map_err(|_| SecretDestinationParseError)?;
    Ok(host)
}

impl FromStr for SecretDestination {
    type Err = SecretDestinationParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "*" {
            return Ok(Self::Any);
        }
        let (scheme, authority) = value.split_once("://").ok_or(SecretDestinationParseError)?;
        let scheme = scheme.parse::<SecretDestinationScheme>()?;
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        if authority.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(
                    c,
                    '/' | '\\' | '?' | '#' | '@' | '[' | ']' | '<' | '>' | '^' | '|' | '%'
                )
        }) {
            return Err(SecretDestinationParseError);
        }
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => {
                if port.is_empty() || !port.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(SecretDestinationParseError);
                }
                (
                    host,
                    Some(
                        port.parse::<u16>()
                            .map_err(|_| SecretDestinationParseError)?,
                    ),
                )
            }
            None => (authority, None),
        };
        let host = normalize_destination_host(host)?;
        if host == "*"
            || (matches!(
                scheme,
                SecretDestinationScheme::Http | SecretDestinationScheme::Ws
            ) && host.contains('*'))
        {
            return Err(SecretDestinationParseError);
        }
        Ok(Self::Origin {
            scheme,
            host,
            port: port.filter(|port| *port != scheme.default_port()),
        })
    }
}

impl SecretDestination {
    pub fn matches(&self, scheme: SecretDestinationScheme, host: &str, port: Option<u16>) -> bool {
        if host.contains('*')
            || host.chars().any(|c| {
                c.is_control()
                    || matches!(
                        c,
                        '/' | '\\' | '?' | '#' | '@' | '[' | ']' | '<' | '>' | '^' | '|' | '%'
                    )
            })
        {
            return false;
        }
        let Ok(host) = normalize_destination_host(host) else {
            return false;
        };
        let port = port.unwrap_or(scheme.default_port());
        match self {
            Self::Any => {
                matches!(
                    scheme,
                    SecretDestinationScheme::Https | SecretDestinationScheme::Wss
                ) && port == 443
            }
            Self::Origin {
                scheme: expected_scheme,
                host: expected_host,
                port: expected_port,
            } => {
                scheme == *expected_scheme
                    && port == expected_port.unwrap_or(expected_scheme.default_port())
                    && host_pattern_subsumes(expected_host, &host)
            }
        }
    }
}

impl Display for SecretDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Any => f.write_str("*"),
            Self::Origin { scheme, host, port } => {
                write!(f, "{scheme}://{host}")?;
                if let Some(port) = port.filter(|port| *port != scheme.default_port()) {
                    write!(f, ":{port}")?;
                }
                Ok(())
            }
        }
    }
}

impl TryFrom<String> for SecretDestination {
    type Error = SecretDestinationParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<SecretDestination> for String {
    fn from(value: SecretDestination) -> Self {
        value.to_string()
    }
}

declare_transparent_newtypes! {
    /// Agent secret path in any casing. All agent secret paths
    /// are converted to the same casing internally to allow easier cross-language use.
    #[cfg_attr(feature = "full", oai(to_header = false))]
    pub struct AgentSecretPath(pub Vec<String>);

    /// Canonical representation of an agent secret path (segments are each camelCase)
    #[derive(Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    #[cfg_attr(feature = "full", oai(to_header = false))]
    pub struct CanonicalAgentSecretPath(pub Vec<String>);
}

impl CanonicalAgentSecretPath {
    #[cfg(feature = "full")]
    pub fn from_path_in_unknown_casing(value: &[String]) -> Self {
        Self(canonicalize_agent_path(value))
    }
}

impl Display for CanonicalAgentSecretPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", render_config_path(&self.0))
    }
}

#[cfg(feature = "full")]
impl From<AgentSecretPath> for CanonicalAgentSecretPath {
    fn from(value: AgentSecretPath) -> Self {
        Self::from_path_in_unknown_casing(&value.0)
    }
}

declare_structs! {
    pub struct AgentSecretDto {
        pub id: AgentSecretId,
        pub environment_id: EnvironmentId,
        pub path: CanonicalAgentSecretPath,
        pub revision: AgentSecretRevision,
        pub secret_type: SchemaGraph,
        pub secret_value: Option<SchemaValue>,
    }

    pub struct AgentSecretCreation {
        pub path: AgentSecretPath,
        pub secret_type: SchemaGraph,
        pub secret_value: Option<SchemaValue>,
    }

    pub struct AgentSecretUpdate {
        pub current_revision: AgentSecretRevision,
        pub secret_value: OptionalFieldUpdate<SchemaValue>
    }
}

#[cfg(test)]
mod destination {
    use super::*;
    use test_r::test;

    #[test]
    fn matching() {
        use SecretDestinationScheme::*;
        for (destination, scheme, host, port, expected) in [
            ("https://*.example.com", Https, "a.example.com", None, true),
            ("https://*.example.com", Https, "example.com", None, false),
            (
                "https://*.example.com",
                Https,
                "a.b.example.com",
                None,
                false,
            ),
            ("*", Https, "x.org", None, true),
            ("*", Wss, "x.org", None, true),
            ("*", Http, "x.org", None, false),
            ("*", Ws, "x.org", None, false),
            ("*", Https, "x.org", Some(8443), false),
            ("https://api.x.com", Https, "api.x.com", Some(443), true),
            ("https://api.x.com", Https, "api.x.com", Some(8443), false),
            ("https://api.x.com", Http, "api.x.com", None, false),
            ("http://localhost:8080", Http, "localhost", Some(8080), true),
            (
                "http://localhost:8080",
                Http,
                "localhost",
                Some(8081),
                false,
            ),
            ("HTTPS://API.X.COM.", Https, "api.x.com", None, true),
            ("https://*.example.com", Https, "A.Example.COM.", None, true),
            ("https://*.example.com", Https, "*.example.com", None, false),
            ("*", Https, "[::1]", None, false),
            ("*", Https, "", None, false),
            ("*", Https, "example.com\\private", None, false),
            ("*", Https, "exa\0mple.com", None, false),
            ("*", Https, "xn--", None, false),
        ] {
            assert_eq!(
                destination
                    .parse::<SecretDestination>()
                    .unwrap()
                    .matches(scheme, host, port),
                expected,
                "{destination}, {scheme}://{host}:{port:?}"
            );
        }
    }

    #[test]
    fn parsing() {
        for input in [
            "https://x.com:1-2",
            "https://[::1]",
            "ftp://x.com",
            "x.com",
            "http://*",
            "ws://*",
            "https://*",
            "wss://*",
            "http://*.internal",
            "ws://*.internal",
            "https://x.com/path",
            "https://x.com//",
            "https://x.com?x",
            "https://x.com#x",
            "https://user@x.com",
            "https://x.com:abc",
            "https://x.com:",
            "https://x.com:65536",
            "https://",
            "https:// x.com",
            "https://a*b.com",
            "https://example.com\\private",
            "https://exa\0mple.com",
            "https://exa<mple.com",
            "https://exa|mple.com",
            "https://exa%mple.com",
            "https://xn--",
            "https://\u{fffd}.example",
            "https://\u{200d}.example",
        ] {
            assert!(input.parse::<SecretDestination>().is_err(), "{input}");
            assert!(serde_json::from_value::<SecretDestination>(serde_json::json!(input)).is_err());
        }
        for (input, canonical) in [
            ("HTTPS://API.X.COM.:443/", "https://api.x.com"),
            ("https://x.com:443", "https://x.com"),
            ("http://localhost:8080/", "http://localhost:8080"),
            ("ws://localhost:80", "ws://localhost"),
            ("wss://*.example.com:443", "wss://*.example.com"),
            ("https://127.0.0.1", "https://127.0.0.1"),
            ("*", "*"),
        ] {
            let destination: SecretDestination = input.parse().unwrap();
            assert_eq!(destination.to_string(), canonical);
            assert_eq!(serde_json::to_value(&destination).unwrap(), canonical);
            assert_eq!(
                serde_json::from_value::<SecretDestination>(serde_json::json!(input)).unwrap(),
                destination
            );
        }
    }
}
