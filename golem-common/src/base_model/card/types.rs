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

use super::{PermissionPattern, PolymorphicPermissionPattern};
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::component::{ComponentId, ComponentRevision};
use crate::base_model::environment::EnvironmentId;
use crate::{declare_revision, newtype_uuid};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

newtype_uuid!(CardId, wit_name: "card-id", wit_owner: "golem:core@2.0.0/types");

declare_revision!(CardRevision);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardManagedBy {
    AccountRoot {
        account_id: AccountId,
    },
    EnvironmentDefault {
        environment_id: EnvironmentId,
    },
    PermissionShare {
        permission_share_id: Uuid,
    },
    AgentInitial {
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type: AgentTypeName,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct Card {
    pub card_id: CardId,
    pub parent_ids: Vec<CardId>,
    pub lower_positive: Vec<PermissionPattern>,
    pub lower_negative: Vec<PermissionPattern>,
    pub upper_positive: Vec<PermissionPattern>,
    pub upper_negative: Vec<PermissionPattern>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
    pub managed_by: Option<CardManagedBy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PolymorphicCard {
    pub card_id: CardId,
    pub parent_ids: Vec<CardId>,
    pub lower_positive: Vec<PolymorphicPermissionPattern>,
    pub lower_negative: Vec<PolymorphicPermissionPattern>,
    pub upper_positive: Vec<PolymorphicPermissionPattern>,
    pub upper_negative: Vec<PolymorphicPermissionPattern>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
}

#[cfg(feature = "full")]
impl poem_openapi::types::Type for PolymorphicCard {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "PolymorphicCard".into()
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        poem_openapi::registry::MetaSchemaRef::Reference(Self::name().into_owned())
    }

    fn register(registry: &mut poem_openapi::registry::Registry) {
        registry.create_schema::<Self, _>(Self::name().into_owned(), |_| {
            poem_openapi::registry::MetaSchema::new("object")
        });
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ToJSON for PolymorphicCard {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromJSON for PolymorphicCard {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        match value {
            Some(value) => {
                serde_json::from_value(value).map_err(poem_openapi::types::ParseError::custom)
            }
            None => Err(poem_openapi::types::ParseError::expected_input()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StoredCard {
    Concrete(Card),
    Polymorphic(PolymorphicCard),
}

impl From<Card> for StoredCard {
    fn from(value: Card) -> Self {
        Self::Concrete(value)
    }
}

impl From<PolymorphicCard> for StoredCard {
    fn from(value: PolymorphicCard) -> Self {
        Self::Polymorphic(value)
    }
}

impl StoredCard {
    pub fn card_id(&self) -> CardId {
        match self {
            Self::Concrete(card) => card.card_id,
            Self::Polymorphic(card) => card.card_id,
        }
    }

    pub fn parent_ids(&self) -> &[CardId] {
        match self {
            Self::Concrete(card) => &card.parent_ids,
            Self::Polymorphic(card) => &card.parent_ids,
        }
    }

    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Concrete(card) => card.expires_at,
            Self::Polymorphic(card) => card.expires_at,
        }
    }

    pub fn system_card(&self) -> bool {
        match self {
            Self::Concrete(card) => card.system_card,
            Self::Polymorphic(card) => card.system_card,
        }
    }

    pub fn into_concrete(self) -> Result<Card, Self> {
        match self {
            Self::Concrete(card) => Ok(card),
            other => Err(other),
        }
    }

    pub fn into_polymorphic(self) -> Result<PolymorphicCard, Self> {
        match self {
            Self::Polymorphic(card) => Ok(card),
            other => Err(other),
        }
    }
}
