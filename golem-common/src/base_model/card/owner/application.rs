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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationOwnerPattern {
    AnyApplications,
    AccountApplications {
        account: AccountEmail,
    },
    Application {
        account: AccountEmail,
        application: ApplicationName,
    },
}

impl ApplicationOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*", "*"] => Ok(Self::AnyApplications),
            [account, "*"] => Ok(Self::AccountApplications {
                account: AccountEmail::new(parse_concrete_segment(account)?),
            }),
            [account, application] => Ok(Self::Application {
                account: AccountEmail::new(parse_concrete_segment(account)?),
                application: ApplicationName::try_from(parse_concrete_segment(application)?)?,
            }),
            _ => Err(value.to_string()),
        }
    }

    fn account_part(&self) -> Option<&AccountEmail> {
        match self {
            Self::AnyApplications => None,
            Self::AccountApplications { account } | Self::Application { account, .. } => {
                Some(account)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicApplicationOwnerPattern {
    Concrete(ApplicationOwnerPattern),
    Env,
    Self_,
}

impl OwnerPattern for ApplicationOwnerPattern {
    type Polymorphic = PolymorphicApplicationOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        parse_prefix_owner_slot(value, Self::parse).map(|slot| match slot {
            PrefixOwnerSlot::Concrete(owner) => PolymorphicApplicationOwnerPattern::Concrete(owner),
            PrefixOwnerSlot::Env => PolymorphicApplicationOwnerPattern::Env,
            PrefixOwnerSlot::Self_ => PolymorphicApplicationOwnerPattern::Self_,
        })
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyApplications, _) => true,
            (Self::AccountApplications { account: a }, other) => {
                other.account_part().is_some_and(|b| a == b)
            }
            (
                Self::Application {
                    account: aa,
                    application: ap,
                },
                Self::Application {
                    account: ba,
                    application: bp,
                },
            ) => aa == ba && ap == bp,
            (Self::Application { .. }, _) => false,
        }
    }
}
