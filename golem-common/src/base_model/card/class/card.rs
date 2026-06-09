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

use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AccountOwnerPattern;
use crate::model::card::recipient::RecipientPattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardResourcePattern {
    Any,
    InstallTarget(RecipientPattern),
}

impl CardResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn install_target(target: RecipientPattern) -> Self {
        Self::InstallTarget(target)
    }
}

impl ResourcePattern for CardResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(CardResourcePattern::Any)
        } else if resource.is_empty() {
            Err(CardParseError::InvalidResource {
                class: CardClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        } else {
            Ok(CardResourcePattern::InstallTarget(
                RecipientPattern::parse(resource).map_err(CardParseError::InvalidRecipientPath)?,
            ))
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::InstallTarget(a), Self::InstallTarget(b)) => a.subsumes(b),
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardVerb {
    Derive,
    Revoke,
    Inspect,
    Install,
}
impl VerbPattern for CardVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "derive" => Some(Self::Derive),
            "revoke" => Some(Self::Revoke),
            "inspect" => Some(Self::Inspect),
            "install" => Some(Self::Install),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct CardClass;

impl PermissionClass for CardClass {
    type Verb = CardVerb;
    type Owner = AccountOwnerPattern;
    type Resource = CardResourcePattern;
    const NAME: &'static str = "card";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Card(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Card(pattern)
    }
}
