use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EmptyOwnerPattern;

impl EmptyOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            Ok(Self)
        } else {
            Err(value.to_string())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEmptyOwnerPattern {
    Concrete(EmptyOwnerPattern),
}

impl OwnerPattern for EmptyOwnerPattern {
    type Polymorphic = PolymorphicEmptyOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        if value.is_empty() {
            Self::parse(value).map(PolymorphicEmptyOwnerPattern::Concrete)
        } else if split_leftmost_owner_slot(value)?.is_some() {
            Err(value.to_string())
        } else {
            Self::parse(value).map(PolymorphicEmptyOwnerPattern::Concrete)
        }
    }

    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}
