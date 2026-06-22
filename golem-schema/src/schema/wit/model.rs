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

use super::wire;
use crate::model::{AccountId, AgentId, ComponentId, Datetime, EnvironmentId, PromiseId, Uuid};
use chrono::TimeZone;

pub fn uuid_to_wire(uuid: &Uuid) -> wire::Uuid {
    let (high_bits, low_bits) = uuid.as_u64_pair();
    wire::Uuid {
        high_bits,
        low_bits,
    }
}

pub fn uuid_from_wire(uuid: &wire::Uuid) -> Uuid {
    Uuid::from_u64_pair(uuid.high_bits, uuid.low_bits)
}

pub fn datetime_to_wire(datetime: &Datetime) -> wire::Datetime {
    wire::Datetime {
        seconds: datetime.timestamp(),
        nanoseconds: datetime.timestamp_subsec_nanos(),
    }
}

pub fn datetime_from_wire(datetime: &wire::Datetime) -> Option<Datetime> {
    chrono::Utc
        .timestamp_opt(datetime.seconds, datetime.nanoseconds)
        .single()
}

impl From<&EnvironmentId> for wire::EnvironmentId {
    fn from(value: &EnvironmentId) -> Self {
        Self {
            uuid: uuid_to_wire(&value.uuid),
        }
    }
}

impl From<EnvironmentId> for wire::EnvironmentId {
    fn from(value: EnvironmentId) -> Self {
        Self::from(&value)
    }
}

impl From<&wire::EnvironmentId> for EnvironmentId {
    fn from(value: &wire::EnvironmentId) -> Self {
        Self::new(uuid_from_wire(&value.uuid))
    }
}

impl From<wire::EnvironmentId> for EnvironmentId {
    fn from(value: wire::EnvironmentId) -> Self {
        Self::from(&value)
    }
}

impl From<&ComponentId> for wire::ComponentId {
    fn from(value: &ComponentId) -> Self {
        Self {
            uuid: uuid_to_wire(&value.uuid),
        }
    }
}

impl From<ComponentId> for wire::ComponentId {
    fn from(value: ComponentId) -> Self {
        Self::from(&value)
    }
}

impl From<&wire::ComponentId> for ComponentId {
    fn from(value: &wire::ComponentId) -> Self {
        Self::new(uuid_from_wire(&value.uuid))
    }
}

impl From<wire::ComponentId> for ComponentId {
    fn from(value: wire::ComponentId) -> Self {
        Self::from(&value)
    }
}

impl From<&AccountId> for wire::AccountId {
    fn from(value: &AccountId) -> Self {
        Self {
            uuid: uuid_to_wire(&value.uuid),
        }
    }
}

impl From<AccountId> for wire::AccountId {
    fn from(value: AccountId) -> Self {
        Self::from(&value)
    }
}

impl From<&wire::AccountId> for AccountId {
    fn from(value: &wire::AccountId) -> Self {
        Self::new(uuid_from_wire(&value.uuid))
    }
}

impl From<wire::AccountId> for AccountId {
    fn from(value: wire::AccountId) -> Self {
        Self::from(&value)
    }
}

impl From<&AgentId> for wire::AgentId {
    fn from(value: &AgentId) -> Self {
        Self {
            component_id: (&value.component_id).into(),
            agent_id: value.agent_id.clone(),
        }
    }
}

impl From<AgentId> for wire::AgentId {
    fn from(value: AgentId) -> Self {
        Self::from(&value)
    }
}

impl From<&wire::AgentId> for AgentId {
    fn from(value: &wire::AgentId) -> Self {
        Self::new((&value.component_id).into(), value.agent_id.clone())
    }
}

impl From<wire::AgentId> for AgentId {
    fn from(value: wire::AgentId) -> Self {
        Self::from(&value)
    }
}

impl From<&PromiseId> for wire::PromiseId {
    fn from(value: &PromiseId) -> Self {
        Self {
            agent_id: (&value.agent_id).into(),
            oplog_idx: value.oplog_idx,
        }
    }
}

impl From<PromiseId> for wire::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self::from(&value)
    }
}

impl From<&wire::PromiseId> for PromiseId {
    fn from(value: &wire::PromiseId) -> Self {
        Self::new((&value.agent_id).into(), value.oplog_idx)
    }
}

impl From<wire::PromiseId> for PromiseId {
    fn from(value: wire::PromiseId) -> Self {
        Self::from(&value)
    }
}
