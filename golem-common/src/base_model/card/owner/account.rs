use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOwnerPattern {
    Any,
    Account { account: String },
}

impl AccountOwnerPattern {
    pub fn new(path: impl Into<String>) -> Self {
        Self::parse(&path.into()).expect("invalid owner path")
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*"] => Ok(Self::Any),
            [account] => Ok(Self::Account {
                account: parse_concrete_segment(account)?.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }
}

impl From<String> for AccountOwnerPattern {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for AccountOwnerPattern {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAccountOwnerPattern {
    Concrete(AccountOwnerPattern),
    Env,
    Self_,
}

impl OwnerPattern for AccountOwnerPattern {
    type Polymorphic = PolymorphicAccountOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        parse_prefix_owner_slot(value, Self::parse).map(|slot| match slot {
            PrefixOwnerSlot::Concrete(owner) => PolymorphicAccountOwnerPattern::Concrete(owner),
            PrefixOwnerSlot::Env => PolymorphicAccountOwnerPattern::Env,
            PrefixOwnerSlot::Self_ => PolymorphicAccountOwnerPattern::Self_,
        })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account: a }, Self::Account { account: b }) => a == b,
            (Self::Account { .. }, Self::Any) => false,
        }
    }
}
