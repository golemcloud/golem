use super::*;

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
