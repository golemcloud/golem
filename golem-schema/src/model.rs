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

use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};

/// Native counterpart of `golem:core/types@2.0.0::uuid`.
pub type Uuid = uuid::Uuid;

/// Native counterpart of `golem:core/types@2.0.0::datetime`.
pub type Datetime = chrono::DateTime<chrono::Utc>;

/// Native counterpart of `golem:core/types@2.0.0::environment-id`.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[schema(named = "golem.core.EnvironmentId")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
pub struct EnvironmentId {
    pub uuid: Uuid,
}

impl EnvironmentId {
    pub fn new(uuid: Uuid) -> Self {
        Self { uuid }
    }
}

impl From<Uuid> for EnvironmentId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

impl From<EnvironmentId> for Uuid {
    fn from(value: EnvironmentId) -> Self {
        value.uuid
    }
}

/// Native counterpart of `golem:core/types@2.0.0::component-id`.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[schema(named = "golem.core.ComponentId")]
pub struct ComponentId {
    pub uuid: Uuid,
}

impl ComponentId {
    pub fn new(uuid: Uuid) -> Self {
        Self { uuid }
    }
}

impl From<Uuid> for ComponentId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

impl From<ComponentId> for Uuid {
    fn from(value: ComponentId) -> Self {
        value.uuid
    }
}

/// Native counterpart of `golem:core/types@2.0.0::account-id`.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[schema(named = "golem.core.AccountId")]
pub struct AccountId {
    pub uuid: Uuid,
}

impl AccountId {
    pub fn new(uuid: Uuid) -> Self {
        Self { uuid }
    }
}

impl From<Uuid> for AccountId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

impl From<AccountId> for Uuid {
    fn from(value: AccountId) -> Self {
        value.uuid
    }
}

/// Native counterpart of `golem:core/types@2.0.0::card-id`.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[schema(named = "golem.core.CardId")]
pub struct CardId {
    pub uuid: Uuid,
}

impl CardId {
    pub fn new(uuid: Uuid) -> Self {
        Self { uuid }
    }
}

impl From<Uuid> for CardId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

impl From<CardId> for Uuid {
    fn from(value: CardId) -> Self {
        value.uuid
    }
}

/// Native counterpart of `golem:core/types@2.0.0::agent-id`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[schema(named = "golem.core.AgentId")]
pub struct AgentId {
    pub component_id: ComponentId,
    pub agent_id: String,
}

impl AgentId {
    pub fn new(component_id: ComponentId, agent_id: String) -> Self {
        Self {
            component_id,
            agent_id,
        }
    }
}

/// Native counterpart of `golem:core/types@2.0.0::oplog-index`.
pub type OplogIndex = u64;

/// Native counterpart of `golem:core/types@2.0.0::promise-id`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[schema(named = "golem.core.PromiseId")]
pub struct PromiseId {
    pub agent_id: AgentId,
    pub oplog_idx: OplogIndex,
}

impl PromiseId {
    pub fn new(agent_id: AgentId, oplog_idx: OplogIndex) -> Self {
        Self {
            agent_id,
            oplog_idx,
        }
    }
}
