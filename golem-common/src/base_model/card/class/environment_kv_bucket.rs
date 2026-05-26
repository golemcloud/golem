use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentKvBucketResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentKvBucketResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentKvBucketResourcePattern {
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
pub enum PolymorphicEnvironmentKvBucketResourcePattern {
    Concrete(EnvironmentKvBucketResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentKvBucketResourcePattern {
    type Polymorphic = PolymorphicEnvironmentKvBucketResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentKvBucketVerb {
    View,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentKvBucketClass;

impl PermissionClass for EnvironmentKvBucketClass {
    type Verb = EnvironmentKvBucketVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentKvBucketResourcePattern;
    const NAME: &'static str = "environment.kv-bucket";
}

pub type EnvironmentKvBucketPermissionPattern = ClassPermissionPattern<EnvironmentKvBucketClass>;
pub type PolymorphicEnvironmentKvBucketPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentKvBucketClass>;
