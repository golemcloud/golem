use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRetryPolicyResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentRetryPolicyResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentRetryPolicyResourcePattern {
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
pub enum PolymorphicEnvironmentRetryPolicyResourcePattern {
    Concrete(EnvironmentRetryPolicyResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentRetryPolicyResourcePattern {
    type Polymorphic = PolymorphicEnvironmentRetryPolicyResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRetryPolicyVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentRetryPolicyClass;

impl PermissionClass for EnvironmentRetryPolicyClass {
    type Verb = EnvironmentRetryPolicyVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentRetryPolicyResourcePattern;
    const NAME: &'static str = "environment.retry-policy";
}

pub type EnvironmentRetryPolicyPermissionPattern =
    ClassPermissionPattern<EnvironmentRetryPolicyClass>;
pub type PolymorphicEnvironmentRetryPolicyPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentRetryPolicyClass>;

impl EnvironmentRetryPolicyClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentRetryPolicy(match verb {
            "*" => EnvironmentRetryPolicyPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentRetryPolicyPermissionPattern::Verb {
                verb: EnvironmentRetryPolicyVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentRetryPolicyPermissionPattern::Verb {
                verb: EnvironmentRetryPolicyVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => EnvironmentRetryPolicyPermissionPattern::Verb {
                verb: EnvironmentRetryPolicyVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentRetryPolicyPermissionPattern::Verb {
                verb: EnvironmentRetryPolicyVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentRetryPolicy(
            match verb {
                "*" => PolymorphicEnvironmentRetryPolicyPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentRetryPolicyPermissionPattern::Verb {
                    verb: EnvironmentRetryPolicyVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentRetryPolicyPermissionPattern::Verb {
                    verb: EnvironmentRetryPolicyVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "update" => PolymorphicEnvironmentRetryPolicyPermissionPattern::Verb {
                    verb: EnvironmentRetryPolicyVerb::Update,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentRetryPolicyPermissionPattern::Verb {
                    verb: EnvironmentRetryPolicyVerb::Delete,
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
    ) -> Result<EnvironmentRetryPolicyResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentRetryPolicyResourcePattern::Any)
        } else {
            Ok(EnvironmentRetryPolicyResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentRetryPolicyResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentRetryPolicyResourcePattern::Concrete,
            PolymorphicEnvironmentRetryPolicyResourcePattern::Slot,
            PolymorphicEnvironmentRetryPolicyResourcePattern::Template,
        )
    }
}
