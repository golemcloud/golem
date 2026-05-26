use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_environment_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_environment_owner,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum KvResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl KvResourcePattern {
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

impl Subsumes for KvResourcePattern {
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
pub enum PolymorphicKvResourcePattern {
    Concrete(KvResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for KvResourcePattern {
    type Polymorphic = PolymorphicKvResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum KvVerb {
    Read,
    Write,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct KvClass;

impl PermissionClass for KvClass {
    type Verb = KvVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = KvResourcePattern;
    const NAME: &'static str = "kv";
}

pub type KvPermissionPattern = ClassPermissionPattern<KvClass>;
pub type PolymorphicKvPermissionPattern = PolymorphicClassPermissionPattern<KvClass>;

impl KvClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Kv(match verb {
            "*" => KvPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => KvPermissionPattern::Verb {
                verb: KvVerb::Read,
                owner,
                recipient,
                resource,
            },
            "write" => KvPermissionPattern::Verb {
                verb: KvVerb::Write,
                owner,
                recipient,
                resource,
            },
            "delete" => KvPermissionPattern::Verb {
                verb: KvVerb::Delete,
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
        let owner = parse_polymorphic_environment_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Kv(match verb {
            "*" => PolymorphicKvPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => PolymorphicKvPermissionPattern::Verb {
                verb: KvVerb::Read,
                owner,
                recipient,
                resource,
            },
            "write" => PolymorphicKvPermissionPattern::Verb {
                verb: KvVerb::Write,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicKvPermissionPattern::Verb {
                verb: KvVerb::Delete,
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

    fn parse_resource(_class: &str, resource: &str) -> Result<KvResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(KvResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(KvResourcePattern::Glob(resource.to_string()))
        } else {
            Ok(KvResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicKvResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicKvResourcePattern::Concrete,
            PolymorphicKvResourcePattern::Slot,
            PolymorphicKvResourcePattern::Template,
        )
    }
}
