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
pub enum PathSegmentPattern {
    Any,
    Exact(String),
}

impl PathSegmentPattern {
    pub fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum EnvironmentPathPattern {
    Any,
    Environment {
        account: PathSegmentPattern,
        application: PathSegmentPattern,
        environment: PathSegmentPattern,
    },
}

impl EnvironmentPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }

        let segments = parse_path_segments(value)?;
        match segments.as_slice() {
            [account, application, environment] => Ok(Self::Environment {
                account: account.clone(),
                application: application.clone(),
                environment: environment.clone(),
            }),
            _ => Err(value.to_string()),
        }
    }

    pub fn segments(&self) -> Vec<PathSegmentPattern> {
        match self {
            Self::Any => vec![
                PathSegmentPattern::Any,
                PathSegmentPattern::Any,
                PathSegmentPattern::Any,
            ],
            Self::Environment {
                account,
                application,
                environment,
            } => vec![account.clone(), application.clone(), environment.clone()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum RecipientPathPattern {
    Any,
    Account {
        account: PathSegmentPattern,
    },
    Environment {
        account: PathSegmentPattern,
        application: PathSegmentPattern,
        environment: PathSegmentPattern,
    },
    Agent {
        account: PathSegmentPattern,
        application: PathSegmentPattern,
        environment: PathSegmentPattern,
        component: PathSegmentPattern,
        agent: PathSegmentPattern,
    },
}

impl RecipientPathPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value == "*" {
            return Ok(Self::Any);
        }

        let segments = parse_path_segments(value)?;

        match segments.as_slice() {
            [account]
                if is_concrete_root(account)
                    && !matches!(account, PathSegmentPattern::Exact(value) if value.contains('(')) =>
            {
                Ok(Self::Account {
                    account: account.clone(),
                })
            }
            [account, application, environment] if is_concrete_root(account) => {
                Ok(Self::Environment {
                    account: account.clone(),
                    application: application.clone(),
                    environment: environment.clone(),
                })
            }
            [account, application, environment, component, agent] if is_concrete_root(account) => {
                Ok(Self::Agent {
                    account: account.clone(),
                    application: application.clone(),
                    environment: environment.clone(),
                    component: component.clone(),
                    agent: agent.clone(),
                })
            }
            _ => Err(value.to_string()),
        }
    }

    pub fn segments(&self) -> Vec<PathSegmentPattern> {
        match self {
            Self::Any => vec![
                PathSegmentPattern::Any,
                PathSegmentPattern::Any,
                PathSegmentPattern::Any,
                PathSegmentPattern::Any,
                PathSegmentPattern::Any,
            ],
            Self::Account { account } => vec![account.clone()],
            Self::Environment {
                account,
                application,
                environment,
            } => vec![account.clone(), application.clone(), environment.clone()],
            Self::Agent {
                account,
                application,
                environment,
                component,
                agent,
            } => vec![
                account.clone(),
                application.clone(),
                environment.clone(),
                component.clone(),
                agent.clone(),
            ],
        }
    }
}

fn parse_segment(value: &str) -> PathSegmentPattern {
    if value == "*" {
        PathSegmentPattern::Any
    } else {
        PathSegmentPattern::Exact(value.to_string())
    }
}

fn is_concrete_root(segment: &PathSegmentPattern) -> bool {
    matches!(segment, PathSegmentPattern::Exact(_))
}

fn parse_path_segments(value: &str) -> Result<Vec<PathSegmentPattern>, String> {
    if value.is_empty() || value.split('/').any(str::is_empty) {
        Err(value.to_string())
    } else {
        let segments = value.split('/').map(parse_segment).collect::<Vec<_>>();
        if has_concrete_after_wildcard(&segments) {
            Err(value.to_string())
        } else {
            Ok(segments)
        }
    }
}

fn has_concrete_after_wildcard(segments: &[PathSegmentPattern]) -> bool {
    let mut seen_wildcard = false;
    for segment in segments {
        match segment {
            PathSegmentPattern::Any => seen_wildcard = true,
            PathSegmentPattern::Exact(_) if seen_wildcard => return true,
            PathSegmentPattern::Exact(_) => {}
        }
    }
    false
}
