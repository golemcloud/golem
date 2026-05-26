use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_owner, parse_agent_recipient, parse_polymorphic_agent_owner,
    parse_polymorphic_agent_recipient,
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentResourcePattern {
    Any,
    Empty,
    Method(ResourceIdentifier),
    OplogIndex(u64),
    InvocationId(AgentInvocationIdPattern),
    PluginName(ResourceIdentifier),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentInvocationIdPattern {
    Uuid(Uuid),
    Identifier(ResourceIdentifier),
}

impl AgentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn empty() -> Self {
        Self::Empty
    }

    pub fn method(method: impl Into<String>) -> Self {
        Self::Method(ResourceIdentifier::parse(&method.into()).expect("invalid method name"))
    }
}

impl Subsumes for AgentResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Empty, Self::Empty) => true,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AgentClass;

impl PermissionClass for AgentClass {
    type Verb = AgentVerb;
    type Owner = AgentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = AgentResourcePattern;
    const NAME: &'static str = "agent";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "invoke" => Some(Self::Verb::Invoke),
            "view" => Some(Self::Verb::View),
            "delete" => Some(Self::Verb::Delete),
            "interrupt" => Some(Self::Verb::Interrupt),
            "resume" => Some(Self::Verb::Resume),
            "update-revision" => Some(Self::Verb::UpdateRevision),
            "fork" => Some(Self::Verb::Fork),
            "revert" => Some(Self::Verb::Revert),
            "cancel-invocation" => Some(Self::Verb::CancelInvocation),
            "activate-plugin" => Some(Self::Verb::ActivatePlugin),
            "deactivate-plugin" => Some(Self::Verb::DeactivatePlugin),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_agent_owner(Self::NAME, owner)
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
        parse_polymorphic_agent_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_agent_recipient(recipient)
    }

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

impl AgentClass {
    fn parse_resource(class: &str, resource: &str) -> Result<AgentResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AgentResourcePattern::Any)
        } else if resource.is_empty() {
            Ok(AgentResourcePattern::Empty)
        } else if let Ok(index) = resource.parse::<u64>() {
            Ok(AgentResourcePattern::OplogIndex(index))
        } else if let Ok(uuid) = Uuid::parse_str(resource) {
            Ok(AgentResourcePattern::InvocationId(
                AgentInvocationIdPattern::Uuid(uuid),
            ))
        } else if let Ok(identifier) = ResourceIdentifier::parse(resource) {
            Ok(AgentResourcePattern::Method(identifier))
        } else {
            Err(CardParseError::InvalidResource {
                class: class.to_string(),
                resource: resource.to_string(),
            })
        }
    }
}
