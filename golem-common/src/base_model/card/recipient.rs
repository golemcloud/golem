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
pub enum RecipientPattern {
    Any,
    Account {
        account: String,
    },
    Environment {
        account: String,
        application: String,
        environment: String,
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
pub enum PolymorphicRecipientPattern {
    Concrete(RecipientPattern),
    Account,
    Environment(PolymorphicEnvironmentRecipientPattern),
    Agent(PolymorphicAgentRecipientPattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentRecipientPattern {
    AccountEnvironments,
    ApplicationEnvironments,
    Environment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAgentRecipientPattern {
    AccountAgents,
    ApplicationAgents,
    EnvironmentAgents,
    EnvironmentAgent { component: String, agent: String },
    ComponentAgents,
    ComponentAgent { agent: String },
    Self_,
}

impl RecipientPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }

        match parse_anchored_segments(value)?.as_slice() {
            [account] => Ok(Self::Account {
                account: concrete_segment(account)?.to_string(),
            }),
            [account, application, environment] => Ok(Self::Environment {
                account: concrete_segment(account)?.to_string(),
                application: application_segment(application)?.to_string(),
                environment: application_segment(environment)?.to_string(),
            }),
            [account, application, environment, component, agent] => Ok(Self::Agent {
                account: concrete_segment(account)?.to_string(),
                application: application_segment(application)?.to_string(),
                environment: application_segment(environment)?.to_string(),
                component: application_segment(component)?.to_string(),
                agent: agent_segment(agent)?.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    pub fn matches_holder(&self, holder: &str) -> bool {
        let Ok(holder) = Self::parse_holder(holder) else {
            return false;
        };
        self.subsumes(&holder)
    }

    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account: a }, other) => {
                other.account_part().is_some_and(|account| a == account)
            }
            (
                Self::Environment {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => other.environment_part().is_some_and(|(ba, bp, be)| {
                aa == ba && segment_subsumes(ap, bp) && segment_subsumes(ae, be)
            }),
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
            ) => {
                aa == ba
                    && segment_subsumes(ap, bp)
                    && segment_subsumes(ae, be)
                    && segment_subsumes(ac, bc)
                    && agent_segment_subsumes(ag, bg)
            }
            (Self::Agent { .. }, _) => false,
        }
    }

    fn parse_holder(value: &str) -> Result<Self, String> {
        let segments = parse_holder_segments(value)?;
        if segments.contains(&"*") {
            return Err(value.to_string());
        }
        Self::parse(value)
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::Account { account }
            | Self::Environment { account, .. }
            | Self::Agent { account, .. } => Some(account),
        }
    }

    fn environment_part(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::Environment {
                account,
                application,
                environment,
            }
            | Self::Agent {
                account,
                application,
                environment,
                ..
            } => Some((account, application, environment)),
            Self::Any | Self::Account { .. } => None,
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
                        component: rest[0].to_string(),
                        agent: rest[1].to_string(),
                    },
                ))
            }
            Some(("?component", rest)) if rest.as_slice() == ["*"] => Ok(Self::Agent(
                PolymorphicAgentRecipientPattern::ComponentAgents,
            )),
            Some(("?component", rest)) if rest.len() == 1 && valid_suffix_segment(rest[0]) => Ok(
                Self::Agent(PolymorphicAgentRecipientPattern::ComponentAgent {
                    agent: rest[0].to_string(),
                }),
            ),
            Some(("?self", rest)) if rest.is_empty() => {
                Ok(Self::Agent(PolymorphicAgentRecipientPattern::Self_))
            }
            Some(_) => Err(value.to_string()),
            None => RecipientPattern::parse(value).map(Self::Concrete),
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

fn parse_holder_segments(value: &str) -> Result<Vec<&str>, String> {
    let segments = parse_anchored_segments(value)?;
    match segments.len() {
        1 | 3 | 5 => Ok(segments),
        _ => Err(value.to_string()),
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

fn application_segment(segment: &str) -> Result<&str, String> {
    if segment.is_empty() || segment.contains('?') {
        Err(segment.to_string())
    } else {
        Ok(segment)
    }
}

fn agent_segment(segment: &str) -> Result<&str, String> {
    if segment.is_empty() || segment.contains('?') {
        Err(segment.to_string())
    } else {
        Ok(segment)
    }
}

fn contains_slot_reference(value: &str) -> bool {
    value
        .split('/')
        .any(|segment| segment.starts_with('?') || segment.contains("/?"))
}

fn segment_subsumes(left: &str, right: &str) -> bool {
    left == "*" || left == right
}

fn agent_segment_subsumes(left: &str, right: &str) -> bool {
    if left == right || left == "*" {
        return true;
    }
    let Some(agent_type) = left.strip_suffix("(*)") else {
        return false;
    };
    right
        .strip_prefix(agent_type)
        .is_some_and(|suffix| suffix.starts_with('(') && suffix.ends_with(')'))
}
