use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentDomainRegistrationResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentDomainRegistrationResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentDomainRegistrationResourcePattern {
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
pub enum PolymorphicEnvironmentDomainRegistrationResourcePattern {
    Concrete(EnvironmentDomainRegistrationResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentDomainRegistrationResourcePattern {
    type Polymorphic = PolymorphicEnvironmentDomainRegistrationResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentDomainRegistrationVerb {
    View,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentDomainRegistrationClass;

impl PermissionClass for EnvironmentDomainRegistrationClass {
    type Verb = EnvironmentDomainRegistrationVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentDomainRegistrationResourcePattern;
    const NAME: &'static str = "environment.domain-registration";
}

pub type EnvironmentDomainRegistrationPermissionPattern =
    ClassPermissionPattern<EnvironmentDomainRegistrationClass>;
pub type PolymorphicEnvironmentDomainRegistrationPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentDomainRegistrationClass>;
