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
pub struct OplogRegion {
    pub start: u64,
    pub end: u64,
}

impl OplogRegion {
    pub fn contains(&self, target: u64) -> bool {
        target > self.end && target <= self.start
    }
}

impl Display for OplogRegion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} => {}>", self.start, self.end)
    }
}

/// Structure holding all the regions deleted from the oplog by jumps
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct DeletedRegions {
    regions: Vec<OplogRegion>,
}

impl Default for DeletedRegions {
    fn default() -> Self {
        Self::new()
    }
}

impl DeletedRegions {
    /// Constructs an empty set of deleted regions
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    /// Initializes from known list of deleted oplog regions
    pub fn from_regions(regions: &[OplogRegion]) -> Self {
        Self {
            regions: regions.to_vec(),
        }
    }

    /// Returns the list of deleted regions
    pub fn regions(&self) -> &Vec<OplogRegion> {
        &self.regions
    }

    /// Adds a new region to the list of deleted regions
    pub fn add(&mut self, region: OplogRegion) {
        self.regions.push(region);
    }

    /// Checks whether there is an deleted region starting at the given oplog index.
    /// If there is, returns the oplog index where the execution should continue, otherwise returns None.
    pub fn is_deleted_region_start(&self, oplog_index: u64) -> Option<u64> {
        // TODO: optimize this by introducing a map during construction
        self.regions
            .iter()
            .filter_map(|region| {
                if region.start == oplog_index {
                    Some(region.end + 1)
                } else {
                    None
                }
            })
            .max()
    }

    pub fn is_in_deleted_region(&self, oplog_index: u64) -> bool {
        self.regions
            .iter()
            .any(|region| region.contains(oplog_index))
    }
}

impl Display for DeletedRegions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]",
            self.regions
                .iter()
                .map(|region| region.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}
