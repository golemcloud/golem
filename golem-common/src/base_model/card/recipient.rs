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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RecipientPathPattern {
    Any,
    Account {
        account: String,
    },
    AccountEnvironments {
        account: String,
    },
    ApplicationEnvironments {
        account: String,
        application: String,
    },
    AccountAgents {
        account: String,
    },
    ApplicationAgents {
        account: String,
        application: String,
    },
    Environment {
        account: String,
        application: String,
        environment: String,
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

impl RecipientPathPattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn account(account: impl Into<String>) -> Self {
        Self::Account {
            account: account.into(),
        }
    }

    pub fn account_environments(account: impl Into<String>) -> Self {
        Self::AccountEnvironments {
            account: account.into(),
        }
    }

    pub fn application_environments(
        account: impl Into<String>,
        application: impl Into<String>,
    ) -> Self {
        Self::ApplicationEnvironments {
            account: account.into(),
            application: application.into(),
        }
    }

    pub fn account_agents(account: impl Into<String>) -> Self {
        Self::AccountAgents {
            account: account.into(),
        }
    }

    pub fn application_agents(account: impl Into<String>, application: impl Into<String>) -> Self {
        Self::ApplicationAgents {
            account: account.into(),
            application: application.into(),
        }
    }

    pub fn environment(
        account: impl Into<String>,
        application: impl Into<String>,
        environment: impl Into<String>,
    ) -> Self {
        Self::Environment {
            account: account.into(),
            application: application.into(),
            environment: environment.into(),
        }
    }

    pub fn environment_agents(
        account: impl Into<String>,
        application: impl Into<String>,
        environment: impl Into<String>,
    ) -> Self {
        Self::EnvironmentAgents {
            account: account.into(),
            application: application.into(),
            environment: environment.into(),
        }
    }

    pub fn component_agents(
        account: impl Into<String>,
        application: impl Into<String>,
        environment: impl Into<String>,
        component: impl Into<String>,
    ) -> Self {
        Self::ComponentAgents {
            account: account.into(),
            application: application.into(),
            environment: environment.into(),
            component: component.into(),
        }
    }

    pub fn agent(
        account: impl Into<String>,
        application: impl Into<String>,
        environment: impl Into<String>,
        component: impl Into<String>,
        agent: impl Into<String>,
    ) -> Self {
        Self::Agent {
            account: account.into(),
            application: application.into(),
            environment: environment.into(),
            component: component.into(),
            agent: agent.into(),
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }

        let segments = parse_exact_segments(value)?;

        match segments.as_slice() {
            [account] => Ok(Self::Account {
                account: account.to_string(),
            }),
            [account, "*", "*"] => Ok(Self::AccountEnvironments {
                account: account.to_string(),
            }),
            [account, application, "*"] => Ok(Self::ApplicationEnvironments {
                account: account.to_string(),
                application: application.to_string(),
            }),
            [account, "*", "*", "*", "*"] => Ok(Self::AccountAgents {
                account: account.to_string(),
            }),
            [account, application, "*", "*", "*"] => Ok(Self::ApplicationAgents {
                account: account.to_string(),
                application: application.to_string(),
            }),
            [account, application, environment] => Ok(Self::Environment {
                account: account.to_string(),
                application: application.to_string(),
                environment: environment.to_string(),
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

    pub fn concrete_prefix(&self) -> Vec<&str> {
        match self {
            Self::Any => Vec::new(),
            Self::Account { account }
            | Self::AccountEnvironments { account }
            | Self::AccountAgents { account } => {
                vec![account.as_str()]
            }
            Self::ApplicationEnvironments {
                account,
                application,
            }
            | Self::ApplicationAgents {
                account,
                application,
            } => vec![account.as_str(), application.as_str()],
            Self::Environment {
                account,
                application,
                environment,
            }
            | Self::EnvironmentAgents {
                account,
                application,
                environment,
            } => vec![account.as_str(), application.as_str(), environment.as_str()],
            Self::ComponentAgents {
                account,
                application,
                environment,
                component,
            } => vec![
                account.as_str(),
                application.as_str(),
                environment.as_str(),
                component.as_str(),
            ],
            Self::Agent {
                account,
                application,
                environment,
                component,
                agent,
            } => vec![
                account.as_str(),
                application.as_str(),
                environment.as_str(),
                component.as_str(),
                agent.as_str(),
            ],
        }
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account }, other) => {
                other.account_part().is_some_and(|b| account == b)
            }
            (Self::AccountEnvironments { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b) && other.is_environment_or_deeper()
            }
            (
                Self::ApplicationEnvironments {
                    account: aa,
                    application: ap,
                },
                other,
            ) => {
                other
                    .application()
                    .is_some_and(|(ba, bp)| aa == ba && ap == bp)
                    && other.is_environment_or_deeper()
            }
            (Self::AccountAgents { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b) && other.is_agent_scope()
            }
            (
                Self::ApplicationAgents {
                    account: aa,
                    application: ap,
                },
                other,
            ) => {
                other
                    .application()
                    .is_some_and(|(ba, bp)| aa == ba && ap == bp)
                    && other.is_agent_scope()
            }
            (
                Self::Environment {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            )
            | (
                Self::EnvironmentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => {
                other
                    .environment_part()
                    .is_some_and(|(ba, bp, be)| aa == ba && ap == bp && ae == be)
                    && (!matches!(self, Self::EnvironmentAgents { .. }) || other.is_agent_scope())
            }
            (
                Self::ComponentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                },
                other,
            ) => {
                other
                    .component()
                    .is_some_and(|(ba, bp, be, bc)| aa == ba && ap == bp && ae == be && ac == bc)
                    && other.is_agent_scope()
            }
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
            ) => aa == ba && ap == bp && ae == be && ac == bc && ag == bg,
            (Self::Agent { .. }, _) => false,
        }
    }

    pub fn matches_holder(&self, holder: &Self) -> bool {
        self.subsumes(holder)
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::Account { account }
            | Self::AccountEnvironments { account }
            | Self::AccountAgents { account }
            | Self::ApplicationEnvironments { account, .. }
            | Self::ApplicationAgents { account, .. }
            | Self::Environment { account, .. }
            | Self::EnvironmentAgents { account, .. }
            | Self::ComponentAgents { account, .. }
            | Self::Agent { account, .. } => Some(account),
        }
    }

    fn application(&self) -> Option<(&str, &str)> {
        match self {
            Self::ApplicationEnvironments {
                account,
                application,
            }
            | Self::ApplicationAgents {
                account,
                application,
            }
            | Self::Environment {
                account,
                application,
                ..
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
            Self::Any
            | Self::Account { .. }
            | Self::AccountEnvironments { .. }
            | Self::AccountAgents { .. } => None,
        }
    }

    fn environment_part(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::Environment {
                account,
                application,
                environment,
            }
            | Self::EnvironmentAgents {
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
            Self::Any
            | Self::Account { .. }
            | Self::AccountEnvironments { .. }
            | Self::ApplicationEnvironments { .. }
            | Self::AccountAgents { .. }
            | Self::ApplicationAgents { .. } => None,
        }
    }

    fn component(&self) -> Option<(&str, &str, &str, &str)> {
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
            | Self::Account { .. }
            | Self::AccountEnvironments { .. }
            | Self::ApplicationEnvironments { .. }
            | Self::AccountAgents { .. }
            | Self::ApplicationAgents { .. }
            | Self::Environment { .. }
            | Self::EnvironmentAgents { .. } => None,
        }
    }

    fn is_environment_or_deeper(&self) -> bool {
        matches!(
            self,
            Self::AccountEnvironments { .. }
                | Self::ApplicationEnvironments { .. }
                | Self::Environment { .. }
                | Self::AccountAgents { .. }
                | Self::ApplicationAgents { .. }
                | Self::EnvironmentAgents { .. }
                | Self::ComponentAgents { .. }
                | Self::Agent { .. }
        )
    }

    fn is_agent_scope(&self) -> bool {
        match self {
            Self::AccountAgents { .. }
            | Self::ApplicationAgents { .. }
            | Self::EnvironmentAgents { .. }
            | Self::ComponentAgents { .. }
            | Self::Agent { .. } => true,
            Self::Any
            | Self::Account { .. }
            | Self::AccountEnvironments { .. }
            | Self::ApplicationEnvironments { .. }
            | Self::Environment { .. } => false,
        }
    }
}

fn parse_exact_segments(value: &str) -> Result<Vec<&str>, String> {
    if value.is_empty() {
        return Err(value.to_string());
    }

    let segments = value.split('/').collect::<Vec<_>>();
    if segments.first() == Some(&"*")
        || segments.iter().any(|s| s.is_empty())
        || has_concrete_after_wildcard_segment(&segments)
    {
        Err(value.to_string())
    } else {
        Ok(segments)
    }
}

fn has_concrete_after_wildcard_segment(segments: &[&str]) -> bool {
    let mut seen_wildcard = false;
    for segment in segments {
        match *segment {
            "*" => seen_wildcard = true,
            _ if seen_wildcard => return true,
            _ => {}
        }
    }
    false
}
