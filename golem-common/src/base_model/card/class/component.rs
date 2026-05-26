use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_component_owner, parse_environment_recipient,
    parse_polymorphic_component_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ComponentResourcePattern;

impl Subsumes for ComponentResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicComponentResourcePattern {
    Concrete(ComponentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for ComponentResourcePattern {
    type Polymorphic = PolymorphicComponentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ComponentVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ComponentClass;

impl PermissionClass for ComponentClass {
    type Verb = ComponentVerb;
    type Owner = ComponentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = ComponentResourcePattern;
    const NAME: &'static str = "component";
}

pub type ComponentPermissionPattern = ClassPermissionPattern<ComponentClass>;
pub type PolymorphicComponentPermissionPattern = PolymorphicClassPermissionPattern<ComponentClass>;

impl ComponentClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_component_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Component(match verb {
            "*" => ComponentPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => ComponentPermissionPattern::Verb {
                verb: ComponentVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => ComponentPermissionPattern::Verb {
                verb: ComponentVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => ComponentPermissionPattern::Verb {
                verb: ComponentVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => ComponentPermissionPattern::Verb {
                verb: ComponentVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::Component(match verb {
            "*" => PolymorphicComponentPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicComponentPermissionPattern::Verb {
                verb: ComponentVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicComponentPermissionPattern::Verb {
                verb: ComponentVerb::Create,
                owner,
                recipient,
                resource,
            },
            "update" => PolymorphicComponentPermissionPattern::Verb {
                verb: ComponentVerb::Update,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicComponentPermissionPattern::Verb {
                verb: ComponentVerb::Delete,
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
    ) -> Result<ComponentResourcePattern, CardParseError> {
        if resource.is_empty() {
            Ok(ComponentResourcePattern)
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
    ) -> Result<PolymorphicComponentResourcePattern, CardParseError> {
        if let Ok(resource) = Self::parse_resource(class, resource) {
            Ok(PolymorphicComponentResourcePattern::Concrete(resource))
        } else if let Ok(slot) = SlotVariable::parse(resource) {
            Ok(PolymorphicComponentResourcePattern::Slot(slot))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}
