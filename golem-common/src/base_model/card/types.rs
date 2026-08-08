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
use crate::base_model::{AgentId, IdempotencyKey, OplogIndex};
use crate::{declare_revision, declare_structs, declare_unions, newtype_uuid};
use chrono::{DateTime, Utc};
use uuid::Uuid;

newtype_uuid!(CardId, wit_name: "card-id", wit_owner: "golem:core@2.0.0/types");

#[cfg(feature = "full")]
impl From<CardId> for golem_schema::schema::wit::wire::CardId {
    fn from(card_id: CardId) -> Self {
        card_id.0.into()
    }
}

declare_revision!(CardRevision);

declare_structs! {
    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct WalletVersionToken {
        pub wallet_id_hash: [u8; 32],
        pub generation: u64,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct InvocationWalletPin {
        pub wallet_token: WalletVersionToken,
        pub pinned_card_ids: Vec<CardId>,
        pub scope_card_id: Option<CardId>,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct PublicInvocationWalletPin {
        pub wallet_token: WalletVersionToken,
        pub scope_card_id: Option<CardId>,
    }

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
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct CardManagedByRuntimeDerived {
        pub environment_id: EnvironmentId,
        pub agent_id: AgentId,
        pub invocation_key: IdempotencyKey,
        pub oplog_index: OplogIndex,
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
    pub struct ScopeCard {
        pub scope_card_id: CardId,
        pub root_card_ids: Vec<CardId>,
        pub lower_positive: Vec<PermissionPattern>,
        pub lower_negative: Vec<PermissionPattern>,
        pub upper_positive: Vec<PermissionPattern>,
        pub upper_negative: Vec<PermissionPattern>,
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

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct AccountCardHolder {
        pub account_id: Uuid,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct ApplicationCardHolder {
        pub application_id: Uuid,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct AgentCardHolder {
        pub agent_id: AgentId,
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
        RuntimeDerived(CardManagedByRuntimeDerived),
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum CardHolder {
        Account(AccountCardHolder),
        Application(ApplicationCardHolder),
        Agent(AgentCardHolder),
    }
}

pub type PublicCardHolder = CardHolder;

impl CardHolder {
    const WALLET_ID_ENCODING_DOMAIN: &'static [u8] = b"golem:permissions:wallet-id:v1\0";

    /// Encodes the wallet owner using the stable, versioned wallet identity format.
    ///
    /// UUIDs use RFC 4122 network byte order. Agent IDs encode the component UUID
    /// followed by the big-endian byte length and UTF-8 bytes of the agent name.
    pub fn canonical_wallet_id_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::from(Self::WALLET_ID_ENCODING_DOMAIN);
        match self {
            Self::Account(holder) => {
                bytes.push(0);
                bytes.extend_from_slice(holder.account_id.as_bytes());
            }
            Self::Application(holder) => {
                bytes.push(1);
                bytes.extend_from_slice(holder.application_id.as_bytes());
            }
            Self::Agent(holder) => {
                bytes.push(2);
                bytes.extend_from_slice(holder.agent_id.component_id.0.as_bytes());
                let agent_id = holder.agent_id.agent_id.as_bytes();
                let length = u64::try_from(agent_id.len())
                    .expect("agent ID byte length does not fit into u64");
                bytes.extend_from_slice(&length.to_be_bytes());
                bytes.extend_from_slice(agent_id);
            }
        }
        bytes
    }

    pub fn wallet_id_hash(&self) -> [u8; 32] {
        *blake3::hash(&self.canonical_wallet_id_bytes()).as_bytes()
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

    pub fn created_at(&self) -> DateTime<Utc> {
        match self {
            Self::Concrete(card) => card.created_at,
            Self::Polymorphic(card) => card.created_at,
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
