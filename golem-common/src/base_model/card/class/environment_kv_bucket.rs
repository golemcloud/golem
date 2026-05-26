use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentKvBucketResourcePattern {
    Any,
    Name(EnvironmentKvBucketName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvironmentKvBucketName(pub String);

impl EnvironmentKvBucketName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_environment_kv_bucket_identifier(value).map(Self)
    }
}

impl EnvironmentKvBucketResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(EnvironmentKvBucketName::parse(&value.into()).expect("invalid KV bucket name"))
    }
}

impl ResourcePattern for EnvironmentKvBucketResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentKvBucketResourcePattern::Any)
        } else {
            EnvironmentKvBucketName::parse(resource)
                .map(EnvironmentKvBucketResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentKvBucketClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

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
pub enum EnvironmentKvBucketVerb {
    View,
    Create,
    Delete,
    Clear,
}
impl VerbPattern for EnvironmentKvBucketVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "delete" => Some(Self::Delete),
            "clear" => Some(Self::Clear),
            _ => None,
        }
    }
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

fn parse_environment_kv_bucket_identifier(value: &str) -> Result<String, String> {
    let mut chars = value.chars();
    if chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(value.to_string())
    } else {
        Err(value.to_string())
    }
}
