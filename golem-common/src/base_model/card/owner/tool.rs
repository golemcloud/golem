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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolOwnerPattern {
    AnyTools,
    AccountTools {
        account: String,
    },
    ApplicationTools {
        account: String,
        application: String,
    },
    EnvironmentTools {
        account: String,
        application: String,
        environment: String,
    },
    ComponentTools {
        account: String,
        application: String,
        environment: String,
        component: String,
    },
    Tool {
        account: String,
        application: String,
        environment: String,
        component: String,
        tool: String,
    },
}

impl ToolOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*", "*", "*", "*"] => Ok(Self::AnyTools),
            [account, "*", "*", "*", "*"] => Ok(Self::AccountTools {
                account: parse_concrete_segment(account)?.to_string(),
            }),
            [account, application, "*", "*", "*"] => Ok(Self::ApplicationTools {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
            }),
            [account, application, environment, "*", "*"] => Ok(Self::EnvironmentTools {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
                environment: parse_concrete_segment(environment)?.to_string(),
            }),
            [account, application, environment, component, "*"] => Ok(Self::ComponentTools {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
                environment: parse_concrete_segment(environment)?.to_string(),
                component: parse_concrete_segment(component)?.to_string(),
            }),
            [account, application, environment, component, tool] => Ok(Self::Tool {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
                environment: parse_concrete_segment(environment)?.to_string(),
                component: parse_concrete_segment(component)?.to_string(),
                tool: parse_concrete_segment(tool)?.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::AnyTools => None,
            Self::AccountTools { account }
            | Self::ApplicationTools { account, .. }
            | Self::EnvironmentTools { account, .. }
            | Self::ComponentTools { account, .. }
            | Self::Tool { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&str, &str)> {
        match self {
            Self::ApplicationTools {
                account,
                application,
            }
            | Self::EnvironmentTools {
                account,
                application,
                ..
            }
            | Self::ComponentTools {
                account,
                application,
                ..
            }
            | Self::Tool {
                account,
                application,
                ..
            } => Some((account, application)),
            Self::AnyTools | Self::AccountTools { .. } => None,
        }
    }

    fn environment_part(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::EnvironmentTools {
                account,
                application,
                environment,
            }
            | Self::ComponentTools {
                account,
                application,
                environment,
                ..
            }
            | Self::Tool {
                account,
                application,
                environment,
                ..
            } => Some((account, application, environment)),
            Self::AnyTools | Self::AccountTools { .. } | Self::ApplicationTools { .. } => None,
        }
    }

    fn component_part(&self) -> Option<(&str, &str, &str, &str)> {
        match self {
            Self::ComponentTools {
                account,
                application,
                environment,
                component,
            }
            | Self::Tool {
                account,
                application,
                environment,
                component,
                ..
            } => Some((account, application, environment, component)),
            Self::AnyTools
            | Self::AccountTools { .. }
            | Self::ApplicationTools { .. }
            | Self::EnvironmentTools { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicToolOwnerPattern {
    Concrete(ToolOwnerPattern),
    EnvTools,
    EnvComponentTools { component: String },
    EnvTool { component: String, tool: String },
}

impl OwnerPattern for ToolOwnerPattern {
    type Polymorphic = PolymorphicToolOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_owner_slot(value)? {
            Some(("?env", rest)) if rest.as_slice() == ["*", "*"] => {
                Ok(PolymorphicToolOwnerPattern::EnvTools)
            }
            Some(("?env", rest)) if rest.len() == 2 && rest[1] == "*" => {
                Ok(PolymorphicToolOwnerPattern::EnvComponentTools {
                    component: parse_concrete_segment(rest[0])?.to_string(),
                })
            }
            Some(("?env", rest)) if rest.len() == 2 => Ok(PolymorphicToolOwnerPattern::EnvTool {
                component: parse_concrete_segment(rest[0])?.to_string(),
                tool: parse_concrete_segment(rest[1])?.to_string(),
            }),
            Some(_) => Err(value.to_string()),
            None => Self::parse(value).map(PolymorphicToolOwnerPattern::Concrete),
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyTools, _) => true,
            (Self::AccountTools { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b)
            }
            (
                Self::ApplicationTools {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .application_part()
                .is_some_and(|(ba, bp)| aa == ba && ap == bp),
            (
                Self::EnvironmentTools {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => other
                .environment_part()
                .is_some_and(|(ba, bp, be)| aa == ba && ap == bp && ae == be),
            (
                Self::ComponentTools {
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
                Self::Tool {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                    tool: at,
                },
                Self::Tool {
                    account: ba,
                    application: bp,
                    environment: be,
                    component: bc,
                    tool: bt,
                },
            ) => aa == ba && ap == bp && ae == be && ac == bc && at == bt,
            (Self::Tool { .. }, _) => false,
        }
    }
}
