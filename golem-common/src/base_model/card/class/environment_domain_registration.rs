use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentDomainRegistrationResourcePattern {
    Any,
    Domain(DomainNamePattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct DomainNamePattern {
    pub labels: Vec<DomainLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct DomainLabel(pub String);

impl DomainLabel {
    fn parse(value: &str) -> Result<Self, String> {
        let mut chars = value.chars();
        if chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            Ok(Self(value.to_string()))
        } else {
            Err(value.to_string())
        }
    }
}

impl DomainNamePattern {
    fn parse(value: &str) -> Result<Self, String> {
        let labels = value
            .split('.')
            .map(DomainLabel::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if labels.is_empty() {
            Err(value.to_string())
        } else {
            Ok(Self { labels })
        }
    }
}

impl EnvironmentDomainRegistrationResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Domain(DomainNamePattern::parse(&value.into()).expect("invalid domain name"))
    }
}

impl Subsumes for EnvironmentDomainRegistrationResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Domain(a), Self::Domain(b)) => a == b,
            (Self::Domain(_), Self::Any) => false,
        }
    }
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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "delete" => Some(Self::Verb::Delete),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_environment_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_environment_recipient(recipient)
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_environment_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_environment_recipient(recipient)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentDomainRegistration(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentDomainRegistration(pattern)
    }
}

pub type EnvironmentDomainRegistrationPermissionPattern =
    ClassPermissionPattern<EnvironmentDomainRegistrationClass>;
pub type PolymorphicEnvironmentDomainRegistrationPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentDomainRegistrationClass>;

impl EnvironmentDomainRegistrationClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentDomainRegistrationResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentDomainRegistrationResourcePattern::Any)
        } else {
            DomainNamePattern::parse(resource)
                .map(EnvironmentDomainRegistrationResourcePattern::Domain)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentDomainRegistrationClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}
