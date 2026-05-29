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

use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EmptyOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PortPattern {
    Any,
    Single(u16),
    Range { start: u16, end: u16 },
}

impl PortPattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn single(port: u16) -> Self {
        Self::Single(port)
    }

    pub fn range(start: u16, end: u16) -> Self {
        Self::Range { start, end }
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Single(a), Self::Single(b)) => a == b,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Single(b),
            ) => as_ <= b && b <= ae,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Range { start: bs, end: be },
            ) => as_ <= bs && be <= ae,
            (Self::Single(_), Self::Any | Self::Range { .. }) => false,
            (Self::Range { .. }, Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum NetworkResourcePattern {
    Any,
    HostPort { host: String, ports: PortPattern },
}

impl NetworkResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn host(host: impl Into<String>) -> Self {
        Self::HostPort {
            host: host.into(),
            ports: PortPattern::Any,
        }
    }

    pub fn host_port(host: impl Into<String>, ports: PortPattern) -> Self {
        Self::HostPort {
            host: host.into(),
            ports,
        }
    }
}

impl ResourcePattern for NetworkResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        let (host, ports) = if let Some((host, port)) = resource.rsplit_once(':') {
            validate_host_pattern(host, resource)?;
            (host.to_string(), parse_port_pattern(port, resource)?)
        } else {
            validate_host_pattern(resource, resource)?;
            (resource.to_string(), PortPattern::Any)
        };

        if host == "*" && ports == PortPattern::Any {
            Ok(NetworkResourcePattern::Any)
        } else {
            Ok(NetworkResourcePattern::HostPort { host, ports })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::HostPort {
                    host: ah,
                    ports: ap,
                },
                Self::HostPort {
                    host: bh,
                    ports: bp,
                },
            ) => host_pattern_subsumes(ah, bh) && ap.subsumes(bp),
            (Self::HostPort { .. }, Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum NetworkVerb {
    Connect,
}
impl VerbPattern for NetworkVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "connect" => Some(Self::Connect),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct NetworkClass;

impl PermissionClass for NetworkClass {
    type Verb = NetworkVerb;
    type Owner = EmptyOwnerPattern;
    type Resource = NetworkResourcePattern;
    const NAME: &'static str = "network";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Network(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Network(pattern)
    }
}

fn validate_host_pattern(host: &str, resource: &str) -> Result<(), CardParseError> {
    if host == "*" {
        return Ok(());
    }

    if host.is_empty() {
        return Err(invalid_network_resource(resource));
    }

    for segment in host.split('.') {
        if segment.is_empty()
            || (segment != "*"
                && segment
                    .chars()
                    .any(|c| c == '*' || c == ':' || c.is_whitespace()))
        {
            return Err(invalid_network_resource(resource));
        }
    }

    Ok(())
}

fn host_pattern_subsumes(left: &str, right: &str) -> bool {
    if left == "*" {
        return true;
    }

    let left_segments: Vec<_> = left.split('.').collect();
    let right_segments: Vec<_> = right.split('.').collect();
    left_segments.len() == right_segments.len()
        && left_segments
            .iter()
            .zip(right_segments.iter())
            .all(|(left, right)| *left == "*" || left == right)
}

fn parse_port_pattern(port: &str, resource: &str) -> Result<PortPattern, CardParseError> {
    if let Some((start, end)) = port.split_once('-') {
        let start = parse_port_number(start, resource)?;
        let end = parse_port_number(end, resource)?;
        if start > end {
            return Err(invalid_network_resource(resource));
        }
        Ok(PortPattern::Range { start, end })
    } else {
        Ok(PortPattern::Single(parse_port_number(port, resource)?))
    }
}

fn parse_port_number(port: &str, resource: &str) -> Result<u16, CardParseError> {
    if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid_network_resource(resource));
    }

    port.parse().map_err(|_| invalid_network_resource(resource))
}

fn invalid_network_resource(resource: &str) -> CardParseError {
    CardParseError::InvalidResource {
        class: NetworkClass::NAME.to_string(),
        resource: resource.to_string(),
    }
}
