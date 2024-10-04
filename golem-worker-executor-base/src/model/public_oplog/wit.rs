// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::public_oplog::PublicOplogEntry;
use crate::preview2::golem::api1_1_0_rc1::oplog::{AccountId, CreateParameters};
use crate::preview2::wasi::clocks::wall_clock::Datetime;
use golem_common::model::Timestamp;

impl From<PublicOplogEntry> for crate::preview2::golem::api1_1_0_rc1::oplog::OplogEntry {
    fn from(value: PublicOplogEntry) -> Self {
        match value {
            PublicOplogEntry::Create {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                account_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
            } => Self::Create(CreateParameters {
                timestamp: timestamp.into(),
                worker_id: worker_id.into(),
                component_version,
                args,
                env,
                account_id: AccountId {
                    value: account_id.value,
                },
                parent: parent.map(|id| id.into()),
                component_size,
                initial_total_linear_memory_size,
            }),
            _ => todo!(),
        }
    }
}

impl From<Timestamp> for Datetime {
    fn from(value: Timestamp) -> Self {
        let ms = value.to_millis();
        Self {
            seconds: ms / 1000,
            nanoseconds: ((ms % 1000) * 1_000_000) as u32,
        }
    }
}
