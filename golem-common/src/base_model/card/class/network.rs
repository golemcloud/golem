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
use crate::base_model::address_class::{AddressClass, embedded_ipv4};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EmptyOwnerPattern;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv6Addr};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum NetworkResourcePattern {
    Any,
    /// A host name or IP literal with ports. An IPv6 host is stored without brackets, in the
    /// canonical text form of [`canonical_ip_host`].
    HostPort {
        host: String,
        ports: PortPattern,
    },
    /// The destination of a raw socket connect or outgoing datagram, with the hostnames that the
    /// agent's recorded address lookups linked to the address. It is a request, never a grant:
    /// card text never parses to it. A grant matches it when it matches the address or one of the
    /// hostnames, and a grant whose host has a wildcard matches it only when the address is
    /// [`AddressClass::Public`] and is not an address of this host (`this_host`). Every other
    /// address needs a grant that names it: the address itself, or an exact linked hostname.
    SocketAddress {
        host: String,
        port: u16,
        this_host: bool,
        hostnames: Vec<String>,
    },
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

    /// A socket destination; `this_host` tells whether `ip` is an address of the executor host,
    /// and `hostnames` are the normalized hostnames linked to `ip`.
    pub fn socket_address(ip: IpAddr, port: u16, this_host: bool, hostnames: Vec<String>) -> Self {
        Self::SocketAddress {
            host: canonical_ip_host(ip),
            port,
            this_host,
            hostnames,
        }
    }

    /// Whether this pattern, used as a denial, covers `other`. A denial with a wildcard host also
    /// covers socket destinations that are not public, a denial of an IPv4 host also covers the
    /// IPv6 forms that embed it (NAT64, 6to4, IPv4-compatible), and a denial that covers a linked
    /// hostname covers the address.
    pub fn denies(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::HostPort { host, ports },
                Self::SocketAddress {
                    host: target,
                    port,
                    hostnames,
                    ..
                },
            ) => {
                ports.subsumes(&PortPattern::Single(*port))
                    && (host_pattern_subsumes(host, target)
                        || embedded_ipv4_host(target)
                            .is_some_and(|ipv4| host_pattern_subsumes(host, &ipv4))
                        || hostnames
                            .iter()
                            .any(|name| host_pattern_subsumes(host, name)))
            }
            _ => self.subsumes(other),
        }
    }
}

/// The canonical host text of an IP address: dotted IPv4, or RFC 5952 IPv6 (lowercase and
/// compressed) without brackets. An IPv4-mapped IPv6 address is its IPv4 address.
pub fn canonical_ip_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => v6.to_string(),
        },
    }
}

fn wildcard_host_reaches(host: &str, this_host: bool) -> bool {
    match host.parse::<IpAddr>() {
        Ok(ip) => !this_host && AddressClass::of(ip) == AddressClass::Public,
        Err(_) => true,
    }
}

fn embedded_ipv4_host(host: &str) -> Option<String> {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(ip)) => embedded_ipv4(&ip).map(|ipv4| ipv4.to_string()),
        _ => None,
    }
}

impl ResourcePattern for NetworkResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        let (host, ports) = if let Some(bracketed) = resource.trim().strip_prefix('[') {
            let (address, rest) = bracketed
                .split_once(']')
                .ok_or_else(|| invalid_network_resource(resource))?;
            let ip = address
                .parse::<Ipv6Addr>()
                .map_err(|_| invalid_network_resource(resource))?;
            let ports = match rest.strip_prefix(':') {
                Some(port) => parse_port_pattern(port, resource)?,
                None if rest.is_empty() => PortPattern::Any,
                None => return Err(invalid_network_resource(resource)),
            };
            (canonical_ip_host(IpAddr::V6(ip)), ports)
        } else if let Some((host, port)) = resource.rsplit_once(':') {
            (
                normalize_host_pattern(host, resource)?,
                parse_port_pattern(port, resource)?,
            )
        } else {
            (
                normalize_host_pattern(resource, resource)?,
                PortPattern::Any,
            )
        };

        if host == "*" && ports == PortPattern::Any {
            Ok(NetworkResourcePattern::Any)
        } else {
            Ok(NetworkResourcePattern::HostPort { host, ports })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
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
            (
                Self::Any,
                Self::SocketAddress {
                    host, this_host, ..
                },
            ) => wildcard_host_reaches(host, *this_host),
            (Self::Any, _) => true,
            (
                Self::HostPort { host, ports },
                Self::SocketAddress {
                    host: target,
                    port,
                    this_host,
                    hostnames,
                },
            ) => {
                (!host.contains('*') || wildcard_host_reaches(target, *this_host))
                    && ports.subsumes(&PortPattern::Single(*port))
                    && std::iter::once(target)
                        .chain(hostnames)
                        .any(|name| host_pattern_subsumes(host, name))
            }
            (Self::HostPort { .. }, Self::Any) => false,
            (Self::SocketAddress { .. }, _) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

fn normalize_host_pattern(host: &str, resource: &str) -> Result<String, CardParseError> {
    let host = host.trim();
    if host == "*" {
        return Ok(host.to_string());
    }

    let host = if let Some(host) = host.strip_suffix('.') {
        if host.is_empty() || host.ends_with('.') || host == "*" {
            return Err(invalid_network_resource(resource));
        }
        host
    } else {
        host
    };

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

    if !host.contains('*') {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return match ip {
                IpAddr::V4(ip) => Ok(ip.to_string()),
                IpAddr::V6(_) => Err(invalid_network_resource(resource)),
            };
        }
        if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(invalid_network_resource(resource));
        }
    }

    Ok(host.to_ascii_lowercase())
}

fn host_pattern_subsumes(left: &str, right: &str) -> bool {
    if left == "*" {
        return true;
    }

    let mut left_segments = left.split('.');
    let mut right_segments = right.split('.');
    loop {
        match (left_segments.next(), right_segments.next()) {
            (Some(left), Some(right)) if left == "*" || left.eq_ignore_ascii_case(right) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
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
