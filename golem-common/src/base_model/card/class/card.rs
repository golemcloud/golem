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
    Any,
    Empty,
    InstallTarget(RecipientPathPattern),
    Slot(SlotVariable),
    Template(ResourceTemplate),
}

impl From<CardResourcePattern> for PolymorphicCardResourcePattern {
    fn from(value: CardResourcePattern) -> Self {
        match value {
            CardResourcePattern::Any => Self::Any,
            CardResourcePattern::Empty => Self::Empty,
            CardResourcePattern::InstallTarget(value) => Self::InstallTarget(value),
        }
    }
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

    fn parse_verb(verb: &str) -> Option<Self::Verb> {
        match verb {
            "derive" => Some(Self::Verb::Derive),
            "revoke" => Some(Self::Verb::Revoke),
            "inspect" => Some(Self::Verb::Inspect),
            "install" => Some(Self::Verb::Install),
            _ => None,
        }
    }

    fn parse_owner(owner: &str) -> Result<Self::Owner, CardParseError> {
        parse_account_owner(Self::NAME, owner)
    }

    fn parse_recipient(recipient: &str) -> Result<Self::Recipient, CardParseError> {
        parse_agent_recipient(recipient)
    }

    fn parse_resource(resource: &str) -> Result<Self::Resource, CardParseError> {
        Self::parse_resource(Self::NAME, resource)
    }

    fn parse_polymorphic_owner(
        owner: &str,
    ) -> Result<<Self::Owner as OwnerPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_account_owner(Self::NAME, owner)
    }

    fn parse_polymorphic_recipient(
        recipient: &str,
    ) -> Result<<Self::Recipient as RecipientPattern>::Polymorphic, CardParseError> {
        parse_polymorphic_agent_recipient(recipient)
    }

    fn parse_polymorphic_resource(
        resource: &str,
    ) -> Result<<Self::Resource as ResourcePattern>::Polymorphic, CardParseError> {
        Self::parse_polymorphic_resource(Self::NAME, resource)
    }

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Card(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Card(pattern)
    }
}

pub type CardPermissionPattern = ClassPermissionPattern<CardClass>;
pub type PolymorphicCardPermissionPattern = PolymorphicClassPermissionPattern<CardClass>;

impl CardClass {
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
            PolymorphicCardResourcePattern::from,
            PolymorphicCardResourcePattern::Slot,
            PolymorphicCardResourcePattern::Template,
        )
    }
}
