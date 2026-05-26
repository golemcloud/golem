use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl EnvironmentAgentSecretResourcePattern {
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

impl Subsumes for EnvironmentAgentSecretResourcePattern {
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
pub enum PolymorphicEnvironmentAgentSecretResourcePattern {
    Concrete(EnvironmentAgentSecretResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentAgentSecretResourcePattern {
    type Polymorphic = PolymorphicEnvironmentAgentSecretResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentAgentSecretVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentAgentSecretClass;

impl PermissionClass for EnvironmentAgentSecretClass {
    type Verb = EnvironmentAgentSecretVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentAgentSecretResourcePattern;
    const NAME: &'static str = "environment.agent-secret";
}

pub type EnvironmentAgentSecretPermissionPattern =
    ClassPermissionPattern<EnvironmentAgentSecretClass>;
pub type PolymorphicEnvironmentAgentSecretPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentAgentSecretClass>;

impl EnvironmentAgentSecretClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentAgentSecret(match verb {
            "*" => EnvironmentAgentSecretPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentAgentSecretPermissionPattern::Verb {
                verb: EnvironmentAgentSecretVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentAgentSecretPermissionPattern::Verb {
                verb: EnvironmentAgentSecretVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => EnvironmentAgentSecretPermissionPattern::Verb {
                verb: EnvironmentAgentSecretVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentAgentSecretPermissionPattern::Verb {
                verb: EnvironmentAgentSecretVerb::Delete,
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
        let recipient = parse_polymorphic_environment_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::EnvironmentAgentSecret(
            match verb {
                "*" => PolymorphicEnvironmentAgentSecretPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentAgentSecretPermissionPattern::Verb {
                    verb: EnvironmentAgentSecretVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentAgentSecretPermissionPattern::Verb {
                    verb: EnvironmentAgentSecretVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "update" => PolymorphicEnvironmentAgentSecretPermissionPattern::Verb {
                    verb: EnvironmentAgentSecretVerb::Update,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentAgentSecretPermissionPattern::Verb {
                    verb: EnvironmentAgentSecretVerb::Delete,
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
            },
        ))
    }

    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentAgentSecretResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(EnvironmentAgentSecretResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(EnvironmentAgentSecretResourcePattern::Glob(
                resource.to_string(),
            ))
        } else {
            Ok(EnvironmentAgentSecretResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentAgentSecretResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentAgentSecretResourcePattern::Concrete,
            PolymorphicEnvironmentAgentSecretResourcePattern::Slot,
            PolymorphicEnvironmentAgentSecretResourcePattern::Template,
        )
    }
}
