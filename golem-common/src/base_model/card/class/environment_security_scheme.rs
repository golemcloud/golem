use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentSecuritySchemeResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentSecuritySchemeResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentSecuritySchemeResourcePattern {
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
pub enum PolymorphicEnvironmentSecuritySchemeResourcePattern {
    Concrete(EnvironmentSecuritySchemeResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentSecuritySchemeResourcePattern {
    type Polymorphic = PolymorphicEnvironmentSecuritySchemeResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentSecuritySchemeVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentSecuritySchemeClass;

impl PermissionClass for EnvironmentSecuritySchemeClass {
    type Verb = EnvironmentSecuritySchemeVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentSecuritySchemeResourcePattern;
    const NAME: &'static str = "environment.security-scheme";
}

pub type EnvironmentSecuritySchemePermissionPattern =
    ClassPermissionPattern<EnvironmentSecuritySchemeClass>;
pub type PolymorphicEnvironmentSecuritySchemePermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentSecuritySchemeClass>;

impl EnvironmentSecuritySchemeClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentSecurityScheme(match verb {
            "*" => EnvironmentSecuritySchemePermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentSecuritySchemePermissionPattern::Verb {
                verb: EnvironmentSecuritySchemeVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentSecuritySchemePermissionPattern::Verb {
                verb: EnvironmentSecuritySchemeVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => EnvironmentSecuritySchemePermissionPattern::Verb {
                verb: EnvironmentSecuritySchemeVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentSecuritySchemePermissionPattern::Verb {
                verb: EnvironmentSecuritySchemeVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentSecurityScheme(
            match verb {
                "*" => PolymorphicEnvironmentSecuritySchemePermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentSecuritySchemePermissionPattern::Verb {
                    verb: EnvironmentSecuritySchemeVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentSecuritySchemePermissionPattern::Verb {
                    verb: EnvironmentSecuritySchemeVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "update" => PolymorphicEnvironmentSecuritySchemePermissionPattern::Verb {
                    verb: EnvironmentSecuritySchemeVerb::Update,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentSecuritySchemePermissionPattern::Verb {
                    verb: EnvironmentSecuritySchemeVerb::Delete,
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
    ) -> Result<EnvironmentSecuritySchemeResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentSecuritySchemeResourcePattern::Any)
        } else {
            Ok(EnvironmentSecuritySchemeResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentSecuritySchemeResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentSecuritySchemeResourcePattern::Concrete,
            PolymorphicEnvironmentSecuritySchemeResourcePattern::Slot,
            PolymorphicEnvironmentSecuritySchemeResourcePattern::Template,
        )
    }
}
