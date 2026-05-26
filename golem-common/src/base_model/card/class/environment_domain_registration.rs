use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentDomainRegistrationResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentDomainRegistrationResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentDomainRegistrationResourcePattern {
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
pub enum PolymorphicEnvironmentDomainRegistrationResourcePattern {
    Concrete(EnvironmentDomainRegistrationResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentDomainRegistrationResourcePattern {
    type Polymorphic = PolymorphicEnvironmentDomainRegistrationResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentDomainRegistrationVerb {
    View,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentDomainRegistrationClass;

impl PermissionClass for EnvironmentDomainRegistrationClass {
    type Verb = EnvironmentDomainRegistrationVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentDomainRegistrationResourcePattern;
    const NAME: &'static str = "environment.domain-registration";
}

pub type EnvironmentDomainRegistrationPermissionPattern =
    ClassPermissionPattern<EnvironmentDomainRegistrationClass>;
pub type PolymorphicEnvironmentDomainRegistrationPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentDomainRegistrationClass>;

impl EnvironmentDomainRegistrationClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentDomainRegistration(
            match verb {
                "*" => EnvironmentDomainRegistrationPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => EnvironmentDomainRegistrationPermissionPattern::Verb {
                    verb: EnvironmentDomainRegistrationVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => EnvironmentDomainRegistrationPermissionPattern::Verb {
                    verb: EnvironmentDomainRegistrationVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => EnvironmentDomainRegistrationPermissionPattern::Verb {
                    verb: EnvironmentDomainRegistrationVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentDomainRegistration(
            match verb {
                "*" => PolymorphicEnvironmentDomainRegistrationPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentDomainRegistrationPermissionPattern::Verb {
                    verb: EnvironmentDomainRegistrationVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentDomainRegistrationPermissionPattern::Verb {
                    verb: EnvironmentDomainRegistrationVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentDomainRegistrationPermissionPattern::Verb {
                    verb: EnvironmentDomainRegistrationVerb::Delete,
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
    ) -> Result<EnvironmentDomainRegistrationResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentDomainRegistrationResourcePattern::Any)
        } else {
            Ok(EnvironmentDomainRegistrationResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentDomainRegistrationResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentDomainRegistrationResourcePattern::Concrete,
            PolymorphicEnvironmentDomainRegistrationResourcePattern::Slot,
            PolymorphicEnvironmentDomainRegistrationResourcePattern::Template,
        )
    }
}
