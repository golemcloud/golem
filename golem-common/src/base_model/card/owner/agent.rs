use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentOwnerLeafPattern {
    Agent(String),
    AgentTypeWildcard(String),
}

impl AgentOwnerLeafPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = parse_concrete_segment(value)?;
        if let Some(agent_type) = value.strip_suffix("(*)") {
            if !agent_type.is_empty() {
                return Ok(Self::AgentTypeWildcard(agent_type.to_string()));
            }
        }
        Ok(Self::Agent(value.to_string()))
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Agent(left), Self::Agent(right)) => left == right,
            (Self::AgentTypeWildcard(left), Self::Agent(right)) => right
                .strip_prefix(left)
                .is_some_and(|suffix| suffix.starts_with('(') && suffix.ends_with(')')),
            (Self::AgentTypeWildcard(left), Self::AgentTypeWildcard(right)) => left == right,
            (Self::Agent(_), Self::AgentTypeWildcard(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentOwnerPattern {
    AnyAgents,
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
        agent: AgentOwnerLeafPattern,
    },
}

impl AgentOwnerPattern {
    pub fn new(path: impl Into<String>) -> Self {
        Self::parse(&path.into()).expect("invalid owner path")
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*", "*", "*", "*"] => Ok(Self::AnyAgents),
            [account, "*", "*", "*", "*"] => Ok(Self::AccountAgents {
                account: parse_concrete_segment(account)?.to_string(),
            }),
            [account, application, "*", "*", "*"] => Ok(Self::ApplicationAgents {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
            }),
            [account, application, environment, "*", "*"] => Ok(Self::EnvironmentAgents {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
                environment: parse_concrete_segment(environment)?.to_string(),
            }),
            [account, application, environment, component, "*"] => Ok(Self::ComponentAgents {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
                environment: parse_concrete_segment(environment)?.to_string(),
                component: parse_concrete_segment(component)?.to_string(),
            }),
            [account, application, environment, component, agent] => Ok(Self::Agent {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
                environment: parse_concrete_segment(environment)?.to_string(),
                component: parse_concrete_segment(component)?.to_string(),
                agent: AgentOwnerLeafPattern::parse(agent)?,
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::AnyAgents => None,
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
            Self::AnyAgents | Self::AccountAgents { .. } => None,
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
            Self::AnyAgents | Self::AccountAgents { .. } | Self::ApplicationAgents { .. } => None,
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
            Self::AnyAgents
            | Self::AccountAgents { .. }
            | Self::ApplicationAgents { .. }
            | Self::EnvironmentAgents { .. } => None,
        }
    }
}

impl From<String> for AgentOwnerPattern {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}
impl From<&str> for AgentOwnerPattern {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Subsumes for AgentOwnerPattern {
    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyAgents, _) => true,
            (Self::AccountAgents { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b)
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
            ) => aa == ba && ap == bp && ae == be && ac == bc && ag.subsumes(bg),
            (Self::Agent { .. }, _) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAgentOwnerPattern {
    Concrete(AgentOwnerPattern),
    EnvAgents,
    EnvComponentAgents {
        component: String,
    },
    EnvAgent {
        component: String,
        agent: AgentOwnerLeafPattern,
    },
    Self_,
}

impl OwnerPattern for AgentOwnerPattern {
    type Polymorphic = PolymorphicAgentOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_owner_slot(value)? {
            Some(("?env", rest)) if rest.as_slice() == ["*", "*"] => {
                Ok(PolymorphicAgentOwnerPattern::EnvAgents)
            }
            Some(("?env", rest)) if rest.len() == 2 && rest[1] == "*" => {
                Ok(PolymorphicAgentOwnerPattern::EnvComponentAgents {
                    component: parse_concrete_segment(rest[0])?.to_string(),
                })
            }
            Some(("?env", rest)) if rest.len() == 2 => Ok(PolymorphicAgentOwnerPattern::EnvAgent {
                component: parse_concrete_segment(rest[0])?.to_string(),
                agent: AgentOwnerLeafPattern::parse(rest[1])?,
            }),
            Some(("?self", rest)) if rest.is_empty() => Ok(PolymorphicAgentOwnerPattern::Self_),
            Some(_) => Err(value.to_string()),
            None => Self::parse(value).map(PolymorphicAgentOwnerPattern::Concrete),
        }
    }
}
