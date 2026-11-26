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

use crate::base_model::Timestamp;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(
    Debug, Copy, Clone, PartialOrd, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum InterruptKind {
    Interrupt(Timestamp),
    Restart,
    Suspend(Timestamp),
    Jump,
    /// Cooperative cancellation of one transient entity body, not its owning agent.
    EntityCancellation,
    /// This executor no longer owns the agent's shard. Terminal here: the agent is stopped
    /// without writing to its oplog or its status, dropped from the executor, and left for the
    /// worker service to resume on the shard's owner. Never a restart in place - that would
    /// reopen the oplog with the same stale epoch.
    ShardLost,
}

impl Display for InterruptKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            InterruptKind::Interrupt(_) => write!(f, "Interrupted via the Golem API"),
            InterruptKind::Restart => write!(f, "Simulated crash via the Golem API"),
            InterruptKind::Suspend(_) => write!(f, "Suspended"),
            InterruptKind::Jump => write!(f, "Jumping back in time"),
            InterruptKind::EntityCancellation => write!(f, "Entity invocation cancelled"),
            InterruptKind::ShardLost => {
                write!(f, "This executor no longer owns the agent's shard")
            }
        }
    }
}

impl Error for InterruptKind {}
