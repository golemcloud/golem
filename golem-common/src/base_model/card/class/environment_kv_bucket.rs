use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentKvBucketResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentKvBucketResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentKvBucketResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(_), Self::Any) => false,
        }
    }
}

pub type PolymorphicEnvironmentKvBucketResourcePattern = EnvironmentKvBucketResourcePattern;

impl ResourcePattern for EnvironmentKvBucketResourcePattern {
    type Polymorphic = PolymorphicEnvironmentKvBucketResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentKvBucketVerb {
    View,
    Create,
    Delete,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentKvBucketClass;

impl PermissionClass for EnvironmentKvBucketClass {
    type Verb = EnvironmentKvBucketVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentKvBucketResourcePattern;
    const NAME: &'static str = "environment.kv-bucket";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "view" => Some(Self::Verb::View),
            "create" => Some(Self::Verb::Create),
            "delete" => Some(Self::Verb::Delete),
            "clear" => Some(Self::Verb::Clear),
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
        PermissionPattern::EnvironmentKvBucket(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentKvBucket(pattern)
    }
}

pub type EnvironmentKvBucketPermissionPattern = ClassPermissionPattern<EnvironmentKvBucketClass>;
pub type PolymorphicEnvironmentKvBucketPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentKvBucketClass>;

impl EnvironmentKvBucketClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentKvBucketResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentKvBucketResourcePattern::Any)
        } else {
            Ok(EnvironmentKvBucketResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentKvBucketResourcePattern, CardParseError> {
        Self::parse_resource(class, resource)
    }
}
