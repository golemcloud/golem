use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum FilesystemResourcePattern {
    Any,
    Exact(String),
    Glob(String),
}

impl FilesystemResourcePattern {
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

impl Subsumes for FilesystemResourcePattern {
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
pub enum PolymorphicFilesystemResourcePattern {
    Concrete(FilesystemResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for FilesystemResourcePattern {
    type Polymorphic = PolymorphicFilesystemResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum FilesystemVerb {
    Read,
    Write,
    List,
    Stat,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct FilesystemClass;

impl PermissionClass for FilesystemClass {
    type Verb = FilesystemVerb;
    type Owner = AgentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = FilesystemResourcePattern;
    const NAME: &'static str = "filesystem";
}

pub type FilesystemPermissionPattern = ClassPermissionPattern<FilesystemClass>;
pub type PolymorphicFilesystemPermissionPattern =
    PolymorphicClassPermissionPattern<FilesystemClass>;
