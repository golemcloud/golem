use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentPluginGrantResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentPluginGrantResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentPluginGrantResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentPluginGrantResourcePattern {
    Concrete(EnvironmentPluginGrantResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentPluginGrantResourcePattern {
    type Polymorphic = PolymorphicEnvironmentPluginGrantResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentPluginGrantVerb {
    View,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentPluginGrantClass;

impl PermissionClass for EnvironmentPluginGrantClass {
    type Verb = EnvironmentPluginGrantVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentPluginGrantResourcePattern;
    const NAME: &'static str = "environment.plugin-grant";
}

pub type EnvironmentPluginGrantPermissionPattern =
    ClassPermissionPattern<EnvironmentPluginGrantClass>;
pub type PolymorphicEnvironmentPluginGrantPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentPluginGrantClass>;
