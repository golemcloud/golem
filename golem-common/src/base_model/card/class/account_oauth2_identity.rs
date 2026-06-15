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
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityResourcePattern {
    Any,
    Identity {
        provider: String,
        external_id: String,
    },
}

impl AccountOauth2IdentityResourcePattern {
    pub fn any() -> Self {
        Self::Any
    }
}

impl ResourcePattern for AccountOauth2IdentityResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(AccountOauth2IdentityResourcePattern::Any)
        } else if let Some((provider, external_id)) = resource.split_once('/') {
            if provider.is_empty() || external_id.is_empty() {
                Err(CardParseError::InvalidResource {
                    class: AccountOauth2IdentityClass::NAME.to_string(),
                    resource: resource.to_string(),
                })
            } else {
                Ok(AccountOauth2IdentityResourcePattern::Identity {
                    provider: provider.to_string(),
                    external_id: external_id.to_string(),
                })
            }
        } else {
            Err(CardParseError::InvalidResource {
                class: AccountOauth2IdentityClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (
                Self::Identity {
                    provider: a_provider,
                    external_id: a_external_id,
                },
                Self::Identity {
                    provider: b_provider,
                    external_id: b_external_id,
                },
            ) => a_provider == b_provider && a_external_id == b_external_id,
            (Self::Identity { .. }, Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum AccountOauth2IdentityVerb {
    View,
    Link,
    Unlink,
}
impl VerbPattern for AccountOauth2IdentityVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "link" => Some(Self::Link),
            "unlink" => Some(Self::Unlink),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct AccountOauth2IdentityClass;

impl PermissionClass for AccountOauth2IdentityClass {
    type Verb = AccountOauth2IdentityVerb;
    type Owner = AccountOwnerPattern;
    type Resource = AccountOauth2IdentityResourcePattern;
    const NAME: &'static str = "account.oauth2-identity";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::AccountOauth2Identity(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::AccountOauth2Identity(pattern)
    }
}
