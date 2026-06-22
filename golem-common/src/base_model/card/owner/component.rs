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
pub enum ComponentOwnerPattern {
    AnyComponents,
    AccountComponents {
        account: AccountEmail,
    },
    ApplicationComponents {
        account: AccountEmail,
        application: ApplicationName,
    },
    EnvironmentComponents {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
    },
    Component {
        account: AccountEmail,
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
    },
}

impl ComponentOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*", "*", "*"] => Ok(Self::AnyComponents),
            [account, "*", "*", "*"] => Ok(Self::AccountComponents {
                account: AccountEmail::new(parse_concrete_segment(account)?),
            }),
            [account, application, "*", "*"] => Ok(Self::ApplicationComponents {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
            }),
            [account, application, environment, "*"] => Ok(Self::EnvironmentComponents {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
            }),
            [account, application, environment, component] => Ok(Self::Component {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
                environment: EnvironmentName::try_from(parse_concrete_segment(environment)?)?,
                component: ComponentName(parse_concrete_segment(component)?.to_string()),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&AccountEmail> {
        match self {
            Self::AnyComponents => None,
            Self::AccountComponents { account }
            | Self::ApplicationComponents { account, .. }
            | Self::EnvironmentComponents { account, .. }
            | Self::Component { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&AccountEmail, &ApplicationName)> {
        match self {
            Self::ApplicationComponents {
                account,
                application,
            }
            | Self::EnvironmentComponents {
                account,
                application,
                ..
            }
            | Self::Component {
                account,
                application,
                ..
            } => Some((account, application)),
            Self::AnyComponents | Self::AccountComponents { .. } => None,
        }
    }

    fn environment_part(&self) -> Option<(&AccountEmail, &ApplicationName, &EnvironmentName)> {
        match self {
            Self::EnvironmentComponents {
                account,
                application,
                environment,
            }
            | Self::Component {
                account,
                application,
                environment,
                ..
            } => Some((account, application, environment)),
            Self::AnyComponents
            | Self::AccountComponents { .. }
            | Self::ApplicationComponents { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicComponentOwnerPattern {
    Concrete(ComponentOwnerPattern),
    AccountComponents,
    AccountApplicationComponents {
        application: ApplicationName,
    },
    AccountEnvironmentComponents {
        application: ApplicationName,
        environment: EnvironmentName,
    },
    AccountComponent {
        application: ApplicationName,
        environment: EnvironmentName,
        component: ComponentName,
    },
    ApplicationComponents,
    ApplicationEnvironmentComponents {
        environment: EnvironmentName,
    },
    ApplicationComponent {
        environment: EnvironmentName,
        component: ComponentName,
    },
    EnvComponents,
    EnvComponent {
        component: ComponentName,
    },
    Component,
}

impl OwnerPattern for ComponentOwnerPattern {
    type Polymorphic = PolymorphicComponentOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_owner_slot(value)? {
            Some(("?account", rest)) if rest.as_slice() == ["*", "*", "*"] => {
                Ok(PolymorphicComponentOwnerPattern::AccountComponents)
            }
            Some(("?account", rest)) if rest.len() == 3 && rest[1] == "*" && rest[2] == "*" => Ok(
                PolymorphicComponentOwnerPattern::AccountApplicationComponents {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                },
            ),
            Some(("?account", rest)) if rest.len() == 3 && rest[2] == "*" => Ok(
                PolymorphicComponentOwnerPattern::AccountEnvironmentComponents {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                },
            ),
            Some(("?account", rest)) if rest.len() == 3 => {
                Ok(PolymorphicComponentOwnerPattern::AccountComponent {
                    application: ApplicationName::try_from(parse_concrete_segment(rest[0])?)?,
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[1])?)?,
                    component: ComponentName(parse_concrete_segment(rest[2])?.to_string()),
                })
            }
            Some(("?app", rest)) if rest.as_slice() == ["*", "*"] => {
                Ok(PolymorphicComponentOwnerPattern::ApplicationComponents)
            }
            Some(("?app", rest)) if rest.len() == 2 && rest[1] == "*" => Ok(
                PolymorphicComponentOwnerPattern::ApplicationEnvironmentComponents {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                },
            ),
            Some(("?app", rest)) if rest.len() == 2 => {
                Ok(PolymorphicComponentOwnerPattern::ApplicationComponent {
                    environment: EnvironmentName::try_from(parse_concrete_segment(rest[0])?)?,
                    component: ComponentName(parse_concrete_segment(rest[1])?.to_string()),
                })
            }
            Some(("?env", rest)) if rest.as_slice() == ["*"] => {
                Ok(PolymorphicComponentOwnerPattern::EnvComponents)
            }
            Some(("?env", rest)) if rest.len() == 1 => {
                Ok(PolymorphicComponentOwnerPattern::EnvComponent {
                    component: ComponentName(parse_concrete_segment(rest[0])?.to_string()),
                })
            }
            Some(("?component", rest)) if rest.is_empty() => {
                Ok(PolymorphicComponentOwnerPattern::Component)
            }
            Some(_) => Err(value.to_string()),
            None => Self::parse(value).map(PolymorphicComponentOwnerPattern::Concrete),
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyComponents, _) => true,
            (Self::AccountComponents { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b)
            }
            (
                Self::ApplicationComponents {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .application_part()
                .is_some_and(|(ba, bp)| aa == ba && ap == bp),
            (
                Self::EnvironmentComponents {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                other,
            ) => other
                .environment_part()
                .is_some_and(|(ba, bp, be)| aa == ba && ap == bp && ae == be),
            (
                Self::Component {
                    account: aa,
                    application: ap,
                    environment: ae,
                    component: ac,
                },
                Self::Component {
                    account: ba,
                    application: bp,
                    environment: be,
                    component: bc,
                },
            ) => aa == ba && ap == bp && ae == be && ac == bc,
            (Self::Component { .. }, _) => false,
        }
    }
}
