use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::EnvironmentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentSecuritySchemeResourcePattern {
    Any,
    Name(EnvironmentSecuritySchemeName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvironmentSecuritySchemeName(pub String);

impl EnvironmentSecuritySchemeName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_environment_security_scheme_identifier(value).map(Self)
    }
}

impl EnvironmentSecuritySchemeResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(
            EnvironmentSecuritySchemeName::parse(&value.into()).expect("invalid scheme name"),
        )
    }
}

impl ResourcePattern for EnvironmentSecuritySchemeResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentSecuritySchemeResourcePattern::Any)
        } else {
            EnvironmentSecuritySchemeName::parse(resource)
                .map(EnvironmentSecuritySchemeResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentSecuritySchemeClass::NAME.to_string(),
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
pub enum EnvironmentSecuritySchemeVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}
impl VerbPattern for EnvironmentSecuritySchemeVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "restore" => Some(Self::Restore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentSecuritySchemeClass;

impl PermissionClass for EnvironmentSecuritySchemeClass {
    type Verb = EnvironmentSecuritySchemeVerb;
    type Owner = EnvironmentOwnerPattern;
    type Resource = EnvironmentSecuritySchemeResourcePattern;
    const NAME: &'static str = "environment.security-scheme";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::EnvironmentSecurityScheme(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentSecurityScheme(pattern)
    }
}


fn parse_environment_security_scheme_identifier(value: &str) -> Result<String, String> {
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
