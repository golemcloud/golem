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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EmptyOwnerPattern;

impl EmptyOwnerPattern {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            Ok(Self)
        } else {
            Err(value.to_string())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PolymorphicEmptyOwnerPattern {
    Concrete(EmptyOwnerPattern),
}

impl OwnerPattern for EmptyOwnerPattern {
    type Polymorphic = PolymorphicEmptyOwnerPattern;

    fn parse(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }

    fn parse_polymorphic(value: &str) -> Result<Self::Polymorphic, String> {
        if value.is_empty() {
            Self::parse(value).map(PolymorphicEmptyOwnerPattern::Concrete)
        } else if split_leftmost_owner_slot(value)?.is_some() {
            Err(value.to_string())
        } else {
            Self::parse(value).map(PolymorphicEmptyOwnerPattern::Concrete)
        }
    }

    fn subsumes(&self, _other: &Self) -> bool {
        true
    }
}
