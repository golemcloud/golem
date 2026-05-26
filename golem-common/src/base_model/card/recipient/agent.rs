use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentRecipientPattern {
    Any,
    AccountAgents {
        account: String,
    },
    ApplicationAgents {
        account: String,
        application: String,
    },
    EnvironmentAgents {
        account: String,
        application: String,
        environment: String,
    },
    ComponentAgents {
        account: String,
        application: String,
        environment: String,
        component: String,
    },
    Agent {
        account: String,
        application: String,
        environment: String,
        component: String,
        agent: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAgentRecipientPattern {
    Concrete(AgentRecipientPattern),
    AccountAgents,
    ApplicationAgents,
    EnvironmentAgents,
    EnvironmentAgent { component: String, agent: String },
    ComponentAgents,
    ComponentAgent { agent: String },
    Self_,
}

impl AgentRecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        <Self as RecipientPattern>::parse(value)
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::AccountAgents { account }
            | Self::ApplicationAgents { account, .. }
            | Self::EnvironmentAgents { account, .. }
            | Self::ComponentAgents { account, .. }
            | Self::Agent { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&str, &str)> {
        match self {
            Self::ApplicationAgents {
                account,
                application,
            }
            | Self::EnvironmentAgents {
                account,
                application,
                ..
            }
            | Self::ComponentAgents {
                account,
                application,
                ..
            }
            | Self::Agent {
                account,
                application,
                ..
            } => Some((account, application)),
            Self::Any | Self::AccountAgents { .. } => None,
        }
    }

    fn environment_part(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::EnvironmentAgents {
                account,
                application,
                environment,
            }
            | Self::ComponentAgents {
                account,
                application,
                environment,
                ..
            }
            | Self::Agent {
                account,
                application,
                environment,
                ..
            } => Some((account, application, environment)),
            Self::Any | Self::AccountAgents { .. } | Self::ApplicationAgents { .. } => None,
        }
    }

    fn component_part(&self) -> Option<(&str, &str, &str, &str)> {
        match self {
            Self::ComponentAgents {
                account,
                application,
                environment,
                component,
            }
            | Self::Agent {
                account,
                application,
                environment,
                component,
                ..
            } => Some((account, application, environment, component)),
            Self::Any
            | Self::AccountAgents { .. }
            | Self::ApplicationAgents { .. }
            | Self::EnvironmentAgents { .. } => None,
        }
    }
}

impl RecipientPattern for AgentRecipientPattern {
    type Polymorphic = PolymorphicAgentRecipientPattern;

    fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }
        match parse_anchored_segments(value)?.as_slice() {
            [account, "*", "*", "*", "*"] => Ok(Self::AccountAgents {
                account: account.to_string(),
            }),
            [account, application, "*", "*", "*"] => Ok(Self::ApplicationAgents {
                account: account.to_string(),
                application: application.to_string(),
            }),
            [account, application, environment, "*", "*"] => Ok(Self::EnvironmentAgents {
                account: account.to_string(),
                application: application.to_string(),
                environment: environment.to_string(),
            }),
            [account, application, environment, component, "*"] => Ok(Self::ComponentAgents {
                account: account.to_string(),
                application: application.to_string(),
                environment: environment.to_string(),
                component: component.to_string(),
            }),
            [account, application, environment, component, agent] => Ok(Self::Agent {
                account: account.to_string(),
                application: application.to_string(),
                environment: environment.to_string(),
                component: component.to_string(),
                agent: agent.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_slot(value)? {
            Some(("?account", rest)) if rest.as_slice() == ["*", "*", "*", "*"] => {
                Ok(PolymorphicAgentRecipientPattern::AccountAgents)
            }
            Some(("?app", rest)) if rest.as_slice() == ["*", "*", "*"] => {
                Ok(PolymorphicAgentRecipientPattern::ApplicationAgents)
            }
            Some(("?env", rest)) if rest.as_slice() == ["*", "*"] => {
                Ok(PolymorphicAgentRecipientPattern::EnvironmentAgents)
            }
            Some(("?env", rest))
                if rest.len() == 2
                    && valid_suffix_segment(rest[0])
                    && valid_suffix_segment(rest[1]) =>
            {
                Ok(PolymorphicAgentRecipientPattern::EnvironmentAgent {
                    component: rest[0].to_string(),
                    agent: rest[1].to_string(),
                })
            }
            Some(("?component", rest)) if rest.as_slice() == ["*"] => {
                Ok(PolymorphicAgentRecipientPattern::ComponentAgents)
            }
            Some(("?component", rest)) if rest.len() == 1 && valid_suffix_segment(rest[0]) => {
                Ok(PolymorphicAgentRecipientPattern::ComponentAgent {
                    agent: rest[0].to_string(),
                })
            }
            Some(("?self", rest)) if rest.is_empty() => Ok(PolymorphicAgentRecipientPattern::Self_),
            Some(_) => Err(value.to_string()),
            None => Self::parse(value).map(PolymorphicAgentRecipientPattern::Concrete),
        }
    }

    fn matches_holder(&self, holder: &str) -> bool {
        let Ok(holder) = Self::parse(holder) else {
            return false;
        };
        self.subsumes(&holder)
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::AccountAgents { account: a }, other) => {
                other.account_part().is_some_and(|account| a == account)
            }
            (
                Self::ApplicationAgents {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .application_part()
                .is_some_and(|(ba, bp)| aa == ba && ap == bp),
            (
                Self::EnvironmentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => other
                .environment_part()
                .is_some_and(|(ba, bp, be)| aa == ba && ap == bp && ae == be),
            (
                Self::ComponentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                },
                other,
            ) => other
                .component_part()
                .is_some_and(|(ba, bp, be, bc)| aa == ba && ap == bp && ae == be && ac == bc),
            (
                Self::Agent {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                    agent: ag,
                },
                Self::Agent {
                    account: ba,
                    application: bp,
                    environment: be,
                    component: bc,
                    agent: bg,
                },
            ) => aa == ba && ap == bp && ae == be && ac == bc && agent_segment_subsumes(ag, bg),
            (Self::Agent { .. }, _) => false,
        }
    }
}
