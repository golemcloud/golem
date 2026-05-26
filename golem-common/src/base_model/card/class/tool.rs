use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_polymorphic_agent_recipient,
    parse_polymorphic_tool_owner, parse_tool_owner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolResourcePattern {
    Any,
    Command(String),
}

impl ToolResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn command(command: impl Into<String>) -> Self {
        Self::Command(command.into())
    }
}

impl Subsumes for ToolResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Command(a), Self::Command(b)) => a == b,
            (Self::Command(_), Self::Any) => false,
        }
    }
}

pub type PolymorphicToolResourcePattern = ToolResourcePattern;

impl ResourcePattern for ToolResourcePattern {
    type Polymorphic = PolymorphicToolResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolVerb {
    Invoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolClass;

impl PermissionClass for ToolClass {
    type Verb = ToolVerb;
    type Owner = ToolOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = ToolResourcePattern;
    const NAME: &'static str = "tool";

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "invoke" => Some(Self::Verb::Invoke),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_tool_owner(Self::NAME, owner)
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
        parse_polymorphic_tool_owner(Self::NAME, owner)
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
        PermissionPattern::Tool(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Tool(pattern)
    }
}

pub type ToolPermissionPattern = ClassPermissionPattern<ToolClass>;
pub type PolymorphicToolPermissionPattern = PolymorphicClassPermissionPattern<ToolClass>;

impl ToolClass {
    fn parse_resource(_class: &str, resource: &str) -> Result<ToolResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(ToolResourcePattern::Any)
        } else {
            Ok(ToolResourcePattern::Command(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicToolResourcePattern, CardParseError> {
        Self::parse_resource(class, resource)
    }
}
