use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_owner, parse_agent_recipient, parse_polymorphic_agent_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ConfigResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl ConfigResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::Glob(value.into())
    }
}

impl Subsumes for ConfigResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Glob(a), Self::Glob(b)) => glob_subsumes(a, b),
            (Self::Glob(a), Self::Exact(b)) => glob_matches(a, b),
            (Self::Glob(_), Self::Any) => false,
            (Self::Exact(_), Self::Any | Self::Glob(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicConfigResourcePattern {
    Concrete(ConfigResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for ConfigResourcePattern {
    type Polymorphic = PolymorphicConfigResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ConfigVerb {
    Read,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ConfigClass;

impl PermissionClass for ConfigClass {
    type Verb = ConfigVerb;
    type Owner = AgentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = ConfigResourcePattern;
    const NAME: &'static str = "config";
}

pub type ConfigPermissionPattern = ClassPermissionPattern<ConfigClass>;
pub type PolymorphicConfigPermissionPattern = PolymorphicClassPermissionPattern<ConfigClass>;

impl ConfigClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_agent_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Config(match verb {
            "*" => ConfigPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => ConfigPermissionPattern::Verb {
                verb: ConfigVerb::Read,
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
        let owner = parse_polymorphic_agent_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Config(match verb {
            "*" => PolymorphicConfigPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => PolymorphicConfigPermissionPattern::Verb {
                verb: ConfigVerb::Read,
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
        _class: &str,
        resource: &str,
    ) -> Result<ConfigResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(ConfigResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(ConfigResourcePattern::Glob(resource.to_string()))
        } else {
            Ok(ConfigResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicConfigResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicConfigResourcePattern::Concrete,
            PolymorphicConfigResourcePattern::Slot,
            PolymorphicConfigResourcePattern::Template,
        )
    }
}
