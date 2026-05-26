use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_environment_owner, parse_environment_recipient,
    parse_polymorphic_environment_owner, parse_polymorphic_environment_recipient,
    parse_polymorphic_resource,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentKvBucketResourcePattern {
    Concrete(EnvironmentKvBucketResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentKvBucketResourcePattern {
    type Polymorphic = PolymorphicEnvironmentKvBucketResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentKvBucketVerb {
    View,
    Create,
    Delete,
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
}

pub type EnvironmentKvBucketPermissionPattern = ClassPermissionPattern<EnvironmentKvBucketClass>;
pub type PolymorphicEnvironmentKvBucketPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentKvBucketClass>;

impl EnvironmentKvBucketClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_environment_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::EnvironmentKvBucket(match verb {
            "*" => EnvironmentKvBucketPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "view" => EnvironmentKvBucketPermissionPattern::Verb {
                verb: EnvironmentKvBucketVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => EnvironmentKvBucketPermissionPattern::Verb {
                verb: EnvironmentKvBucketVerb::Create,
                owner,
                recipient,
                resource,
            },
            "delete" => EnvironmentKvBucketPermissionPattern::Verb {
                verb: EnvironmentKvBucketVerb::Delete,
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
        Ok(PolymorphicPermissionPattern::EnvironmentKvBucket(
            match verb {
                "*" => PolymorphicEnvironmentKvBucketPermissionPattern::Any {
                    owner,
                    recipient,
                    resource,
                },
                "view" => PolymorphicEnvironmentKvBucketPermissionPattern::Verb {
                    verb: EnvironmentKvBucketVerb::View,
                    owner,
                    recipient,
                    resource,
                },
                "create" => PolymorphicEnvironmentKvBucketPermissionPattern::Verb {
                    verb: EnvironmentKvBucketVerb::Create,
                    owner,
                    recipient,
                    resource,
                },
                "delete" => PolymorphicEnvironmentKvBucketPermissionPattern::Verb {
                    verb: EnvironmentKvBucketVerb::Delete,
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
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicEnvironmentKvBucketResourcePattern::Concrete,
            PolymorphicEnvironmentKvBucketResourcePattern::Slot,
            PolymorphicEnvironmentKvBucketResourcePattern::Template,
        )
    }
}
