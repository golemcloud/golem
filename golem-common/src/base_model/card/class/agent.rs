use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AgentOwnerPattern;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentResourcePattern {
    Any,
    Method(AgentMethodName),
    OplogIndex(u64),
    InvocationId(AgentInvocationIdPattern),
    PluginName(AgentPluginName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentMethodName(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentPluginName(pub String);

impl AgentMethodName {
    fn parse(value: &str) -> Result<Self, String> {
        parse_agent_identifier(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentInvocationIdPattern {
    Uuid(Uuid),
    Identifier(AgentInvocationIdentifier),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentInvocationIdentifier(pub String);

impl AgentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn method(method: impl Into<String>) -> Self {
        Self::Method(AgentMethodName::parse(&method.into()).expect("invalid method name"))
    }
}

impl ResourcePattern for AgentResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(AgentResourcePattern::Any)
        } else if let Ok(index) = resource.parse::<u64>() {
            Ok(AgentResourcePattern::OplogIndex(index))
        } else if let Ok(uuid) = Uuid::parse_str(resource) {
            Ok(AgentResourcePattern::InvocationId(
                AgentInvocationIdPattern::Uuid(uuid),
            ))
        } else if let Ok(identifier) = AgentMethodName::parse(resource) {
            Ok(AgentResourcePattern::Method(identifier))
        } else {
            Err(CardParseError::InvalidResource {
                class: AgentClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Method(a), Self::Method(b)) => a == b,
            (Self::OplogIndex(a), Self::OplogIndex(b)) => a == b,
            (Self::InvocationId(a), Self::InvocationId(b)) => a == b,
            (Self::PluginName(a), Self::PluginName(b)) => a == b,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentVerb {
    Invoke,
    View,
    Delete,
    Interrupt,
    Resume,
    UpdateRevision,
    Fork,
    Revert,
    CancelInvocation,
    ActivatePlugin,
    DeactivatePlugin,
}
impl VerbPattern for AgentVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "invoke" => Some(Self::Invoke),
            "view" => Some(Self::View),
            "delete" => Some(Self::Delete),
            "interrupt" => Some(Self::Interrupt),
            "resume" => Some(Self::Resume),
            "update-revision" => Some(Self::UpdateRevision),
            "fork" => Some(Self::Fork),
            "revert" => Some(Self::Revert),
            "cancel-invocation" => Some(Self::CancelInvocation),
            "activate-plugin" => Some(Self::ActivatePlugin),
            "deactivate-plugin" => Some(Self::DeactivatePlugin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AgentClass;

impl PermissionClass for AgentClass {
    type Verb = AgentVerb;
    type Owner = AgentOwnerPattern;
    type Resource = AgentResourcePattern;
    const NAME: &'static str = "agent";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Agent(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Agent(pattern)
    }
}

pub type AgentPermissionPattern = ClassPermissionPattern<AgentClass>;
pub type PolymorphicAgentPermissionPattern = PolymorphicClassPermissionPattern<AgentClass>;

fn parse_agent_identifier(value: &str) -> Result<String, String> {
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
