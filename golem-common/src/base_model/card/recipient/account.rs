use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountRecipientPattern {
    Any,
    Account { account: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAccountRecipientPattern {
    Concrete(AccountRecipientPattern),
    Account,
}

impl AccountRecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        <Self as RecipientPattern>::parse(value)
    }
}

impl Subsumes for AccountRecipientPattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account: a }, Self::Account { account: b }) => a == b,
            (Self::Account { .. }, Self::Any) => false,
        }
    }
}

impl RecipientPattern for AccountRecipientPattern {
    type Polymorphic = PolymorphicAccountRecipientPattern;

    fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }
        match parse_anchored_segments(value)?.as_slice() {
            [account] => Ok(Self::Account {
                account: account.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        if value == "?account" {
            Ok(PolymorphicAccountRecipientPattern::Account)
        } else if split_leftmost_slot(value)?.is_some() {
            Err(value.to_string())
        } else {
            Self::parse(value).map(PolymorphicAccountRecipientPattern::Concrete)
        }
    }

    fn matches_holder(&self, holder: &str) -> bool {
        let Ok(segments) = parse_holder_segments(holder) else {
            return false;
        };
        match self {
            Self::Any => true,
            Self::Account { account } => segments.first().is_some_and(|holder| account == holder),
        }
    }
}
