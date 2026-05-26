use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentHttpApiDeploymentResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentHttpApiDeploymentResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentHttpApiDeploymentResourcePattern {
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
pub enum PolymorphicEnvironmentHttpApiDeploymentResourcePattern {
    Concrete(EnvironmentHttpApiDeploymentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentHttpApiDeploymentResourcePattern {
    type Polymorphic = PolymorphicEnvironmentHttpApiDeploymentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentHttpApiDeploymentVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentHttpApiDeploymentClass;

impl PermissionClass for EnvironmentHttpApiDeploymentClass {
    type Verb = EnvironmentHttpApiDeploymentVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentHttpApiDeploymentResourcePattern;
    const NAME: &'static str = "environment.http-api-deployment";
}

pub type EnvironmentHttpApiDeploymentPermissionPattern =
    ClassPermissionPattern<EnvironmentHttpApiDeploymentClass>;
pub type PolymorphicEnvironmentHttpApiDeploymentPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentHttpApiDeploymentClass>;
