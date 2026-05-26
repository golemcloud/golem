use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentRecipientPattern {
    Any,
    AccountEnvironments {
        account: String,
    },
    ApplicationEnvironments {
        account: String,
        application: String,
    },
    Environment {
        account: String,
        application: String,
        environment: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentRecipientPattern {
    Concrete(EnvironmentRecipientPattern),
    AccountEnvironments,
    ApplicationEnvironments,
    Environment,
}

impl EnvironmentRecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        <Self as RecipientPattern>::parse(value)
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::AccountEnvironments { account }
            | Self::ApplicationEnvironments { account, .. }
            | Self::Environment { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&str, &str)> {
        match self {
            Self::ApplicationEnvironments {
                account,
                application,
            }
            | Self::Environment {
                account,
                application,
                ..
            } => Some((account, application)),
            Self::Any | Self::AccountEnvironments { .. } => None,
        }
    }
}

impl RecipientPattern for EnvironmentRecipientPattern {
    type Polymorphic = PolymorphicEnvironmentRecipientPattern;

    fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }
        match parse_anchored_segments(value)?.as_slice() {
            [account, "*", "*"] => Ok(Self::AccountEnvironments {
                account: account.to_string(),
            }),
            [account, application, "*"] => Ok(Self::ApplicationEnvironments {
                account: account.to_string(),
                application: application.to_string(),
            }),
            [account, application, environment] => Ok(Self::Environment {
                account: account.to_string(),
                application: application.to_string(),
                environment: environment.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_slot(value)? {
            Some(("?account", rest)) if rest.as_slice() == ["*", "*"] => {
                Ok(PolymorphicEnvironmentRecipientPattern::AccountEnvironments)
            }
            Some(("?app", rest)) if rest.as_slice() == ["*"] => {
                Ok(PolymorphicEnvironmentRecipientPattern::ApplicationEnvironments)
            }
            Some(("?env", rest)) if rest.is_empty() => {
                Ok(PolymorphicEnvironmentRecipientPattern::Environment)
            }
            Some(_) => Err(value.to_string()),
            None => Self::parse(value).map(PolymorphicEnvironmentRecipientPattern::Concrete),
        }
    }

    fn matches_holder(&self, holder: &str) -> bool {
        let Ok(segments) = parse_holder_segments(holder) else {
            return false;
        };
        match self {
            Self::Any => true,
            Self::AccountEnvironments { account } => {
                segments.len() >= 3 && segments.first().is_some_and(|holder| account == holder)
            }
            Self::ApplicationEnvironments {
                account,
                application,
            } => segments.len() >= 3 && account == segments[0] && application == segments[1],
            Self::Environment {
                account,
                application,
                environment,
            } => {
                segments.len() >= 3
                    && account == segments[0]
                    && application == segments[1]
                    && environment == segments[2]
            }
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::AccountEnvironments { account: a }, other) => {
                other.account_part().is_some_and(|account| a == account)
            }
            (
                Self::ApplicationEnvironments {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .application_part()
                .is_some_and(|(ba, bp)| aa == ba && ap == bp),
            (
                Self::Environment {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                Self::Environment {
                    account: ba,
                    application: bp,
                    environment: be,
                },
            ) => aa == ba && ap == bp && ae == be,
            (Self::Environment { .. }, _) => false,
        }
    }
}
