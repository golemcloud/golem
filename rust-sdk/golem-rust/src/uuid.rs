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

use crate::bindings::golem::api::host::ComponentId;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

impl From<crate::bindings::golem::api::host::Uuid> for Uuid {
    fn from(uuid: crate::bindings::golem::api::host::Uuid) -> Self {
        Uuid::from_u64_pair(uuid.high_bits, uuid.low_bits)
    }
}

impl From<Uuid> for crate::bindings::golem::api::host::Uuid {
    fn from(value: Uuid) -> Self {
        let (high_bits, low_bits) = value.as_u64_pair();
        Self {
            high_bits,
            low_bits,
        }
    }
}

impl From<Uuid> for crate::bindings::golem::api::host::ComponentId {
    fn from(value: Uuid) -> Self {
        Self { uuid: value.into() }
    }
}

impl From<crate::bindings::golem::api::host::ComponentId> for Uuid {
    fn from(value: crate::bindings::golem::api::host::ComponentId) -> Self {
        value.uuid.into()
    }
}

impl FromStr for crate::bindings::golem::api::host::ComponentId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Uuid::parse_str(s)?.into())
    }
}

impl Display for crate::bindings::golem::api::host::ComponentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &self.uuid)
    }
}

impl Display for crate::bindings::golem::api::host::WorkerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.component_id, self.worker_name)
    }
}

impl FromStr for crate::bindings::golem::api::host::WorkerId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let component_id = ComponentId::from_str(parts[0])
                .map_err(|_| format!("invalid component id: {s} - expected uuid"))?;
            let worker_name = parts[1].to_string();
            Ok(Self {
                component_id,
                worker_name,
            })
        } else {
            Err(format!(
                "invalid worker id: {s} - expected format: <component_id>/<worker_name>"
            ))
        }
    }
}

impl Display for crate::bindings::golem::api::host::PromiseId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.worker_id, self.oplog_idx)
    }
}

impl FromStr for crate::bindings::golem::api::host::PromiseId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let worker_id = crate::bindings::golem::api::host::WorkerId::from_str(parts[0])
                .map_err(|_| {
                    format!(
                        "invalid worker id: {s} - expected format: <component_id>/<worker_name>"
                    )
                })?;
            let oplog_idx = parts[1]
                .parse()
                .map_err(|_| format!("invalid oplog index: {s} - expected integer"))?;
            Ok(Self {
                worker_id,
                oplog_idx,
            })
        } else {
            Err(format!(
                "invalid promise id: {s} - expected format: <worker_id>/<oplog_idx>"
            ))
        }
    }
}
