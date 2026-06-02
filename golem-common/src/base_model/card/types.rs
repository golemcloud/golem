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
    PermissionPattern, PolymorphicManifestPermissionPattern, PolymorphicPermissionPattern,
};
use crate::base_model::account::AccountId;
use crate::base_model::environment::EnvironmentId;
use crate::{declare_revision, newtype_uuid};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

newtype_uuid!(CardId);

declare_revision!(CardRevision);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardManagedBy {
    AccountRoot { account_id: AccountId },
    TokenRoot { account_id: AccountId },
    EnvironmentDefault { environment_id: EnvironmentId },
    PermissionShare { permission_share_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PolymorphicManifestCard {
    pub card_id: CardId,
    pub parent_ids: Vec<CardId>,
    pub lower_positive: Vec<PolymorphicManifestPermissionPattern>,
    pub lower_negative: Vec<PolymorphicManifestPermissionPattern>,
    pub upper_positive: Vec<PolymorphicManifestPermissionPattern>,
    pub upper_negative: Vec<PolymorphicManifestPermissionPattern>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
}
