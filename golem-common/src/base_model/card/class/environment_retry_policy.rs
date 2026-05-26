use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRetryPolicyResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentRetryPolicyResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentRetryPolicyResourcePattern {
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
pub enum PolymorphicEnvironmentRetryPolicyResourcePattern {
    Concrete(EnvironmentRetryPolicyResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentRetryPolicyResourcePattern {
    type Polymorphic = PolymorphicEnvironmentRetryPolicyResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRetryPolicyVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentRetryPolicyClass;

impl PermissionClass for EnvironmentRetryPolicyClass {
    type Verb = EnvironmentRetryPolicyVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentRetryPolicyResourcePattern;
    const NAME: &'static str = "environment.retry-policy";
}

pub type EnvironmentRetryPolicyPermissionPattern =
    ClassPermissionPattern<EnvironmentRetryPolicyClass>;
pub type PolymorphicEnvironmentRetryPolicyPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentRetryPolicyClass>;
