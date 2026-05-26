use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourceDefinitionResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentResourceDefinitionResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentResourceDefinitionResourcePattern {
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
pub enum PolymorphicEnvironmentResourceDefinitionResourcePattern {
    Concrete(EnvironmentResourceDefinitionResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentResourceDefinitionResourcePattern {
    type Polymorphic = PolymorphicEnvironmentResourceDefinitionResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentResourceDefinitionVerb {
    View,
    Create,
    Update,
    Delete,
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
}

pub type EnvironmentResourceDefinitionPermissionPattern =
    ClassPermissionPattern<EnvironmentResourceDefinitionClass>;
pub type PolymorphicEnvironmentResourceDefinitionPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentResourceDefinitionClass>;
