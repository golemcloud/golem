use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_agent_owner, parse_agent_recipient, parse_polymorphic_agent_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum OplogResourcePattern {
    Any,
    Range {
        start: Option<u64>,
        end: Option<u64>,
    },
}

impl OplogResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn range(start: Option<u64>, end: Option<u64>) -> Self {
        Self::Range { start, end }
    }
}

impl Subsumes for OplogResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::Range {
                    start: as_,
                    end: ae,
                },
                Self::Range { start: bs, end: be },
            ) => range_subsumes(*as_, *ae, *bs, *be),
            (Self::Range { .. }, Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicOplogResourcePattern {
    Concrete(OplogResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for OplogResourcePattern {
    type Polymorphic = PolymorphicOplogResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum OplogVerb {
    Read,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct OplogClass;

impl PermissionClass for OplogClass {
    type Verb = OplogVerb;
    type Owner = AgentOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = OplogResourcePattern;
    const NAME: &'static str = "oplog";
}

pub type OplogPermissionPattern = ClassPermissionPattern<OplogClass>;
pub type PolymorphicOplogPermissionPattern = PolymorphicClassPermissionPattern<OplogClass>;

impl OplogClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_agent_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Oplog(match verb {
            "*" => OplogPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => OplogPermissionPattern::Verb {
                verb: OplogVerb::Read,
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
        Ok(PolymorphicPermissionPattern::Oplog(match verb {
            "*" => PolymorphicOplogPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "read" => PolymorphicOplogPermissionPattern::Verb {
                verb: OplogVerb::Read,
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

    fn parse_resource(class: &str, resource: &str) -> Result<OplogResourcePattern, CardParseError> {
        if resource == "*" {
            return Ok(OplogResourcePattern::Any);
        }
        let mut start = None;
        let mut end = None;
        for part in resource.split(':') {
            if let Some(value) = part.strip_prefix("start=") {
                start = Some(value.parse().map_err(|_| CardParseError::InvalidResource {
                    class: class.to_string(),
                    resource: resource.to_string(),
                })?);
            } else if let Some(value) = part.strip_prefix("end=") {
                end = Some(value.parse().map_err(|_| CardParseError::InvalidResource {
                    class: class.to_string(),
                    resource: resource.to_string(),
                })?);
            } else {
                return Err(CardParseError::InvalidResource {
                    class: class.to_string(),
                    resource: resource.to_string(),
                });
            }
        }
        Ok(OplogResourcePattern::Range { start, end })
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicOplogResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicOplogResourcePattern::Concrete,
            PolymorphicOplogResourcePattern::Slot,
            PolymorphicOplogResourcePattern::Template,
        )
    }
}
