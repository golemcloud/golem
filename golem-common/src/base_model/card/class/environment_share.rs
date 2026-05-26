use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentShareResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentShareResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentShareResourcePattern {
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
pub enum PolymorphicEnvironmentShareResourcePattern {
    Concrete(EnvironmentShareResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentShareResourcePattern {
    type Polymorphic = PolymorphicEnvironmentShareResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentShareVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentShareClass;

impl PermissionClass for EnvironmentShareClass {
    type Verb = EnvironmentShareVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentShareResourcePattern;
    const NAME: &'static str = "environment.share";
}

pub type EnvironmentSharePermissionPattern = ClassPermissionPattern<EnvironmentShareClass>;
pub type PolymorphicEnvironmentSharePermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentShareClass>;

impl EnvironmentShareClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentShare(match verb {
            "*" => EnvironmentSharePermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => EnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentShare(match verb {
            "*" => PolymorphicEnvironmentSharePermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicEnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicEnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => PolymorphicEnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicEnvironmentSharePermissionPattern::Verb {
                verb: EnvironmentShareVerb::Delete,
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
    ) -> Result<EnvironmentShareResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentShareResourcePattern::Any)
        } else {
            Ok(EnvironmentShareResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentShareResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentShareResourcePattern::Concrete,
            PolymorphicEnvironmentShareResourcePattern::Slot,
            PolymorphicEnvironmentShareResourcePattern::Template,
        )
    }
}
