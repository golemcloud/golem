use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_environment_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_environment_owner,
    parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum BlobResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl BlobResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::Glob(value.into())
    }
}

impl Subsumes for BlobResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Glob(a), Self::Glob(b)) => glob_subsumes(a, b),
            (Self::Glob(a), Self::Exact(b)) => glob_matches(a, b),
            (Self::Glob(_), Self::Any) => false,
            (Self::Exact(_), Self::Any | Self::Glob(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicBlobResourcePattern {
    Concrete(BlobResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for BlobResourcePattern {
    type Polymorphic = PolymorphicBlobResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum BlobVerb {
    Read,
    Write,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct BlobClass;

impl PermissionClass for BlobClass {
    type Verb = BlobVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = BlobResourcePattern;
    const NAME: &'static str = "blob";
}

pub type BlobPermissionPattern = ClassPermissionPattern<BlobClass>;
pub type PolymorphicBlobPermissionPattern = PolymorphicClassPermissionPattern<BlobClass>;

impl BlobClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_environment_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Blob(match verb {
            "*" => BlobPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => BlobPermissionPattern::Verb {
                verb: BlobVerb::Read,
                owner,
                recipient,
                resource,
            },
            "write" => BlobPermissionPattern::Verb {
                verb: BlobVerb::Write,
                owner,
                recipient,
                resource,
            },
            "delete" => BlobPermissionPattern::Verb {
                verb: BlobVerb::Delete,
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
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Blob(match verb {
            "*" => PolymorphicBlobPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => PolymorphicBlobPermissionPattern::Verb {
                verb: BlobVerb::Read,
                owner,
                recipient,
                resource,
            },
            "write" => PolymorphicBlobPermissionPattern::Verb {
                verb: BlobVerb::Write,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicBlobPermissionPattern::Verb {
                verb: BlobVerb::Delete,
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

    fn parse_resource(_class: &str, resource: &str) -> Result<BlobResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(BlobResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(BlobResourcePattern::Glob(resource.to_string()))
        } else {
            Ok(BlobResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicBlobResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicBlobResourcePattern::Concrete,
            PolymorphicBlobResourcePattern::Slot,
            PolymorphicBlobResourcePattern::Template,
        )
    }
}
