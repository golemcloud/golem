use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourceDefinitionResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentResourceDefinitionResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentResourceDefinitionResourcePattern {
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
pub enum PolymorphicEnvironmentResourceDefinitionResourcePattern {
    Concrete(EnvironmentResourceDefinitionResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentResourceDefinitionResourcePattern {
    type Polymorphic = PolymorphicEnvironmentResourceDefinitionResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourceDefinitionVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentResourceDefinitionClass;

impl PermissionClass for EnvironmentResourceDefinitionClass {
    type Verb = EnvironmentResourceDefinitionVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentResourceDefinitionResourcePattern;
    const NAME: &'static str = "environment.resource-definition";
}

pub type EnvironmentResourceDefinitionPermissionPattern =
    ClassPermissionPattern<EnvironmentResourceDefinitionClass>;
pub type PolymorphicEnvironmentResourceDefinitionPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentResourceDefinitionClass>;

impl EnvironmentResourceDefinitionClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentResourceDefinition(
            match verb {
                "*" => EnvironmentResourceDefinitionPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => EnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => EnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "update" => EnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::Update,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => EnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::Delete,
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

    pub(crate) fn parse_polymorphic_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PolymorphicPermissionPattern, CardParseError> {
        let owner = parse_polymorphic_environment_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_environment_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::EnvironmentResourceDefinition(
            match verb {
                "*" => PolymorphicEnvironmentResourceDefinitionPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "update" => PolymorphicEnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::Update,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentResourceDefinitionPermissionPattern::Verb {
                    verb: EnvironmentResourceDefinitionVerb::Delete,
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
    ) -> Result<EnvironmentResourceDefinitionResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentResourceDefinitionResourcePattern::Any)
        } else {
            Ok(EnvironmentResourceDefinitionResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentResourceDefinitionResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentResourceDefinitionResourcePattern::Concrete,
            PolymorphicEnvironmentResourceDefinitionResourcePattern::Slot,
            PolymorphicEnvironmentResourceDefinitionResourcePattern::Template,
        )
    }
}
