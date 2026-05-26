use super::*;

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
