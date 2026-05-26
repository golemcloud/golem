use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_owner, parse_agent_recipient, parse_polymorphic_agent_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvResourcePattern {
    Any,
    Exact(String),
}

impl EnvResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvResourcePattern {
    Concrete(EnvResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvResourcePattern {
    type Polymorphic = PolymorphicEnvResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvVerb {
    Read,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvClass;

impl PermissionClass for EnvClass {
    type Verb = EnvVerb;
    type Owner = AgentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = EnvResourcePattern;
    const NAME: &'static str = "env";
}

pub type EnvPermissionPattern = ClassPermissionPattern<EnvClass>;
pub type PolymorphicEnvPermissionPattern = PolymorphicClassPermissionPattern<EnvClass>;

impl EnvClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_agent_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Env(match verb {
            "*" => EnvPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => EnvPermissionPattern::Verb {
                verb: EnvVerb::Read,
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
        Ok(PolymorphicPermissionPattern::Env(match verb {
            "*" => PolymorphicEnvPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => PolymorphicEnvPermissionPattern::Verb {
                verb: EnvVerb::Read,
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

    fn parse_resource(_class: &str, resource: &str) -> Result<EnvResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvResourcePattern::Any)
        } else {
            Ok(EnvResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvResourcePattern::Concrete,
            PolymorphicEnvResourcePattern::Slot,
            PolymorphicEnvResourcePattern::Template,
        )
    }
}
