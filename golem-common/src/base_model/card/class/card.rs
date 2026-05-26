use super::*;

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
