use super::*;
use crate::base_model::card::parsing::CardParseError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourceDefinitionResourcePattern {
    Any,
    Name(EnvironmentResourceDefinitionName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct EnvironmentResourceDefinitionName(pub String);

impl EnvironmentResourceDefinitionName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_environment_resource_definition_identifier(value).map(Self)
    }
}

impl EnvironmentResourceDefinitionResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Name(
            EnvironmentResourceDefinitionName::parse(&value.into())
                .expect("invalid resource definition name"),
        )
    }
}

impl Subsumes for EnvironmentResourceDefinitionResourcePattern {
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
pub enum EnvironmentResourceDefinitionVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentResourceDefinitionClass;

impl PermissionClass for EnvironmentResourceDefinitionClass {
    type Verb = EnvironmentResourceDefinitionVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentResourceDefinitionResourcePattern;
    const NAME: &'static str = "environment.resource-definition";

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
        PermissionPattern::EnvironmentResourceDefinition(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::EnvironmentResourceDefinition(pattern)
    }
}

pub type EnvironmentResourceDefinitionPermissionPattern =
    ClassPermissionPattern<EnvironmentResourceDefinitionClass>;
pub type PolymorphicEnvironmentResourceDefinitionPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentResourceDefinitionClass>;

impl EnvironmentResourceDefinitionClass {
    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<EnvironmentResourceDefinitionResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(EnvironmentResourceDefinitionResourcePattern::Any)
        } else {
            EnvironmentResourceDefinitionName::parse(resource)
                .map(EnvironmentResourceDefinitionResourcePattern::Name)
                .map_err(|_| CardParseError::InvalidResource {
                    class: EnvironmentResourceDefinitionClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
        }
    }
}

fn parse_environment_resource_definition_identifier(value: &str) -> Result<String, String> {
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
