use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_empty_owner, parse_polymorphic_agent_recipient,
    parse_polymorphic_empty_owner, parse_polymorphic_resource,
};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicNetworkResourcePattern {
    Concrete(NetworkResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for NetworkResourcePattern {
    type Polymorphic = PolymorphicNetworkResourcePattern;
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
}

pub type NetworkPermissionPattern = ClassPermissionPattern<NetworkClass>;
pub type PolymorphicNetworkPermissionPattern = PolymorphicClassPermissionPattern<NetworkClass>;

impl NetworkClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_empty_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Network(match verb {
            "*" => NetworkPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "connect" => NetworkPermissionPattern::Verb {
                verb: NetworkVerb::Connect,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

    pub(crate) fn parse_polymorphic_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PolymorphicPermissionPattern, CardParseError> {
        let owner = parse_polymorphic_empty_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Network(match verb {
            "*" => PolymorphicNetworkPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "connect" => PolymorphicNetworkPermissionPattern::Verb {
                verb: NetworkVerb::Connect,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

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

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicNetworkResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicNetworkResourcePattern::Concrete,
            PolymorphicNetworkResourcePattern::Slot,
            PolymorphicNetworkResourcePattern::Template,
        )
    }
}
