use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ApplicationResourcePattern;

impl Subsumes for ApplicationResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicApplicationResourcePattern {
    Concrete(ApplicationResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for ApplicationResourcePattern {
    type Polymorphic = PolymorphicApplicationResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationVerb {
    View,
    Create,
    Update,
    Delete,
    Restore,
    MintCredential,
    RotateCredential,
    RevokeCredential,
    ViewCredentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ApplicationClass;

impl PermissionClass for ApplicationClass {
    type Verb = ApplicationVerb;
    type Owner = ApplicationOwnerPattern;
    type Recipient = AccountRecipientPattern;
    type Resource = ApplicationResourcePattern;
    const NAME: &'static str = "application";
}

pub type ApplicationPermissionPattern = ClassPermissionPattern<ApplicationClass>;
pub type PolymorphicApplicationPermissionPattern =
    PolymorphicClassPermissionPattern<ApplicationClass>;
