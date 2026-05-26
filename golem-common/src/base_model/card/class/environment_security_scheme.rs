use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentSecuritySchemeResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentSecuritySchemeResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentSecuritySchemeResourcePattern {
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
pub enum PolymorphicEnvironmentSecuritySchemeResourcePattern {
    Concrete(EnvironmentSecuritySchemeResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentSecuritySchemeResourcePattern {
    type Polymorphic = PolymorphicEnvironmentSecuritySchemeResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentSecuritySchemeVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentSecuritySchemeClass;

impl PermissionClass for EnvironmentSecuritySchemeClass {
    type Verb = EnvironmentSecuritySchemeVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentSecuritySchemeResourcePattern;
    const NAME: &'static str = "environment.security-scheme";
}

pub type EnvironmentSecuritySchemePermissionPattern =
    ClassPermissionPattern<EnvironmentSecuritySchemeClass>;
pub type PolymorphicEnvironmentSecuritySchemePermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentSecuritySchemeClass>;
