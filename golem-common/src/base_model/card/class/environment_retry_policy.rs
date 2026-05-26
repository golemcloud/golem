use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRetryPolicyResourcePattern {
    Any,
    Name(ResourceIdentifier),
}

impl EnvironmentRetryPolicyResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(ResourceIdentifier::parse(&value.into()).expect("invalid retry policy name"))
    }
}

impl Subsumes for EnvironmentRetryPolicyResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Name(_), Self::Any) => false,
        }
    }
}

pub type PolymorphicEnvironmentRetryPolicyResourcePattern = EnvironmentRetryPolicyResourcePattern;

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
    Restore,
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

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentRetryPolicy(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentRetryPolicy(pattern)
    }
}

pub type EnvironmentRetryPolicyPermissionPattern =
    ClassPermissionPattern<EnvironmentRetryPolicyClass>;
pub type PolymorphicEnvironmentRetryPolicyPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentRetryPolicyClass>;

impl EnvironmentRetryPolicyClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentRetryPolicyResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentRetryPolicyResourcePattern::Any)
        } else {
            ResourceIdentifier::parse(resource)
                .map(EnvironmentRetryPolicyResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentRetryPolicyClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentRetryPolicyResourcePattern, CardParseError> {
        Self::parse_resource(class, resource)
    }
}
