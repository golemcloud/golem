use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemResourcePattern;

impl Subsumes for SystemResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicSystemResourcePattern {
    Concrete(SystemResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for SystemResourcePattern {
    type Polymorphic = PolymorphicSystemResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum SystemVerb {
    CreateAccount,
    ViewDefaultPlan,
    ViewAccountSummariesReport,
    ViewAccountCountsReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct SystemClass;

impl PermissionClass for SystemClass {
    type Verb = SystemVerb;
    type Owner = EmptyOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = SystemResourcePattern;
    const NAME: &'static str = "system";
}

pub type SystemPermissionPattern = ClassPermissionPattern<SystemClass>;
pub type PolymorphicSystemPermissionPattern = PolymorphicClassPermissionPattern<SystemClass>;
