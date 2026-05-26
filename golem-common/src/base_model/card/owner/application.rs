use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationOwnerPattern {
    AnyApplications,
    AccountApplications {
        account: String,
    },
    Application {
        account: String,
        application: String,
    },
}

impl ApplicationOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*"] => Ok(Self::AnyApplications),
            [account, "*"] => Ok(Self::AccountApplications {
                account: parse_concrete_segment(account)?.to_string(),
            }),
            [account, application] => Ok(Self::Application {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::AnyApplications => None,
            Self::AccountApplications { account } | Self::Application { account, .. } => {
                Some(account)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicApplicationOwnerPattern {
    Concrete(ApplicationOwnerPattern),
    Env,
    Self_,
}

impl OwnerPattern for ApplicationOwnerPattern {
    type Polymorphic = PolymorphicApplicationOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        parse_prefix_owner_slot(value, Self::parse).map(|slot| match slot {
            PrefixOwnerSlot::Concrete(owner) => PolymorphicApplicationOwnerPattern::Concrete(owner),
            PrefixOwnerSlot::Env => PolymorphicApplicationOwnerPattern::Env,
            PrefixOwnerSlot::Self_ => PolymorphicApplicationOwnerPattern::Self_,
        })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyApplications, _) => true,
            (Self::AccountApplications { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b)
            }
            (
                Self::Application {
                    account: aa,
                    application: ap,
                },
                Self::Application {
                    account: ba,
                    application: bp,
                },
            ) => aa == ba && ap == bp,
            (Self::Application { .. }, _) => false,
        }
    }
}
