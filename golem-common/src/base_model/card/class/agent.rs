use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_owner, parse_agent_recipient, parse_polymorphic_agent_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentResourcePattern {
    Any,
    Empty,
    Method(String),
}

impl AgentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn empty() -> Self {
        Self::Empty
    }

    pub fn method(method: impl Into<String>) -> Self {
        Self::Method(method.into())
    }
}

impl Subsumes for AgentResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Empty, Self::Empty) => true,
            (Self::Method(a), Self::Method(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAgentResourcePattern {
    Concrete(AgentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for AgentResourcePattern {
    type Polymorphic = PolymorphicAgentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentVerb {
    Invoke,
    View,
    Create,
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
}

pub type AgentPermissionPattern = ClassPermissionPattern<AgentClass>;
pub type PolymorphicAgentPermissionPattern = PolymorphicClassPermissionPattern<AgentClass>;

impl AgentClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_agent_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Agent(match verb {
            "*" => AgentPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "invoke" => AgentPermissionPattern::Verb {
                verb: AgentVerb::Invoke,
                owner,
                recipient,
                resource,
            },
            "view" => AgentPermissionPattern::Verb {
                verb: AgentVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => AgentPermissionPattern::Verb {
                verb: AgentVerb::Create,
                owner,
                recipient,
                resource,
            },
            "delete" => AgentPermissionPattern::Verb {
                verb: AgentVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "interrupt" => AgentPermissionPattern::Verb {
                verb: AgentVerb::Interrupt,
                owner,
                recipient,
                resource,
            },
            "resume" => AgentPermissionPattern::Verb {
                verb: AgentVerb::Resume,
                owner,
                recipient,
                resource,
            },
            "update-revision" => AgentPermissionPattern::Verb {
                verb: AgentVerb::UpdateRevision,
                owner,
                recipient,
                resource,
            },
            "fork" => AgentPermissionPattern::Verb {
                verb: AgentVerb::Fork,
                owner,
                recipient,
                resource,
            },
            "revert" => AgentPermissionPattern::Verb {
                verb: AgentVerb::Revert,
                owner,
                recipient,
                resource,
            },
            "cancel-invocation" => AgentPermissionPattern::Verb {
                verb: AgentVerb::CancelInvocation,
                owner,
                recipient,
                resource,
            },
            "activate-plugin" => AgentPermissionPattern::Verb {
                verb: AgentVerb::ActivatePlugin,
                owner,
                recipient,
                resource,
            },
            "deactivate-plugin" => AgentPermissionPattern::Verb {
                verb: AgentVerb::DeactivatePlugin,
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
        let owner = parse_polymorphic_agent_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Agent(match verb {
            "*" => PolymorphicAgentPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "invoke" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Invoke,
                owner,
                recipient,
                resource,
            },
            "view" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::View,
                owner,
                recipient,
                resource,
            },
            "create" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Create,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Delete,
                owner,
                recipient,
                resource,
            },
            "interrupt" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Interrupt,
                owner,
                recipient,
                resource,
            },
            "resume" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Resume,
                owner,
                recipient,
                resource,
            },
            "update-revision" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::UpdateRevision,
                owner,
                recipient,
                resource,
            },
            "fork" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Fork,
                owner,
                recipient,
                resource,
            },
            "revert" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::Revert,
                owner,
                recipient,
                resource,
            },
            "cancel-invocation" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::CancelInvocation,
                owner,
                recipient,
                resource,
            },
            "activate-plugin" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::ActivatePlugin,
                owner,
                recipient,
                resource,
            },
            "deactivate-plugin" => PolymorphicAgentPermissionPattern::Verb {
                verb: AgentVerb::DeactivatePlugin,
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

    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<AgentResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(AgentResourcePattern::Any)
        } else if resource.is_empty() {
            Ok(AgentResourcePattern::Empty)
        } else {
            Ok(AgentResourcePattern::Method(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicAgentResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicAgentResourcePattern::Concrete,
            PolymorphicAgentResourcePattern::Slot,
            PolymorphicAgentResourcePattern::Template,
        )
    }
}
