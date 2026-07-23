// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentOwnerLeafPattern {
    Agent(String),
    AgentTypeWildcard(AgentTypeName),
}

impl AgentOwnerLeafPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = parse_concrete_segment(value)?;
        if let Some(agent_type) = value.strip_suffix("(*)")
            && !agent_type.is_empty()
        {
            return Ok(Self::AgentTypeWildcard(AgentTypeName(
                agent_type.to_string(),
            )));
        }
        Ok(Self::Agent(value.to_string()))
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Agent(left), Self::Agent(right)) => left == right,
            (Self::AgentTypeWildcard(left), Self::Agent(right)) => right
                .strip_prefix(left.as_str())
                .is_some_and(|suffix| suffix.starts_with('(') && suffix.ends_with(')')),
            (Self::AgentTypeWildcard(left), Self::AgentTypeWildcard(right)) => left == right,
            (Self::Agent(_), Self::AgentTypeWildcard(_)) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AgentOwnerPattern {
    AnyAgents,
    AccountAgents {
        account: AccountEmail,
    },
    ApplicationAgents {
        account: AccountEmail,
        application: ApplicationName,
    },
    EnvironmentAgents {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
    },
    ComponentAgents {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
    },
    Agent {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
        agent: AgentOwnerLeafPattern,
    },
}

impl AgentOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*", "*", "*", "*"] => Ok(Self::AnyAgents),
            [account, "*", "*", "*", "*"] => Ok(Self::AccountAgents {
                account: AccountEmail::new(parse_concrete_segment(account)?),
            }),
            [account, application, "*", "*", "*"] => Ok(Self::ApplicationAgents {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
            }),
            [account, application, environment, "*", "*"] => Ok(Self::EnvironmentAgents {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
            }),
            [account, application, environment, component, "*"] => Ok(Self::ComponentAgents {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
                component: ComponentName(parse_concrete_segment(component)?.to_string()),
            }),
            [account, application, environment, component, agent] => Ok(Self::Agent {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
                component: ComponentName(parse_concrete_segment(component)?.to_string()),
                agent: AgentOwnerLeafPattern::parse(agent)?,
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&AccountEmail> {
        match self {
            Self::AnyAgents => None,
            Self::AccountAgents { account }
            | Self::ApplicationAgents { account, .. }
            | Self::EnvironmentAgents { account, .. }
            | Self::ComponentAgents { account, .. }
            | Self::Agent { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&AccountEmail, &ApplicationName)> {
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

    fn environment_part(&self) -> Option<(&AccountEmail, &ApplicationName, &EnvironmentName)> {
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

    fn component_part(
        &self,
    ) -> Option<(
        &AccountEmail,
        &ApplicationName,
        &EnvironmentName,
        &ComponentName,
    )> {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAgentOwnerPattern {
    Concrete(AgentOwnerPattern),
    AccountAgents,
    AccountApplicationAgents {
        application: ApplicationName,
    },
    AccountEnvironmentAgents {
        application: ApplicationName,
        environment: EnvironmentName,
    },
    AccountComponentAgents {
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
    },
    AccountAgent {
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
        agent: AgentOwnerLeafPattern,
    },
    ApplicationAgents,
    ApplicationEnvironmentAgents {
        environment: EnvironmentName,
    },
    ApplicationComponentAgents {
        environment: EnvironmentName,
        component: ComponentName,
    },
    ApplicationAgent {
        environment: EnvironmentName,
        component: ComponentName,
        agent: AgentOwnerLeafPattern,
    },
    EnvAgents,
    EnvComponentAgents {
        component: ComponentName,
    },
    EnvAgent {
        component: ComponentName,
        agent: AgentOwnerLeafPattern,
    },
    ComponentAgents,
    ComponentAgent {
        agent: AgentOwnerLeafPattern,
    },
    Agent,
}

impl OwnerPattern for AgentOwnerPattern {
    type Polymorphic = PolymorphicAgentOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_owner_slot(value)? {
            Some(("?account", rest)) if rest.as_slice() == ["*", "*", "*", "*"] => {
                Ok(PolymorphicAgentOwnerPattern::AccountAgents)
            }
            Some(("?account", rest))
                if rest.len() == 4 && rest[1] == "*" && rest[2] == "*" && rest[3] == "*" =>
            {
                Ok(PolymorphicAgentOwnerPattern::AccountApplicationAgents {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                })
            }
            Some(("?account", rest)) if rest.len() == 4 && rest[2] == "*" && rest[3] == "*" => {
                Ok(PolymorphicAgentOwnerPattern::AccountEnvironmentAgents {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                })
            }
            Some(("?account", rest)) if rest.len() == 4 && rest[3] == "*" => {
                Ok(PolymorphicAgentOwnerPattern::AccountComponentAgents {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                    component: ComponentName(parse_concrete_segment(rest[2])?.to_string()),
                })
            }
            Some(("?account", rest)) if rest.len() == 4 => {
                Ok(PolymorphicAgentOwnerPattern::AccountAgent {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                    component: ComponentName(parse_concrete_segment(rest[2])?.to_string()),
                    agent: AgentOwnerLeafPattern::parse(rest[3])?,
                })
            }
            Some(("?app", rest)) if rest.as_slice() == ["*", "*", "*"] => {
                Ok(PolymorphicAgentOwnerPattern::ApplicationAgents)
            }
            Some(("?app", rest)) if rest.len() == 3 && rest[1] == "*" && rest[2] == "*" => {
                Ok(PolymorphicAgentOwnerPattern::ApplicationEnvironmentAgents {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                })
            }
            Some(("?app", rest)) if rest.len() == 3 && rest[2] == "*" => {
                Ok(PolymorphicAgentOwnerPattern::ApplicationComponentAgents {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                    component: ComponentName(parse_concrete_segment(rest[1])?.to_string()),
                })
            }
            Some(("?app", rest)) if rest.len() == 3 => {
                Ok(PolymorphicAgentOwnerPattern::ApplicationAgent {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                    component: ComponentName(parse_concrete_segment(rest[1])?.to_string()),
                    agent: AgentOwnerLeafPattern::parse(rest[2])?,
                })
            }
            Some(("?env", rest)) if rest.as_slice() == ["*", "*"] => {
                Ok(PolymorphicAgentOwnerPattern::EnvAgents)
            }
            Some(("?env", rest)) if rest.len() == 2 && rest[1] == "*" => {
                Ok(PolymorphicAgentOwnerPattern::EnvComponentAgents {
                    component: ComponentName(parse_concrete_segment(rest[0])?.to_string()),
                })
            }
            Some(("?env", rest)) if rest.len() == 2 => Ok(PolymorphicAgentOwnerPattern::EnvAgent {
                component: ComponentName(parse_concrete_segment(rest[0])?.to_string()),
                agent: AgentOwnerLeafPattern::parse(rest[1])?,
            }),
            Some(("?component", rest)) if rest.as_slice() == ["*"] => {
                Ok(PolymorphicAgentOwnerPattern::ComponentAgents)
            }
            Some(("?component", rest)) if rest.len() == 1 => {
                Ok(PolymorphicAgentOwnerPattern::ComponentAgent {
                    agent: AgentOwnerLeafPattern::parse(rest[0])?,
                })
            }
            Some(("?agent", rest)) if rest.is_empty() => Ok(PolymorphicAgentOwnerPattern::Agent),
            Some(_) => Err(value.to_string()),
            None => Self::parse(value).map(PolymorphicAgentOwnerPattern::Concrete),
        }
    }

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
