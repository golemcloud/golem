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
use crate::{declare_revision, declare_structs, declare_unions, newtype_uuid};
use chrono::{DateTime, Utc};
use uuid::Uuid;

newtype_uuid!(CardId, wit_name: "card-id", wit_owner: "golem:core@2.0.0/types");

declare_revision!(CardRevision);

declare_structs! {
    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct CardManagedByAccountRoot {
        pub account_id: AccountId,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct CardManagedByEnvironmentDefault {
        pub environment_id: EnvironmentId,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct CardManagedByPermissionShare {
        pub permission_share_id: Uuid,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct CardManagedByAgentInitial {
        pub component_id: ComponentId,
        pub component_revision: ComponentRevision,
        pub agent_type: AgentTypeName,
    }

    #[derive(Eq)]
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

    #[derive(Eq)]
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
}

declare_unions! {
    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum StoredCard {
        Concrete(Card),
        Polymorphic(PolymorphicCard),
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum CardManagedBy {
        AccountRoot(CardManagedByAccountRoot),
        EnvironmentDefault(CardManagedByEnvironmentDefault),
        PermissionShare(CardManagedByPermissionShare),
        AgentInitial(CardManagedByAgentInitial),
    }
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
