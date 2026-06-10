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
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolOwnerPattern {
    AnyTools,
    AccountTools {
        account: AccountEmail,
    },
    ApplicationTools {
        account: AccountEmail,
        application: ApplicationName,
    },
    EnvironmentTools {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
    },
    ComponentTools {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
    },
    Tool {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
        tool: String,
    },
}

impl ToolOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*", "*", "*", "*"] => Ok(Self::AnyTools),
            [account, "*", "*", "*", "*"] => Ok(Self::AccountTools {
                account: AccountEmail::new(parse_concrete_segment(account)?),
            }),
            [account, application, "*", "*", "*"] => Ok(Self::ApplicationTools {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
            }),
            [account, application, environment, "*", "*"] => Ok(Self::EnvironmentTools {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
            }),
            [account, application, environment, component, "*"] => Ok(Self::ComponentTools {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
                component: ComponentName(parse_concrete_segment(component)?.to_string()),
            }),
            [account, application, environment, component, tool] => Ok(Self::Tool {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
                component: ComponentName(parse_concrete_segment(component)?.to_string()),
                tool: parse_concrete_segment(tool)?.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&AccountEmail> {
        match self {
            Self::AnyTools => None,
            Self::AccountTools { account }
            | Self::ApplicationTools { account, .. }
            | Self::EnvironmentTools { account, .. }
            | Self::ComponentTools { account, .. }
            | Self::Tool { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&AccountEmail, &ApplicationName)> {
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

    fn environment_part(&self) -> Option<(&AccountEmail, &ApplicationName, &EnvironmentName)> {
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

    fn component_part(
        &self,
    ) -> Option<(
        &AccountEmail,
        &ApplicationName,
        &EnvironmentName,
        &ComponentName,
    )> {
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
    AccountTools,
    AccountApplicationTools {
        application: ApplicationName,
    },
    AccountEnvironmentTools {
        application: ApplicationName,
        environment: EnvironmentName,
    },
    AccountComponentTools {
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
    },
    AccountTool {
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
        tool: String,
    },
    ApplicationTools,
    ApplicationEnvironmentTools {
        environment: EnvironmentName,
    },
    ApplicationComponentTools {
        environment: EnvironmentName,
        component: ComponentName,
    },
    ApplicationTool {
        environment: EnvironmentName,
        component: ComponentName,
        tool: String,
    },
    EnvTools,
    EnvComponentTools {
        component: ComponentName,
    },
    EnvTool {
        component: ComponentName,
        tool: String,
    },
    ComponentTools,
    ComponentTool {
        tool: String,
    },
}

impl OwnerPattern for ToolOwnerPattern {
    type Polymorphic = PolymorphicToolOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_owner_slot(value)? {
            Some(("?account", rest)) if rest.as_slice() == ["*", "*", "*", "*"] => {
                Ok(PolymorphicToolOwnerPattern::AccountTools)
            }
            Some(("?account", rest))
                if rest.len() == 4 && rest[1] == "*" && rest[2] == "*" && rest[3] == "*" =>
            {
                Ok(PolymorphicToolOwnerPattern::AccountApplicationTools {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                })
            }
            Some(("?account", rest)) if rest.len() == 4 && rest[2] == "*" && rest[3] == "*" => {
                Ok(PolymorphicToolOwnerPattern::AccountEnvironmentTools {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                })
            }
            Some(("?account", rest)) if rest.len() == 4 && rest[3] == "*" => {
                Ok(PolymorphicToolOwnerPattern::AccountComponentTools {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                    component: ComponentName(parse_concrete_segment(rest[2])?.to_string()),
                })
            }
            Some(("?account", rest)) if rest.len() == 4 => {
                Ok(PolymorphicToolOwnerPattern::AccountTool {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                    component: ComponentName(parse_concrete_segment(rest[2])?.to_string()),
                    tool: parse_concrete_segment(rest[3])?.to_string(),
                })
            }
            Some(("?app", rest)) if rest.as_slice() == ["*", "*", "*"] => {
                Ok(PolymorphicToolOwnerPattern::ApplicationTools)
            }
            Some(("?app", rest)) if rest.len() == 3 && rest[1] == "*" && rest[2] == "*" => {
                Ok(PolymorphicToolOwnerPattern::ApplicationEnvironmentTools {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                })
            }
            Some(("?app", rest)) if rest.len() == 3 && rest[2] == "*" => {
                Ok(PolymorphicToolOwnerPattern::ApplicationComponentTools {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                    component: ComponentName(parse_concrete_segment(rest[1])?.to_string()),
                })
            }
            Some(("?app", rest)) if rest.len() == 3 => {
                Ok(PolymorphicToolOwnerPattern::ApplicationTool {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                    component: ComponentName(parse_concrete_segment(rest[1])?.to_string()),
                    tool: parse_concrete_segment(rest[2])?.to_string(),
                })
            }
            Some(("?env", rest)) if rest.as_slice() == ["*", "*"] => {
                Ok(PolymorphicToolOwnerPattern::EnvTools)
            }
            Some(("?env", rest)) if rest.len() == 2 && rest[1] == "*" => {
                Ok(PolymorphicToolOwnerPattern::EnvComponentTools {
                    component: ComponentName(parse_concrete_segment(rest[0])?.to_string()),
                })
            }
            Some(("?env", rest)) if rest.len() == 2 => Ok(PolymorphicToolOwnerPattern::EnvTool {
                component: ComponentName(parse_concrete_segment(rest[0])?.to_string()),
                tool: parse_concrete_segment(rest[1])?.to_string(),
            }),
            Some(("?component", rest)) if rest.as_slice() == ["*"] => {
                Ok(PolymorphicToolOwnerPattern::ComponentTools)
            }
            Some(("?component", rest)) if rest.len() == 1 => {
                Ok(PolymorphicToolOwnerPattern::ComponentTool {
                    tool: parse_concrete_segment(rest[0])?.to_string(),
                })
            }
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
