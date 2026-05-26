use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AgentOwnerPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvResourcePattern {
    Any,
    VarName(EnvVarName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvVarName(pub String);

impl EnvVarName {
    fn parse(value: &str) -> Result<Self, String> {
        let mut chars = value.chars();
        if chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            Ok(Self(value.to_string()))
        } else {
            Err(value.to_string())
        }
    }
}

impl EnvResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::VarName(EnvVarName::parse(&value.into()).expect("invalid env var name"))
    }
}

impl ResourcePattern for EnvResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(EnvResourcePattern::Any)
        } else {
            EnvVarName::parse(resource)
                .map(EnvResourcePattern::VarName)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::VarName(a), Self::VarName(b)) => a == b,
            (Self::VarName(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvVerb {
    Read,
}
impl VerbPattern for EnvVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "read" => Some(Self::Read),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvClass;

impl PermissionClass for EnvClass {
    type Verb = EnvVerb;
    type Owner = AgentOwnerPattern;
    type Resource = EnvResourcePattern;
    const NAME: &'static str = "env";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Env(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Env(pattern)
    }
}

pub type EnvPermissionPattern = ClassPermissionPattern<EnvClass>;
pub type PolymorphicEnvPermissionPattern = PolymorphicClassPermissionPattern<EnvClass>;
