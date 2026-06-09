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
pub enum EnvironmentOwnerPattern {
    AnyEnvironments,
    AccountEnvironments {
        account: String,
    },
    ApplicationEnvironments {
        account: String,
        application: String,
    },
    Environment {
        account: String,
        application: String,
        environment: String,
    },
}

impl EnvironmentOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*", "*"] => Ok(Self::AnyEnvironments),
            [account, "*", "*"] => Ok(Self::AccountEnvironments {
                account: parse_concrete_segment(account)?.to_string(),
            }),
            [account, application, "*"] => Ok(Self::ApplicationEnvironments {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
            }),
            [account, application, environment] => Ok(Self::Environment {
                account: parse_concrete_segment(account)?.to_string(),
                application: parse_concrete_segment(application)?.to_string(),
                environment: parse_concrete_segment(environment)?.to_string(),
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&str> {
        match self {
            Self::AnyEnvironments => None,
            Self::AccountEnvironments { account }
            | Self::ApplicationEnvironments { account, .. }
            | Self::Environment { account, .. } => Some(account),
        }
    }

    fn application_part(&self) -> Option<(&str, &str)> {
        match self {
            Self::ApplicationEnvironments {
                account,
                application,
            }
            | Self::Environment {
                account,
                application,
                ..
            } => Some((account, application)),
            Self::AnyEnvironments | Self::AccountEnvironments { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEnvironmentOwnerPattern {
    Concrete(EnvironmentOwnerPattern),
    Env,
    Self_,
}

impl OwnerPattern for EnvironmentOwnerPattern {
    type Polymorphic = PolymorphicEnvironmentOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        parse_prefix_owner_slot(value, Self::parse).map(|slot| match slot {
            PrefixOwnerSlot::Concrete(owner) => PolymorphicEnvironmentOwnerPattern::Concrete(owner),
            PrefixOwnerSlot::Env => PolymorphicEnvironmentOwnerPattern::Env,
            PrefixOwnerSlot::Self_ => PolymorphicEnvironmentOwnerPattern::Self_,
        })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyEnvironments, _) => true,
            (Self::AccountEnvironments { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b)
            }
            (
                Self::ApplicationEnvironments {
                    account: aa,
                    application: ap,
                },
                other,
            ) => other
                .application_part()
                .is_some_and(|(ba, bp)| aa == ba && ap == bp),
            (
                Self::Environment {
                    account: aa,
                    application: ap,
                    environment: ae,
                },
                Self::Environment {
                    account: ba,
                    application: bp,
                    environment: be,
                },
            ) => aa == ba && ap == bp && ae == be,
            (Self::Environment { .. }, _) => false,
        }
    }
}
