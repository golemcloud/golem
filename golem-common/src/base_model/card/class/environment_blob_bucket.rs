use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentBlobBucketResourcePattern {
    Any,
    Exact(String),
}

impl EnvironmentBlobBucketResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn exact(value: impl Into<String>) -> Self {
        Self::Exact(value.into())
    }
}

impl Subsumes for EnvironmentBlobBucketResourcePattern {
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
pub enum PolymorphicEnvironmentBlobBucketResourcePattern {
    Concrete(EnvironmentBlobBucketResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for EnvironmentBlobBucketResourcePattern {
    type Polymorphic = PolymorphicEnvironmentBlobBucketResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentBlobBucketVerb {
    View,
    Create,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EnvironmentBlobBucketClass;

impl PermissionClass for EnvironmentBlobBucketClass {
    type Verb = EnvironmentBlobBucketVerb;
    type Owner = EnvironmentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = EnvironmentBlobBucketResourcePattern;
    const NAME: &'static str = "environment.blob-bucket";
}

pub type EnvironmentBlobBucketPermissionPattern =
    ClassPermissionPattern<EnvironmentBlobBucketClass>;
pub type PolymorphicEnvironmentBlobBucketPermissionPattern =
    PolymorphicClassPermissionPattern<EnvironmentBlobBucketClass>;
