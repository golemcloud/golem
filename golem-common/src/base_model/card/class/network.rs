use super::*;
use crate::base_model::card::parsing::CardParseError;

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

impl Subsumes for NetworkResourcePattern {
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
            ) => glob_subsumes(ah, bh) && ap.subsumes(bp),
            (Self::HostPort { .. }, Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum NetworkVerb {
    Connect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct NetworkClass;

impl PermissionClass for NetworkClass {
    type Verb = NetworkVerb;
    type Owner = EmptyOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = NetworkResourcePattern;
    const NAME: &'static str = "network";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "connect" => Some(Self::Verb::Connect),
            _ => None,
        }
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Network(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Network(pattern)
    }
}

pub type NetworkPermissionPattern = ClassPermissionPattern<NetworkClass>;
pub type PolymorphicNetworkPermissionPattern = PolymorphicClassPermissionPattern<NetworkClass>;

impl NetworkClass {
    fn parse_resource(
        class: &str,
        resource: &str,
    ) -> Result<NetworkResourcePattern, CardParseError> {
        if resource == "*" {
            return Ok(NetworkResourcePattern::Any);
        }

        let (host, ports) = if let Some((host, port)) = resource.rsplit_once(':') {
            if port.chars().all(|c| c.is_ascii_digit() || c == '-') {
                (host.to_string(), Self::parse_port_pattern(class, port)?)
            } else {
                (resource.to_string(), PortPattern::Any)
            }
        } else {
            (resource.to_string(), PortPattern::Any)
        };

        Ok(NetworkResourcePattern::HostPort { host, ports })
    }

    fn parse_port_pattern(class: &str, port: &str) -> Result<PortPattern, CardParseError> {
        if let Some((start, end)) = port.split_once('-') {
            let start = start.parse().map_err(|_| CardParseError::InvalidResource {
                class: class.to_string(),
                resource: port.to_string(),
            })?;
            let end = end.parse().map_err(|_| CardParseError::InvalidResource {
                class: class.to_string(),
                resource: port.to_string(),
            })?;
            Ok(PortPattern::Range { start, end })
        } else {
            Ok(PortPattern::Single(port.parse().map_err(|_| {
                CardParseError::InvalidResource {
                    class: class.to_string(),
                    resource: port.to_string(),
                }
            })?))
        }
    }
}
