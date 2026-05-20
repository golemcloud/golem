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

use golem_service_base::repo::SqlDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, desert_rust::BinaryCodec)]
pub struct PatternGrant {
    pub class: String,
    pub owner: String,
    pub recipient: String,
    pub verb: String,
    pub resource_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, desert_rust::BinaryCodec)]
pub struct CardData {
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
    /// JSON encoded metadata bytes. The metadata is intentionally opaque to the
    /// database because it is not currently used for joins, lookups, or FKs.
    pub metadata: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardRecord {
    pub card_id: Uuid,
    pub parent_ids: Vec<Uuid>,
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
    pub created_at: SqlDateTime,
    pub expires_at: Option<SqlDateTime>,
    pub system_card: bool,
    pub polymorphic: bool,
    pub metadata: Option<serde_json::Value>,
}
