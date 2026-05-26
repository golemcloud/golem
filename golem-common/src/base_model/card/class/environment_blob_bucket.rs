use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentBlobBucketResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentBlobBucketResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentBlobBucketResourcePattern {
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
pub enum PolymorphicEnvironmentBlobBucketResourcePattern {
    Concrete(EnvironmentBlobBucketResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentBlobBucketResourcePattern {
    type Polymorphic = PolymorphicEnvironmentBlobBucketResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentBlobBucketVerb {
    View,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentBlobBucketClass;

impl PermissionClass for EnvironmentBlobBucketClass {
    type Verb = EnvironmentBlobBucketVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentBlobBucketResourcePattern;
    const NAME: &'static str = "environment.blob-bucket";
}

pub type EnvironmentBlobBucketPermissionPattern =
    ClassPermissionPattern<EnvironmentBlobBucketClass>;
pub type PolymorphicEnvironmentBlobBucketPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentBlobBucketClass>;

impl EnvironmentBlobBucketClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentBlobBucket(match verb {
            "*" => EnvironmentBlobBucketPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentBlobBucketPermissionPattern::Verb {
                verb: EnvironmentBlobBucketVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentBlobBucketPermissionPattern::Verb {
                verb: EnvironmentBlobBucketVerb::Create,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentBlobBucketPermissionPattern::Verb {
                verb: EnvironmentBlobBucketVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentBlobBucket(
            match verb {
                "*" => PolymorphicEnvironmentBlobBucketPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentBlobBucketPermissionPattern::Verb {
                    verb: EnvironmentBlobBucketVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentBlobBucketPermissionPattern::Verb {
                    verb: EnvironmentBlobBucketVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentBlobBucketPermissionPattern::Verb {
                    verb: EnvironmentBlobBucketVerb::Delete,
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
    ) -> Result<EnvironmentBlobBucketResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentBlobBucketResourcePattern::Any)
        } else {
            Ok(EnvironmentBlobBucketResourcePattern::Exact(
                resource.to_string(),
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicEnvironmentBlobBucketResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentBlobBucketResourcePattern::Concrete,
            PolymorphicEnvironmentBlobBucketResourcePattern::Slot,
            PolymorphicEnvironmentBlobBucketResourcePattern::Template,
        )
    }
}
