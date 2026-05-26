use super::*;
use crate::base_model::card::parsing::{
    CardParseError, parse_account_owner, parse_agent_recipient, parse_polymorphic_account_owner,
    parse_polymorphic_agent_recipient, parse_polymorphic_resource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardResourcePattern {
    Any,
    Empty,
    InstallTarget(RecipientPathPattern),
}

impl CardResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn empty() -> Self {
        Self::Empty
    }

    pub fn install_target(target: RecipientPathPattern) -> Self {
        Self::InstallTarget(target)
    }
}

impl Subsumes for CardResourcePattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Empty, Self::Empty) => true,
            (Self::InstallTarget(a), Self::InstallTarget(b)) => a.subsumes(b),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicCardResourcePattern {
    Concrete(CardResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for CardResourcePattern {
    type Polymorphic = PolymorphicCardResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardVerb {
    Derive,
    Revoke,
    Inspect,
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct CardClass;

impl PermissionClass for CardClass {
    type Verb = CardVerb;
    type Owner = AccountOwnerPattern;
    type Recipient = AgentRecipientPattern;
    type Resource = CardResourcePattern;
    const NAME: &'static str = "card";
}

pub type CardPermissionPattern = ClassPermissionPattern<CardClass>;
pub type PolymorphicCardPermissionPattern = PolymorphicClassPermissionPattern<CardClass>;

impl CardClass {
    pub(crate) fn parse_permission(
        owner: &str,
        recipient: &str,
        verb: &str,
        resource: &str,
    ) -> Result<PermissionPattern, CardParseError> {
        let owner = parse_account_owner(Self::NAME, owner)?;
        let recipient = parse_agent_recipient(recipient)?;
        let resource = Self::parse_resource(Self::NAME, resource)?;
        Ok(PermissionPattern::Card(match verb {
            "*" => CardPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "derive" => CardPermissionPattern::Verb {
                verb: CardVerb::Derive,
                owner,
                recipient,
                resource,
            },
            "revoke" => CardPermissionPattern::Verb {
                verb: CardVerb::Revoke,
                owner,
                recipient,
                resource,
            },
            "inspect" => CardPermissionPattern::Verb {
                verb: CardVerb::Inspect,
                owner,
                recipient,
                resource,
            },
            "install" => CardPermissionPattern::Verb {
                verb: CardVerb::Install,
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
        let owner = parse_polymorphic_account_owner(Self::NAME, owner)?;
        let recipient = parse_polymorphic_agent_recipient(recipient)?;
        let resource = Self::parse_polymorphic_resource(Self::NAME, resource)?;
        Ok(PolymorphicPermissionPattern::Card(match verb {
            "*" => PolymorphicCardPermissionPattern::Any {
                owner,
                recipient,
                resource,
            },
            "derive" => PolymorphicCardPermissionPattern::Verb {
                verb: CardVerb::Derive,
                owner,
                recipient,
                resource,
            },
            "revoke" => PolymorphicCardPermissionPattern::Verb {
                verb: CardVerb::Revoke,
                owner,
                recipient,
                resource,
            },
            "inspect" => PolymorphicCardPermissionPattern::Verb {
                verb: CardVerb::Inspect,
                owner,
                recipient,
                resource,
            },
            "install" => PolymorphicCardPermissionPattern::Verb {
                verb: CardVerb::Install,
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

    fn parse_resource(_class: &str, resource: &str) -> Result<CardResourcePattern, CardParseError> {
        if resource == "*" {
            Ok(CardResourcePattern::Any)
        } else if resource.is_empty() {
            Ok(CardResourcePattern::Empty)
        } else {
            Ok(CardResourcePattern::InstallTarget(
                RecipientPathPattern::parse(resource)
                    .map_err(CardParseError::InvalidRecipientPath)?,
            ))
        }
    }

    fn parse_polymorphic_resource(
        class: &str,
        resource: &str,
    ) -> Result<PolymorphicCardResourcePattern, CardParseError> {
        parse_polymorphic_resource(
            class,
            resource,
            Self::parse_resource,
            PolymorphicCardResourcePattern::Concrete,
            PolymorphicCardResourcePattern::Slot,
            PolymorphicCardResourcePattern::Template,
        )
    }
}
