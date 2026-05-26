use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentSecuritySchemeResourcePattern {
    Any,
    Name(ResourceIdentifier),
}

impl EnvironmentSecuritySchemeResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(ResourceIdentifier::parse(&value.into()).expect("invalid scheme name"))
    }
}

impl Subsumes for EnvironmentSecuritySchemeResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Name(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentSecuritySchemeVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "update" => Some(Self::Verb::Update),
            "delete" => Some(Self::Verb::Delete),
            "restore" => Some(Self::Verb::Restore),
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
        PermissionPattern::EnvironmentSecurityScheme(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentSecurityScheme(pattern)
    }
}

pub type EnvironmentSecuritySchemePermissionPattern =
    ClassPermissionPattern<EnvironmentSecuritySchemeClass>;
pub type PolymorphicEnvironmentSecuritySchemePermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentSecuritySchemeClass>;

impl EnvironmentSecuritySchemeClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentSecuritySchemeResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentSecuritySchemeResourcePattern::Any)
        } else {
            ResourceIdentifier::parse(resource)
                .map(EnvironmentSecuritySchemeResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentSecuritySchemeClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}
