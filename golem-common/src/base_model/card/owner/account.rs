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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOwnerPattern {
    Any,
    Account { account: AccountEmail },
}

impl AccountOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        match parse_segments(value)?.as_slice() {
            ["*"] => Ok(Self::Any),
            [account] => Ok(Self::Account {
                account: AccountEmail::new(parse_concrete_segment(account)?),
            }),
            _ => Err(value.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicAccountOwnerPattern {
    Concrete(AccountOwnerPattern),
    Account,
}

impl OwnerPattern for AccountOwnerPattern {
    type Polymorphic = PolymorphicAccountOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        match split_leftmost_owner_slot(value)? {
            Some(("?account", rest)) if rest.is_empty() => {
                Ok(PolymorphicAccountOwnerPattern::Account)
            }
            Some(_) => Err(value.to_string()),
            None => Self::parse(value).map(PolymorphicAccountOwnerPattern::Concrete),
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Account { account: a }, Self::Account { account: b }) => a == b,
            (Self::Account { .. }, Self::Any) => false,
        }
    }
}
