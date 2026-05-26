use super::*;
use crate::base_model::card::parsing::CardParseError;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentShareResourcePattern {
    Any,
    Share(Uuid),
}

impl EnvironmentShareResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Share(Uuid::parse_str(&value.into()).expect("invalid share id"))
    }
}

impl Subsumes for EnvironmentShareResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Share(a), Self::Share(b)) => a == b,
            (Self::Share(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentShareVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentShareClass;

impl PermissionClass for EnvironmentShareClass {
    type Verb = EnvironmentShareVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentShareResourcePattern;
    const NAME: &'static str = "environment.share";

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

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentShare(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentShare(pattern)
    }
}

pub type EnvironmentSharePermissionPattern = ClassPermissionPattern<EnvironmentShareClass>;
pub type PolymorphicEnvironmentSharePermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentShareClass>;

impl EnvironmentShareClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentShareResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentShareResourcePattern::Any)
        } else {
            Uuid::parse_str(resource)
                .map(EnvironmentShareResourcePattern::Share)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentShareClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}
