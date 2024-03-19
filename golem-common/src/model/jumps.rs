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

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Jump {
    pub target_oplog_idx: u64,
    pub source_oplog_idx: u64,
}

impl Jump {
    pub fn contains(&self, target: u64) -> bool {
        target > self.source_oplog_idx && target <= self.target_oplog_idx
    }
}

/// Structure holding all the regions deleted from the oplog by jumps
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct DeletedRegions {
    jumps: Vec<Jump>,
}

impl DeletedRegions {
    /// Constructs an empty set of active jumps
    pub fn new() -> Self {
        Self { jumps: Vec::new() }
    }

    /// Initializes from Jump entries in the oplog
    pub fn from_jumps(entries: &[Jump]) -> Self {
        Self {
            jumps: entries.to_vec(),
        }
    }

    /// Returns the list of active jumps
    pub fn jumps(&self) -> &Vec<Jump> {
        &self.jumps
    }

    /// Adds a new jump definition to the list of active jumps
    pub fn add_jump(&mut self, jump: Jump) {
        self.jumps.push(jump);
    }

    /// Checks whether there is an deleted region starting at the given oplog index.
    /// If there is, returns the oplog index where the execution should continue, otherwise returns None.
    pub fn is_deleted_region_start(&self, at: u64) -> Option<u64> {
        // TODO: optimize this by introducing a map during construction
        self.jumps
            .iter()
            .filter_map(|jump| {
                if jump.target_oplog_idx == at {
                    Some(jump.source_oplog_idx + 1)
                } else {
                    None
                }
            })
            .max()
    }

    pub fn is_in_deleted_region(&self, target: u64) -> bool {
        self.jumps.iter().any(|jump| jump.contains(target))
    }
}

impl Display for DeletedRegions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]",
            self.jumps
                .iter()
                .map(|jump| format!("<{} => {}>", jump.source_oplog_idx, jump.target_oplog_idx))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}
