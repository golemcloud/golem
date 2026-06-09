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
use crate::model::permission_share::PermissionShareName;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountPermissionShareResourcePattern {
    Any,
    Name(PermissionShareName),
}

impl AccountPermissionShareResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn name(value: PermissionShareName) -> Self {
        Self::Name(value)
    }
}

impl ResourcePattern for AccountPermissionShareResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(AccountPermissionShareResourcePattern::Any)
        } else if !resource.is_empty() {
            Ok(AccountPermissionShareResourcePattern::Name(
                PermissionShareName(resource.to_string()),
            ))
        } else {
            Err(CardParseError::InvalidResource {
                class: AccountPermissionShareClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Name(_), Self::Any) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountPermissionShareVerb {
    View,
    Create,
    Update,
    Delete,
}

impl VerbPattern for AccountPermissionShareVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountPermissionShareClass;

impl PermissionClass for AccountPermissionShareClass {
    type Verb = AccountPermissionShareVerb;
    type Owner = AccountOwnerPattern;
    type Resource = AccountPermissionShareResourcePattern;
    const NAME: &'static str = "account.permission-share";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::AccountPermissionShare(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::AccountPermissionShare(pattern)
    }
}
