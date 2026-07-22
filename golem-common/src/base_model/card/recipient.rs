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

use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RecipientPattern {
    Any,
    Account {
        account: AccountEmail,
    },
    AccountEnvironments {
        account: AccountEmail,
    },
    ApplicationEnvironments {
        account: AccountEmail,
        application: ApplicationName,
    },
    Environment {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
    },
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
        agent_type: AgentTypeName,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicRecipientPattern {
    Concrete(RecipientPattern),
    Account,
    Environment(PolymorphicEnvironmentRecipientPattern),
    Agent(PolymorphicAgentRecipientPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentRecipientPattern {
    AccountEnvironments,
    ApplicationEnvironments,
    Environment,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAgentRecipientPattern {
    AccountAgents,
    ApplicationAgents,
    EnvironmentAgents,
    EnvironmentAgent {
        component: ComponentName,
        agent_type: AgentTypeName,
    },
    ComponentAgents,
    ComponentAgent {
        agent_type: AgentTypeName,
    },
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientMonomorphizationContext {
    pub account: AccountEmail,
    pub application: ApplicationName,
    pub environment: EnvironmentName,
    pub component: ComponentName,
    pub agent_type: AgentTypeName,
}

impl RecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }

        match parse_anchored_segments(value)?.as_slice() {
            [account] => Ok(Self::Account {
                account: AccountEmail::new(concrete_segment(account)?),
            }),
            [account, "*", "*"] => Ok(Self::AccountEnvironments {
                account: AccountEmail::new(concrete_segment(account)?),
            }),
            [account, application, "*"] => Ok(Self::ApplicationEnvironments {
                account: AccountEmail::new(concrete_segment(account)?),
                application: ApplicationName::try_from(concrete_segment(application)?)?,
            }),
            [account, application, environment] => Ok(Self::Environment {
                account: AccountEmail::new(concrete_segment(account)?),
                application: ApplicationName::try_from(concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(concrete_segment(environment)?)?,
            }),
            [account, "*", "*", "*", "*"] => Ok(Self::AccountAgents {
                account: AccountEmail::new(concrete_segment(account)?),
            }),
            [account, application, "*", "*", "*"] => Ok(Self::ApplicationAgents {
                account: AccountEmail::new(concrete_segment(account)?),
                application: ApplicationName::try_from(concrete_segment(application)?)?,
            }),
            [account, application, environment, "*", "*"] => Ok(Self::EnvironmentAgents {
                account: AccountEmail::new(concrete_segment(account)?),
                application: ApplicationName::try_from(concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(concrete_segment(environment)?)?,
            }),
            [account, application, environment, component, "*"] => Ok(Self::ComponentAgents {
                account: AccountEmail::new(concrete_segment(account)?),
                application: ApplicationName::try_from(concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(concrete_segment(environment)?)?,
                component: ComponentName(concrete_segment(component)?.to_string()),
            }),
            [account, application, environment, component, agent_type] => Ok(Self::Agent {
                account: AccountEmail::new(concrete_segment(account)?),
                application: ApplicationName::try_from(concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(concrete_segment(environment)?)?,
                component: ComponentName(concrete_segment(component)?.to_string()),
                agent_type: AgentTypeName(concrete_segment(agent_type)?.to_string()),
            }),
            _ => Err(value.to_string()),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Any => "*".to_string(),
            Self::Account { account } => account.as_str().to_string(),
            Self::AccountEnvironments { account } => format!("{}/*/*", account.as_str()),
            Self::ApplicationEnvironments {
                account,
                application,
            } => format!("{}/{}/*", account.as_str(), application.0),
            Self::Environment {
                account,
                application,
                environment,
            } => format!("{}/{}/{}", account.as_str(), application.0, environment.0),
            Self::AccountAgents { account } => format!("{}/*/*/*/*", account.as_str()),
            Self::ApplicationAgents {
                account,
                application,
            } => format!("{}/{}/*/*/*", account.as_str(), application.0),
            Self::EnvironmentAgents {
                account,
                application,
                environment,
            } => format!(
                "{}/{}/{}/*/*",
                account.as_str(),
                application.0,
                environment.0
            ),
            Self::ComponentAgents {
                account,
                application,
                environment,
                component,
            } => format!(
                "{}/{}/{}/{}/*",
                account.as_str(),
                application.0,
                environment.0,
                component.0
            ),
            Self::Agent {
                account,
                application,
                environment,
                component,
                agent_type,
            } => format!(
                "{}/{}/{}/{}/{}",
                account.as_str(),
                application.0,
                environment.0,
                component.0,
                agent_type.0
            ),
        }
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account: a }, other) => {
                other.account_part().is_some_and(|account| a == account)
            }
            (Self::AccountEnvironments { account: a }, other) => other
                .environment_scope_part()
                .is_some_and(|(account, _, _)| a == account),
            (
                Self::ApplicationEnvironments {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .environment_scope_part()
                .is_some_and(|(ba, bp, _)| aa == ba && bp == Some(ap)),
            (
                Self::Environment {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => other
                .environment_scope_part()
                .is_some_and(|(ba, bp, be)| aa == ba && bp == Some(ap) && be == Some(ae)),
            (Self::AccountAgents { account: a }, other) => other
                .agent_scope_part()
                .is_some_and(|(account, _, _, _, _)| a == account),
            (
                Self::ApplicationAgents {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .agent_scope_part()
                .is_some_and(|(ba, bp, _, _, _)| aa == ba && bp == Some(ap)),
            (
                Self::EnvironmentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => other
                .agent_scope_part()
                .is_some_and(|(ba, bp, be, _, _)| aa == ba && bp == Some(ap) && be == Some(ae)),
            (
                Self::ComponentAgents {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                },
                other,
            ) => other.agent_scope_part().is_some_and(|(ba, bp, be, bc, _)| {
                aa == ba && bp == Some(ap) && be == Some(ae) && bc == Some(ac)
            }),
            (
                Self::Agent {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                    agent_type: at,
                },
                other,
            ) => other
                .agent_scope_part()
                .is_some_and(|(ba, bp, be, bc, bt)| {
                    aa == ba && bp == Some(ap) && be == Some(ae) && bc == Some(ac) && bt == Some(at)
                }),
        }
    }

    fn account_part(&self) -> Option<&AccountEmail> {
        match self {
            Self::Account { account }
            | Self::AccountEnvironments { account }
            | Self::ApplicationEnvironments { account, .. }
            | Self::Environment { account, .. }
            | Self::AccountAgents { account }
            | Self::ApplicationAgents { account, .. }
            | Self::EnvironmentAgents { account, .. }
            | Self::ComponentAgents { account, .. }
            | Self::Agent { account, .. } => Some(account),
            Self::Any => None,
        }
    }

    fn environment_scope_part(
        &self,
    ) -> Option<(
        &AccountEmail,
        Option<&ApplicationName>,
        Option<&EnvironmentName>,
    )> {
        match self {
            Self::AccountEnvironments { account } => Some((account, None, None)),
            Self::ApplicationEnvironments {
                account,
                application,
            } => Some((account, Some(application), None)),
            Self::Environment {
                account,
                application,
                environment,
            } => Some((account, Some(application), Some(environment))),
            Self::AccountAgents { account } => Some((account, None, None)),
            Self::ApplicationAgents {
                account,
                application,
            } => Some((account, Some(application), None)),
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
            } => Some((account, Some(application), Some(environment))),
            Self::Any | Self::Account { .. } => None,
        }
    }

    #[allow(clippy::type_complexity)]
    fn agent_scope_part(
        &self,
    ) -> Option<(
        &AccountEmail,
        Option<&ApplicationName>,
        Option<&EnvironmentName>,
        Option<&ComponentName>,
        Option<&AgentTypeName>,
    )> {
        match self {
            Self::AccountAgents { account } => Some((account, None, None, None, None)),
            Self::ApplicationAgents {
                account,
                application,
            } => Some((account, Some(application), None, None, None)),
            Self::EnvironmentAgents {
                account,
                application,
                environment,
            } => Some((account, Some(application), Some(environment), None, None)),
            Self::ComponentAgents {
                account,
                application,
                environment,
                component,
            } => Some((
                account,
                Some(application),
                Some(environment),
                Some(component),
                None,
            )),
            Self::Agent {
                account,
                application,
                environment,
                component,
                agent_type,
            } => Some((
                account,
                Some(application),
                Some(environment),
                Some(component),
                Some(agent_type),
            )),
            Self::Any
            | Self::Account { .. }
            | Self::AccountEnvironments { .. }
            | Self::ApplicationEnvironments { .. }
            | Self::Environment { .. } => None,
        }
    }
}

impl PolymorphicRecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match split_leftmost_slot(value)? {
            Some(("?account", rest)) if rest.is_empty() => Ok(Self::Account),
            Some(("?account", rest)) if rest.as_slice() == ["*", "*"] => Ok(Self::Environment(
                PolymorphicEnvironmentRecipientPattern::AccountEnvironments,
            )),
            Some(("?account", rest)) if rest.as_slice() == ["*", "*", "*", "*"] => {
                Ok(Self::Agent(PolymorphicAgentRecipientPattern::AccountAgents))
            }
            Some(("?app", rest)) if rest.as_slice() == ["*"] => Ok(Self::Environment(
                PolymorphicEnvironmentRecipientPattern::ApplicationEnvironments,
            )),
            Some(("?app", rest)) if rest.as_slice() == ["*", "*", "*"] => Ok(Self::Agent(
                PolymorphicAgentRecipientPattern::ApplicationAgents,
            )),
            Some(("?env", rest)) if rest.is_empty() => Ok(Self::Environment(
                PolymorphicEnvironmentRecipientPattern::Environment,
            )),
            Some(("?env", rest)) if rest.as_slice() == ["*", "*"] => Ok(Self::Agent(
                PolymorphicAgentRecipientPattern::EnvironmentAgents,
            )),
            Some(("?env", rest))
                if rest.len() == 2
                    && valid_suffix_segment(rest[0])
                    && valid_suffix_segment(rest[1]) =>
            {
                Ok(Self::Agent(
                    PolymorphicAgentRecipientPattern::EnvironmentAgent {
                        component: ComponentName(rest[0].to_string()),
                        agent_type: AgentTypeName(rest[1].to_string()),
                    },
                ))
            }
            Some(("?component", rest)) if rest.as_slice() == ["*"] => Ok(Self::Agent(
                PolymorphicAgentRecipientPattern::ComponentAgents,
            )),
            Some(("?component", rest)) if rest.len() == 1 && valid_suffix_segment(rest[0]) => Ok(
                Self::Agent(PolymorphicAgentRecipientPattern::ComponentAgent {
                    agent_type: AgentTypeName(rest[0].to_string()),
                }),
            ),
            Some(("?agent", rest)) if rest.is_empty() => {
                Ok(Self::Agent(PolymorphicAgentRecipientPattern::Agent))
            }
            Some(_) => Err(value.to_string()),
            None => RecipientPattern::parse(value).map(Self::Concrete),
        }
    }

    pub fn monomorphize(&self, context: &RecipientMonomorphizationContext) -> RecipientPattern {
        match self {
            Self::Concrete(recipient) => recipient.clone(),
            Self::Account => RecipientPattern::Account {
                account: context.account.clone(),
            },
            Self::Environment(pattern) => match pattern {
                PolymorphicEnvironmentRecipientPattern::AccountEnvironments => {
                    RecipientPattern::AccountEnvironments {
                        account: context.account.clone(),
                    }
                }
                PolymorphicEnvironmentRecipientPattern::ApplicationEnvironments => {
                    RecipientPattern::ApplicationEnvironments {
                        account: context.account.clone(),
                        application: context.application.clone(),
                    }
                }
                PolymorphicEnvironmentRecipientPattern::Environment => {
                    RecipientPattern::Environment {
                        account: context.account.clone(),
                        application: context.application.clone(),
                        environment: context.environment.clone(),
                    }
                }
            },
            Self::Agent(pattern) => match pattern {
                PolymorphicAgentRecipientPattern::AccountAgents => {
                    RecipientPattern::AccountAgents {
                        account: context.account.clone(),
                    }
                }
                PolymorphicAgentRecipientPattern::ApplicationAgents => {
                    RecipientPattern::ApplicationAgents {
                        account: context.account.clone(),
                        application: context.application.clone(),
                    }
                }
                PolymorphicAgentRecipientPattern::EnvironmentAgents => {
                    RecipientPattern::EnvironmentAgents {
                        account: context.account.clone(),
                        application: context.application.clone(),
                        environment: context.environment.clone(),
                    }
                }
                PolymorphicAgentRecipientPattern::EnvironmentAgent {
                    component,
                    agent_type,
                } => RecipientPattern::Agent {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: context.environment.clone(),
                    component: component.clone(),
                    agent_type: agent_type.clone(),
                },
                PolymorphicAgentRecipientPattern::ComponentAgents => {
                    RecipientPattern::ComponentAgents {
                        account: context.account.clone(),
                        application: context.application.clone(),
                        environment: context.environment.clone(),
                        component: context.component.clone(),
                    }
                }
                PolymorphicAgentRecipientPattern::ComponentAgent { agent_type } => {
                    RecipientPattern::Agent {
                        account: context.account.clone(),
                        application: context.application.clone(),
                        environment: context.environment.clone(),
                        component: context.component.clone(),
                        agent_type: agent_type.clone(),
                    }
                }
                PolymorphicAgentRecipientPattern::Agent => RecipientPattern::Agent {
                    account: context.account.clone(),
                    application: context.application.clone(),
                    environment: context.environment.clone(),
                    component: context.component.clone(),
                    agent_type: context.agent_type.clone(),
                },
            },
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Concrete(recipient) => recipient.render(),
            Self::Account => "?account".to_string(),
            Self::Environment(pattern) => match pattern {
                PolymorphicEnvironmentRecipientPattern::AccountEnvironments => {
                    "?account/*/*".to_string()
                }
                PolymorphicEnvironmentRecipientPattern::ApplicationEnvironments => {
                    "?app/*".to_string()
                }
                PolymorphicEnvironmentRecipientPattern::Environment => "?env".to_string(),
            },
            Self::Agent(pattern) => match pattern {
                PolymorphicAgentRecipientPattern::AccountAgents => "?account/*/*/*/*".to_string(),
                PolymorphicAgentRecipientPattern::ApplicationAgents => "?app/*/*/*".to_string(),
                PolymorphicAgentRecipientPattern::EnvironmentAgents => "?env/*/*".to_string(),
                PolymorphicAgentRecipientPattern::EnvironmentAgent {
                    component,
                    agent_type,
                } => format!("?env/{}/{}", component.0, agent_type.0),
                PolymorphicAgentRecipientPattern::ComponentAgents => "?component/*".to_string(),
                PolymorphicAgentRecipientPattern::ComponentAgent { agent_type } => {
                    format!("?component/{}", agent_type.0)
                }
                PolymorphicAgentRecipientPattern::Agent => "?agent".to_string(),
            },
        }
    }
}

fn parse_anchored_segments(value: &str) -> Result<Vec<&str>, String> {
    if value.is_empty() {
        return Err(value.to_string());
    }

    let segments = value.split('/').collect::<Vec<_>>();
    if segments.first() == Some(&"*")
        || segments.iter().any(|segment| segment.is_empty())
        || has_segment_after_wildcard_segment(&segments)
    {
        Err(value.to_string())
    } else {
        Ok(segments)
    }
}

fn has_segment_after_wildcard_segment(segments: &[&str]) -> bool {
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

fn split_leftmost_slot(value: &str) -> Result<Option<(&str, Vec<&str>)>, String> {
    let segments = value.split('/').collect::<Vec<_>>();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(value.to_string());
    }

    let Some((first, rest)) = segments.split_first() else {
        return Err(value.to_string());
    };

    if first.starts_with('?') {
        if rest.iter().any(|segment| segment.contains('?'))
            || has_segment_after_wildcard_segment(rest)
        {
            Err(value.to_string())
        } else {
            Ok(Some((*first, rest.to_vec())))
        }
    } else if contains_slot_reference(value) {
        Err(value.to_string())
    } else {
        Ok(None)
    }
}

fn valid_suffix_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "*" && !segment.contains('?')
}

fn concrete_segment(segment: &str) -> Result<&str, String> {
    if segment.is_empty() || segment.contains('*') || segment.contains('?') {
        Err(segment.to_string())
    } else {
        Ok(segment)
    }
}

fn contains_slot_reference(value: &str) -> bool {
    value.split('/').any(|segment| segment.starts_with('?'))
}
