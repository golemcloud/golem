use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_environment_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_environment_owner,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum SecretResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl SecretResourcePattern {
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

impl Subsumes for SecretResourcePattern {
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
pub enum PolymorphicSecretResourcePattern {
    Concrete(SecretResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for SecretResourcePattern {
    type Polymorphic = PolymorphicSecretResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum SecretVerb {
    Hold,
    Mint,
    Reveal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SecretClass;

impl PermissionClass for SecretClass {
    type Verb = SecretVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = SecretResourcePattern;
    const NAME: &'static str = "secret";
}

pub type SecretPermissionPattern = ClassPermissionPattern<SecretClass>;
pub type PolymorphicSecretPermissionPattern = PolymorphicClassPermissionPattern<SecretClass>;

impl SecretClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Secret(match verb {
            "*" => SecretPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "hold" => SecretPermissionPattern::Verb {
                verb: SecretVerb::Hold,
                owner,
                recipient,
                resource,
            },
            "mint" => SecretPermissionPattern::Verb {
                verb: SecretVerb::Mint,
                owner,
                recipient,
                resource,
            },
            "reveal" => SecretPermissionPattern::Verb {
                verb: SecretVerb::Reveal,
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
        Ok(PolymorphicPermissionPattern::Secret(match verb {
            "*" => PolymorphicSecretPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "hold" => PolymorphicSecretPermissionPattern::Verb {
                verb: SecretVerb::Hold,
                owner,
                recipient,
                resource,
            },
            "mint" => PolymorphicSecretPermissionPattern::Verb {
                verb: SecretVerb::Mint,
                owner,
                recipient,
                resource,
            },
            "reveal" => PolymorphicSecretPermissionPattern::Verb {
                verb: SecretVerb::Reveal,
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
    ) -> Result<SecretResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(SecretResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(SecretResourcePattern::Glob(resource.to_string()))
        } else {
            Ok(SecretResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicSecretResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicSecretResourcePattern::Concrete,
            PolymorphicSecretResourcePattern::Slot,
            PolymorphicSecretResourcePattern::Template,
        )
    }
}
