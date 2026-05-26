use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentMcpDeploymentResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentMcpDeploymentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentMcpDeploymentResourcePattern {
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
pub enum PolymorphicEnvironmentMcpDeploymentResourcePattern {
    Concrete(EnvironmentMcpDeploymentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentMcpDeploymentResourcePattern {
    type Polymorphic = PolymorphicEnvironmentMcpDeploymentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentMcpDeploymentVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentMcpDeploymentClass;

impl PermissionClass for EnvironmentMcpDeploymentClass {
    type Verb = EnvironmentMcpDeploymentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentMcpDeploymentResourcePattern;
    const NAME: &'static str = "environment.mcp-deployment";
}

pub type EnvironmentMcpDeploymentPermissionPattern =
    ClassPermissionPattern<EnvironmentMcpDeploymentClass>;
pub type PolymorphicEnvironmentMcpDeploymentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentMcpDeploymentClass>;

impl EnvironmentMcpDeploymentClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentMcpDeployment(match verb {
            "*" => EnvironmentMcpDeploymentPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentMcpDeploymentPermissionPattern::Verb {
                verb: EnvironmentMcpDeploymentVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentMcpDeploymentPermissionPattern::Verb {
                verb: EnvironmentMcpDeploymentVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => EnvironmentMcpDeploymentPermissionPattern::Verb {
                verb: EnvironmentMcpDeploymentVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentMcpDeploymentPermissionPattern::Verb {
                verb: EnvironmentMcpDeploymentVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentMcpDeployment(
            match verb {
                "*" => PolymorphicEnvironmentMcpDeploymentPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentMcpDeploymentPermissionPattern::Verb {
                    verb: EnvironmentMcpDeploymentVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentMcpDeploymentPermissionPattern::Verb {
                    verb: EnvironmentMcpDeploymentVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "update" => PolymorphicEnvironmentMcpDeploymentPermissionPattern::Verb {
                    verb: EnvironmentMcpDeploymentVerb::Update,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentMcpDeploymentPermissionPattern::Verb {
                    verb: EnvironmentMcpDeploymentVerb::Delete,
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
    ) -> Result<EnvironmentMcpDeploymentResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentMcpDeploymentResourcePattern::Any)
        } else {
            Ok(EnvironmentMcpDeploymentResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentMcpDeploymentResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentMcpDeploymentResourcePattern::Concrete,
            PolymorphicEnvironmentMcpDeploymentResourcePattern::Slot,
            PolymorphicEnvironmentMcpDeploymentResourcePattern::Template,
        )
    }
}
