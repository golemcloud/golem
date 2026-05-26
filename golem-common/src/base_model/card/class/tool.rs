use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_recipient, parse_polymorphic_agent_recipient,
    parse_polymorphic_resource, parse_polymorphic_tool_owner, parse_tool_owner,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicToolResourcePattern {
    Concrete(ToolResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

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
}

pub type ToolPermissionPattern = ClassPermissionPattern<ToolClass>;
pub type PolymorphicToolPermissionPattern = PolymorphicClassPermissionPattern<ToolClass>;

impl ToolClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_tool_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Tool(match verb {
            "*" => ToolPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "invoke" => ToolPermissionPattern::Verb {
                verb: ToolVerb::Invoke,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

    pub(crate) fn parse_polymorphic_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PolymorphicPermissionPattern, CardParseError> {
        let owner = parse_polymorphic_tool_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Tool(match verb {
            "*" => PolymorphicToolPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "invoke" => PolymorphicToolPermissionPattern::Verb {
                verb: ToolVerb::Invoke,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

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
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicToolResourcePattern::Concrete,
            PolymorphicToolResourcePattern::Slot,
            PolymorphicToolResourcePattern::Template,
        )
    }
}
