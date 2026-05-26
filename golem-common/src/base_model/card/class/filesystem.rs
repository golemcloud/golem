use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_owner, parse_agent_recipient, parse_polymorphic_agent_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_resource,
};

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

impl FilesystemClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_agent_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Filesystem(match verb {
            "*" => FilesystemPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Read,
                owner,
                recipient,
                resource,
            },
            "write" => FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Write,
                owner,
                recipient,
                resource,
            },
            "list" => FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::List,
                owner,
                recipient,
                resource,
            },
            "stat" => FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Stat,
                owner,
                recipient,
                resource,
            },
            "delete" => FilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Delete,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

    pub(crate) fn parse_polymorphic_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PolymorphicPermissionPattern, CardParseError> {
        let owner = parse_polymorphic_agent_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Filesystem(match verb {
            "*" => PolymorphicFilesystemPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => PolymorphicFilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Read,
                owner,
                recipient,
                resource,
            },
            "write" => PolymorphicFilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Write,
                owner,
                recipient,
                resource,
            },
            "list" => PolymorphicFilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::List,
                owner,
                recipient,
                resource,
            },
            "stat" => PolymorphicFilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Stat,
                owner,
                recipient,
                resource,
            },
            "delete" => PolymorphicFilesystemPermissionPattern::Verb {
                verb: FilesystemVerb::Delete,
                owner,
                recipient,
                resource,
            },
            other => {
                return Err(CardParseError::UnknownVerb {
                    class: Self::NAME.to_string(),
                    verb: other.to_string(),
                });
            }
        }))
    }

    fn parse_resource(
        _class: &str,
        resource: &str,
    ) -> Result<FilesystemResourcePattern, CardParseError> {
        if resource == "*" || resource == "**" {
            Ok(FilesystemResourcePattern::Any)
        } else if resource.contains('*') {
            Ok(FilesystemResourcePattern::Glob(resource.to_string()))
        } else {
            Ok(FilesystemResourcePattern::Exact(resource.to_string()))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicFilesystemResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicFilesystemResourcePattern::Concrete,
            PolymorphicFilesystemResourcePattern::Slot,
            PolymorphicFilesystemResourcePattern::Template,
        )
    }
}
