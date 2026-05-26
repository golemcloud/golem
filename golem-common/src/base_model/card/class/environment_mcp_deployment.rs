use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentMcpDeploymentResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentMcpDeploymentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentMcpDeploymentResourcePattern {
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
pub enum PolymorphicEnvironmentMcpDeploymentResourcePattern {
    Concrete(EnvironmentMcpDeploymentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentMcpDeploymentResourcePattern {
    type Polymorphic = PolymorphicEnvironmentMcpDeploymentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentMcpDeploymentVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentMcpDeploymentClass;

impl PermissionClass for EnvironmentMcpDeploymentClass {
    type Verb = EnvironmentMcpDeploymentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentMcpDeploymentResourcePattern;
    const NAME: &'static str = "environment.mcp-deployment";
}

pub type EnvironmentMcpDeploymentPermissionPattern =
    ClassPermissionPattern<EnvironmentMcpDeploymentClass>;
pub type PolymorphicEnvironmentMcpDeploymentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentMcpDeploymentClass>;
