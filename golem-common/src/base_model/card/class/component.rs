use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ComponentResourcePattern;

impl Subsumes for ComponentResourcePattern {
    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicComponentResourcePattern {
    Concrete(ComponentResourcePattern),
    Slot(SlotVariable),
    Template(String),
}

impl ResourcePattern for ComponentResourcePattern {
    type Polymorphic = PolymorphicComponentResourcePattern;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ComponentVerb {
    View,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ComponentClass;

impl PermissionClass for ComponentClass {
    type Verb = ComponentVerb;
    type Owner = ComponentOwnerPattern;
    type Recipient = EnvironmentRecipientPattern;
    type Resource = ComponentResourcePattern;
    const NAME: &'static str = "component";
}

pub type ComponentPermissionPattern = ClassPermissionPattern<ComponentClass>;
pub type PolymorphicComponentPermissionPattern = PolymorphicClassPermissionPattern<ComponentClass>;
