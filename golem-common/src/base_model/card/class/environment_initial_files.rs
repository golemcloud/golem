use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_component_owner, parse_environment_recipient,
    parse_polymorphic_component_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentInitialFilesResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl EnvironmentInitialFilesResourcePattern {
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

impl Subsumes for EnvironmentInitialFilesResourcePattern {
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
pub enum PolymorphicEnvironmentInitialFilesResourcePattern {
    Concrete(EnvironmentInitialFilesResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentInitialFilesResourcePattern {
    type Polymorphic = PolymorphicEnvironmentInitialFilesResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentInitialFilesVerb {
    View,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentInitialFilesClass;

impl PermissionClass for EnvironmentInitialFilesClass {
    type Verb = EnvironmentInitialFilesVerb;
    type Owner = ComponentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentInitialFilesResourcePattern;
    const NAME: &'static str = "environment.initial-files";
}

pub type EnvironmentInitialFilesPermissionPattern =
    ClassPermissionPattern<EnvironmentInitialFilesClass>;
pub type PolymorphicEnvironmentInitialFilesPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentInitialFilesClass>;

impl EnvironmentInitialFilesClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_component_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentInitialFiles(match verb {
            "*" => EnvironmentInitialFilesPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentInitialFilesPermissionPattern::Verb {
                verb: EnvironmentInitialFilesVerb::View,
                owner,
                recipient,
                resource,
            },
            "update" => EnvironmentInitialFilesPermissionPattern::Verb {
                verb: EnvironmentInitialFilesVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentInitialFilesPermissionPattern::Verb {
                verb: EnvironmentInitialFilesVerb::Delete,
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
        let owner = parse_polymorphic_component_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_environment_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::EnvironmentInitialFiles(
            match verb {
                "*" => PolymorphicEnvironmentInitialFilesPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentInitialFilesPermissionPattern::Verb {
                    verb: EnvironmentInitialFilesVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "update" => PolymorphicEnvironmentInitialFilesPermissionPattern::Verb {
                    verb: EnvironmentInitialFilesVerb::Update,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentInitialFilesPermissionPattern::Verb {
                    verb: EnvironmentInitialFilesVerb::Delete,
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
    ) -> Result<EnvironmentInitialFilesResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(EnvironmentInitialFilesResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(EnvironmentInitialFilesResourcePattern::Glob(
                resource.to_string(),
            ))
        } else {
            Ok(EnvironmentInitialFilesResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentInitialFilesResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentInitialFilesResourcePattern::Concrete,
            PolymorphicEnvironmentInitialFilesResourcePattern::Slot,
            PolymorphicEnvironmentInitialFilesResourcePattern::Template,
        )
    }
}
