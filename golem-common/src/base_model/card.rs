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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, desert_rust::BinaryCodec)]
#[desert(transparent)]
pub struct OwnerPathPattern(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, desert_rust::BinaryCodec)]
#[desert(transparent)]
pub struct RecipientPathPattern(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionPattern {}

impl desert_rust::BinarySerializer for PermissionPattern {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        _context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match *self {}
    }
}

impl desert_rust::BinaryDeserializer for PermissionPattern {
    fn deserialize(
        _context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        Err(desert_rust::Error::DeserializationFailure(
            "PermissionPattern has no variants yet".to_string(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, desert_rust::BinaryCodec)]
pub struct PatternGrant {
    pub owner: OwnerPathPattern,
    pub recipient: RecipientPathPattern,
    pub permission: PermissionPattern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub system_card: bool,
    pub polymorphic: bool,
}
