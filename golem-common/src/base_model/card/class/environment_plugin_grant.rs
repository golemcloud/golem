use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EnvironmentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentPluginGrantResourcePattern {
    Any,
    Name(EnvironmentPluginGrantName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvironmentPluginGrantName(pub String);

impl EnvironmentPluginGrantName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_environment_plugin_grant_identifier(value).map(Self)
    }
}

impl EnvironmentPluginGrantResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(EnvironmentPluginGrantName::parse(&value.into()).expect("invalid plugin name"))
    }
}

impl ResourcePattern for EnvironmentPluginGrantResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentPluginGrantResourcePattern::Any)
        } else {
            EnvironmentPluginGrantName::parse(resource)
                .map(EnvironmentPluginGrantResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentPluginGrantClass::NAME.to_string(),
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
pub enum EnvironmentPluginGrantVerb {
    View,
    Create,
    Delete,
}
impl VerbPattern for EnvironmentPluginGrantVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentPluginGrantClass;

impl PermissionClass for EnvironmentPluginGrantClass {
    type Verb = EnvironmentPluginGrantVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = EnvironmentPluginGrantResourcePattern;
    const NAME: &'static str = "environment.plugin-grant";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentPluginGrant(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentPluginGrant(pattern)
    }
}

fn parse_environment_plugin_grant_identifier(value: &str) -> Result<String, String> {
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
