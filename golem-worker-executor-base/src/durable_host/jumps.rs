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

use golem_common::model::{Jump, OplogEntry};
use std::fmt::{Display, Formatter};

/// Structure holding all the active jumps for the current execution of a worker
pub struct ActiveJumps {
    jumps: Vec<Jump>,
    forward_jumps: Vec<u64>,
}

impl ActiveJumps {
    /// Constructs an empty set of active jumps
    pub fn new() -> Self {
        Self {
            jumps: Vec::new(),
            forward_jumps: Vec::new(),
        }
    }

    /// Initializes active jumps from the an oplog entry. If it was a jump entry use information
    /// from that, otherwise it returns empty active jumps
    pub fn from_oplog_entry(entry: &OplogEntry) -> Self {
        match entry {
            OplogEntry::Jump { jumps, .. } => Self {
                jumps: jumps.clone(),
                forward_jumps: Vec::new(),
            },
            _ => Self::new(),
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

    /// Checks whether there is an active jump to perform when reaching the given oplog index.
    /// If there is, returns the oplog index where the execution should continue, otherwise returns None.
    pub fn has_active_jump(&self, at: u64) -> Option<u64> {
        // TODO: optimize this by introducing a map during construction
        for jump in &self.jumps {
            if jump.target_oplog_idx == at {
                return Some(jump.source_oplog_idx + 1);
            }
        }
        None
    }

    pub fn record_forward_jump(&mut self, target_oplog_index: u64) {
        self.forward_jumps.push(target_oplog_index);
    }

    pub fn try_match_forward_jump(&mut self, target_oplog_idx: u64) -> bool {
        match self
            .forward_jumps
            .iter()
            .copied()
            .enumerate()
            .find(|(_, oplog_idx)| *oplog_idx == target_oplog_idx)
        {
            None => false,
            Some((vec_idx, _)) => {
                self.forward_jumps.remove(vec_idx);
                true
            }
        }
    }
}

impl Display for ActiveJumps {
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
