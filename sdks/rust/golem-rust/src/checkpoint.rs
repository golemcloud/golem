// Copyright 2024-2026 Golem Cloud
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

use crate::bindings::golem::api::host::{get_oplog_index, set_oplog_index, OplogIndex};

/// A checkpoint that captures the current oplog index and can revert execution back to it.
pub struct Checkpoint {
    oplog_index: OplogIndex,
}

impl Default for Checkpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl Checkpoint {
    /// Creates a new checkpoint at the current oplog index.
    pub fn new() -> Self {
        Self {
            oplog_index: get_oplog_index(),
        }
    }

    /// Reverts execution back to the checkpoint's oplog index.
    pub fn revert(&self) -> ! {
        set_oplog_index(self.oplog_index);
        unreachable!()
    }

    /// Runs the given function, reverting to the checkpoint on error.
    pub fn run_or_revert<T, E>(&self, f: impl FnOnce() -> Result<T, E>) -> T {
        match f() {
            Ok(value) => value,
            Err(_) => self.revert(),
        }
    }

    /// Reverts to the checkpoint if the condition is false.
    pub fn assert_or_revert(&self, condition: bool) {
        if !condition {
            self.revert();
        }
    }
}

/// Extension trait for `Result` that provides checkpoint-based revert on error.
pub trait CheckpointResultExt<T, E> {
    /// Returns the `Ok` value, or reverts to the checkpoint on `Err`.
    fn unwrap_or_revert(self, checkpoint: &Checkpoint) -> T;

    /// Returns the `Ok` value, or logs the message and reverts to the checkpoint on `Err`.
    fn expect_or_revert(self, checkpoint: &Checkpoint, msg: &str) -> T;
}

impl<T, E> CheckpointResultExt<T, E> for Result<T, E> {
    fn unwrap_or_revert(self, checkpoint: &Checkpoint) -> T {
        match self {
            Ok(value) => value,
            Err(_) => checkpoint.revert(),
        }
    }

    fn expect_or_revert(self, checkpoint: &Checkpoint, msg: &str) -> T {
        match self {
            Ok(value) => value,
            Err(_) => {
                eprintln!("{msg}");
                checkpoint.revert();
            }
        }
    }
}

/// Extension trait for `Option` that provides checkpoint-based revert on `None`.
pub trait CheckpointOptionExt<T> {
    /// Returns the `Some` value, or reverts to the checkpoint on `None`.
    fn unwrap_or_revert(self, checkpoint: &Checkpoint) -> T;

    /// Returns the `Some` value, or logs the message and reverts to the checkpoint on `None`.
    fn expect_or_revert(self, checkpoint: &Checkpoint, msg: &str) -> T;
}

impl<T> CheckpointOptionExt<T> for Option<T> {
    fn unwrap_or_revert(self, checkpoint: &Checkpoint) -> T {
        match self {
            Some(value) => value,
            None => checkpoint.revert(),
        }
    }

    fn expect_or_revert(self, checkpoint: &Checkpoint, msg: &str) -> T {
        match self {
            Some(value) => value,
            None => {
                eprintln!("{msg}");
                checkpoint.revert();
            }
        }
    }
}

/// Creates a checkpoint, runs the given function, and reverts on error.
pub fn with_checkpoint<T, E>(f: impl FnOnce(&Checkpoint) -> Result<T, E>) -> T {
    let checkpoint = Checkpoint::new();
    match f(&checkpoint) {
        Ok(value) => value,
        Err(_) => checkpoint.revert(),
    }
}
