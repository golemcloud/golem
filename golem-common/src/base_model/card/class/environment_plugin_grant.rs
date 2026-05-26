use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentPluginGrantResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentPluginGrantResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentPluginGrantResourcePattern {
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
pub enum PolymorphicEnvironmentPluginGrantResourcePattern {
    Concrete(EnvironmentPluginGrantResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentPluginGrantResourcePattern {
    type Polymorphic = PolymorphicEnvironmentPluginGrantResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentPluginGrantVerb {
    View,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentPluginGrantClass;

impl PermissionClass for EnvironmentPluginGrantClass {
    type Verb = EnvironmentPluginGrantVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentPluginGrantResourcePattern;
    const NAME: &'static str = "environment.plugin-grant";
}

pub type EnvironmentPluginGrantPermissionPattern =
    ClassPermissionPattern<EnvironmentPluginGrantClass>;
pub type PolymorphicEnvironmentPluginGrantPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentPluginGrantClass>;

impl EnvironmentPluginGrantClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentPluginGrant(match verb {
            "*" => EnvironmentPluginGrantPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentPluginGrantPermissionPattern::Verb {
                verb: EnvironmentPluginGrantVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentPluginGrantPermissionPattern::Verb {
                verb: EnvironmentPluginGrantVerb::Create,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentPluginGrantPermissionPattern::Verb {
                verb: EnvironmentPluginGrantVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentPluginGrant(
            match verb {
                "*" => PolymorphicEnvironmentPluginGrantPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentPluginGrantPermissionPattern::Verb {
                    verb: EnvironmentPluginGrantVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentPluginGrantPermissionPattern::Verb {
                    verb: EnvironmentPluginGrantVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentPluginGrantPermissionPattern::Verb {
                    verb: EnvironmentPluginGrantVerb::Delete,
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
    ) -> Result<EnvironmentPluginGrantResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentPluginGrantResourcePattern::Any)
        } else {
            Ok(EnvironmentPluginGrantResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentPluginGrantResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentPluginGrantResourcePattern::Concrete,
            PolymorphicEnvironmentPluginGrantResourcePattern::Slot,
            PolymorphicEnvironmentPluginGrantResourcePattern::Template,
        )
    }
}
