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
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Polymorphic grant that can monomorphized and installed, but not used for permission checks
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PolymorphicPatternGrant {
    pub permission: PolymorphicPermissionPattern,
}

/// Polymorphic as allowed in the manifest. Needs to have the recipient monormorphized before it can be sent
/// to the server
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct PolymorphicManifestPatternGrant {
    pub permission: PolymorphicManifestPermissionPattern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PermissionPattern>,
    pub lower_negative: Vec<PermissionPattern>,
    pub upper_positive: Vec<PermissionPattern>,
    pub upper_negative: Vec<PermissionPattern>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
    pub polymorphic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolymorphicCard {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PolymorphicPatternGrant>,
    pub lower_negative: Vec<PolymorphicPatternGrant>,
    pub upper_positive: Vec<PolymorphicPatternGrant>,
    pub upper_negative: Vec<PolymorphicPatternGrant>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolymorphicManifestCard {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PolymorphicManifestPatternGrant>,
    pub lower_negative: Vec<PolymorphicManifestPatternGrant>,
    pub upper_positive: Vec<PolymorphicManifestPatternGrant>,
    pub upper_negative: Vec<PolymorphicManifestPatternGrant>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
}
