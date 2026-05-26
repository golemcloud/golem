use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl RdbmsResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }

    pub fn glob(value: impl Into<String>) -> Self {
        Self::Glob(value.into())
    }
}

impl Subsumes for RdbmsResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Glob(a), Self::Glob(b)) => glob_subsumes(a, b),
            (Self::Glob(a), Self::Exact(b)) => glob_matches(a, b),
            (Self::Glob(_), Self::Any) => false,
            (Self::Exact(_), Self::Any | Self::Glob(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicRdbmsResourcePattern {
    Concrete(RdbmsResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for RdbmsResourcePattern {
    type Polymorphic = PolymorphicRdbmsResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RdbmsVerb {
    Query,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct RdbmsClass;

impl PermissionClass for RdbmsClass {
    type Verb = RdbmsVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = RdbmsResourcePattern;
    const NAME: &'static str = "rdbms";
}

pub type RdbmsPermissionPattern = ClassPermissionPattern<RdbmsClass>;
pub type PolymorphicRdbmsPermissionPattern = PolymorphicClassPermissionPattern<RdbmsClass>;
