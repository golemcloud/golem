use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_environment_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_environment_owner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum BlobResourcePattern {
    BucketKey { bucket: String, key_pattern: String },
}

impl BlobResourcePattern {
    pub fn any() -> Self {
        Self::BucketKey {
            bucket: "*".to_string(),
            key_pattern: "**".to_string(),
        }
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::parse_value(&value.into()).unwrap_or_else(|value| Self::BucketKey {
            bucket: value,
            key_pattern: String::new(),
        })
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::exact(value)
    }

    fn parse_value(value: &str) -> Result<Self, String> {
        let Some((bucket, key_pattern)) = value.split_once('.') else {
            return Err(value.to_string());
        };
        if bucket.is_empty() || key_pattern.is_empty() {
            return Err(value.to_string());
        }
        Ok(Self::BucketKey {
            bucket: bucket.to_string(),
            key_pattern: key_pattern.to_string(),
        })
    }
}

impl Subsumes for BlobResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::BucketKey {
                    bucket: a_bucket,
                    key_pattern: a_key,
                },
                Self::BucketKey {
                    bucket: b_bucket,
                    key_pattern: b_key,
                },
            ) => (a_bucket == "*" || a_bucket == b_bucket) && glob_subsumes(a_key, b_key),
        }
    }
}

pub type PolymorphicBlobResourcePattern = BlobResourcePattern;

impl ResourcePattern for BlobResourcePattern {
    type Polymorphic = PolymorphicBlobResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum BlobVerb {
    Read,
    Write,
    Delete,
    List,
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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "read" => Some(Self::Verb::Read),
            "write" => Some(Self::Verb::Write),
            "delete" => Some(Self::Verb::Delete),
            "list" => Some(Self::Verb::List),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_environment_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_agent_recipient(recipient)
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
        parse_polymorphic_agent_recipient(recipient)
    }

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Blob(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Blob(pattern)
    }
}

pub type BlobPermissionPattern = ClassPermissionPattern<BlobClass>;
pub type PolymorphicBlobPermissionPattern = PolymorphicClassPermissionPattern<BlobClass>;

impl BlobClass {
    fn parse_resource(_class: &str, resource: &str) -> Result<BlobResourcePattern, CardParseError> {
        BlobResourcePattern::parse_value(resource).map_err(|_| CardParseError::InvalidResource {
            class: BlobClass::NAME.to_string(),
            resource: resource.to_string(),
        })
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicBlobResourcePattern, CardParseError> {
        Self::parse_resource(class, resource)
    }
}
