use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AccountOwnerPattern;
use crate::model::card::recipient::{AgentRecipientPattern, RecipientPattern};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardResourcePattern {
    Any,
    InstallTarget(AgentRecipientPattern),
}

impl CardResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn install_target(target: AgentRecipientPattern) -> Self {
        Self::InstallTarget(target)
    }
}

impl ResourcePattern for CardResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(CardResourcePattern::Any)
        } else if resource.is_empty() {
            Err(CardParseError::InvalidResource {
                class: CardClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        } else {
            Ok(CardResourcePattern::InstallTarget(
                AgentRecipientPattern::parse(resource)
                    .map_err(CardParseError::InvalidRecipientPath)?,
            ))
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::InstallTarget(a), Self::InstallTarget(b)) => a.subsumes(b),
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardVerb {
    Derive,
    Revoke,
    Inspect,
    Install,
}
impl VerbPattern for CardVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "derive" => Some(Self::Derive),
            "revoke" => Some(Self::Revoke),
            "inspect" => Some(Self::Inspect),
            "install" => Some(Self::Install),
            _ => None,
        }
    }
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
