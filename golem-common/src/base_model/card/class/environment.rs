use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentResourcePattern;

impl Subsumes for EnvironmentResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentResourcePattern {
    Concrete(EnvironmentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentResourcePattern {
    type Polymorphic = PolymorphicEnvironmentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
    Deploy,
    Rollback,
    ViewDeploymentPlan,
    WriteDeploymentRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentClass;

impl PermissionClass for EnvironmentClass {
    type Verb = EnvironmentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentResourcePattern;
    const NAME: &'static str = "environment";
}

pub type EnvironmentPermissionPattern = ClassPermissionPattern<EnvironmentClass>;
pub type PolymorphicEnvironmentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentClass>;

impl EnvironmentClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Environment(match verb {
            "*" => EnvironmentPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "restore" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Restore,
                owner,
                recipient,
                resource,
            },
            "deploy" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Deploy,
                owner,
                recipient,
                resource,
            },
            "rollback" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Rollback,
                owner,
                recipient,
                resource,
            },
            "view-deployment-plan" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::ViewDeploymentPlan,
                owner,
                recipient,
                resource,
            },
            "write-deployment-record" => EnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::WriteDeploymentRecord,
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
        Ok(PolymorphicPermissionPattern::Environment(match verb {
            "*" => PolymorphicEnvironmentPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "restore" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Restore,
                owner,
                recipient,
                resource,
            },
            "deploy" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Deploy,
                owner,
                recipient,
                resource,
            },
            "rollback" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::Rollback,
                owner,
                recipient,
                resource,
            },
            "view-deployment-plan" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::ViewDeploymentPlan,
                owner,
                recipient,
                resource,
            },
            "write-deployment-record" => PolymorphicEnvironmentPermissionPattern::Verb {
                verb: EnvironmentVerb::WriteDeploymentRecord,
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
        class: &str,
        resource: &str,
    ) -> Result<EnvironmentResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(EnvironmentResourcePattern)
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentResourcePattern, CardParseError> {
        if let Ok(resource) = Self::parse_resource(class, resource) {
            Ok(PolymorphicEnvironmentResourcePattern::Concrete(resource))
        } else if let Ok(slot) = SlotVariable::parse(resource) {
            Ok(PolymorphicEnvironmentResourcePattern::Slot(slot))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}
