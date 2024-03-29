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
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;

use golem_common::model::oplog::WorkerError;
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    ShardAssignment, ShardId, VersionedWorkerId, WorkerId, WorkerStatusRecord,
};
use serde::{Deserialize, Serialize};
use wasmtime::Trap;

use crate::error::GolemError;
use crate::workerctx::WorkerCtx;

pub trait ShardAssignmentCheck {
    fn check_worker(&self, worker_id: &WorkerId) -> Result<(), GolemError>;
}

impl ShardAssignmentCheck for ShardAssignment {
    fn check_worker(&self, worker_id: &WorkerId) -> Result<(), GolemError> {
        let shard_id = ShardId::from_worker_id(worker_id, self.number_of_shards);
        if self.shard_ids.contains(&shard_id) {
            Ok(())
        } else {
            Err(GolemError::invalid_shard_id(
                shard_id,
                self.shard_ids.clone(),
            ))
        }
    }
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub enum InterruptKind {
    Interrupt,
    Restart,
    Suspend,
    Jump,
}

impl Display for InterruptKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            InterruptKind::Interrupt => write!(f, "Interrupted via the Golem API"),
            InterruptKind::Restart => write!(f, "Simulated crash via the Golem API"),
            InterruptKind::Suspend => write!(f, "Suspended"),
            InterruptKind::Jump => write!(f, "Jumping back in time"),
        }
    }
}

impl Error for InterruptKind {}

/// Worker-specific configuration. These values are used to initialize the worker, and they can
/// be different for each worker.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub deleted_regions: DeletedRegions,
}

impl WorkerConfig {
    pub fn new(
        versioned_worker_id: VersionedWorkerId,
        worker_args: Vec<String>,
        mut worker_env: Vec<(String, String)>,
        deleted_regions: DeletedRegions,
    ) -> WorkerConfig {
        let worker_name = versioned_worker_id.worker_id.worker_name.clone();
        let template_id = versioned_worker_id.worker_id.template_id;
        let template_version = versioned_worker_id.template_version.to_string();
        worker_env.push((String::from("GOLEM_WORKER_NAME"), worker_name));
        worker_env.push((String::from("GOLEM_TEMPLATE_ID"), template_id.to_string()));
        worker_env.push((String::from("GOLEM_TEMPLATE_VERSION"), template_version));
        WorkerConfig {
            args: worker_args,
            env: worker_env,
            deleted_regions,
        }
    }
}

/// Information about the available resources for the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentResourceLimits {
    /// The available fuel to borrow
    #[serde(rename = "availableFuel")]
    pub fuel: i64,
    /// The maximum amount of memory that can be used by the worker
    #[serde(rename = "maxMemoryPerInstance")]
    pub max_memory: usize,
}

impl From<golem_api_grpc::proto::golem::common::ResourceLimits> for CurrentResourceLimits {
    fn from(value: golem_api_grpc::proto::golem::common::ResourceLimits) -> Self {
        Self {
            fuel: value.available_fuel,
            max_memory: value.max_memory_per_worker as usize,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ExecutionStatus {
    Running {
        last_known_status: WorkerStatusRecord,
    },
    Suspended {
        last_known_status: WorkerStatusRecord,
    },
    Interrupting {
        interrupt_kind: InterruptKind,
        await_interruption: Arc<tokio::sync::broadcast::Sender<()>>,
        last_known_status: WorkerStatusRecord,
    },
    Interrupted {
        interrupt_kind: InterruptKind,
        last_known_status: WorkerStatusRecord,
    },
}

impl ExecutionStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, ExecutionStatus::Running { .. })
    }

    pub fn last_known_status(&self) -> &WorkerStatusRecord {
        match self {
            ExecutionStatus::Running { last_known_status } => last_known_status,
            ExecutionStatus::Suspended { last_known_status } => last_known_status,
            ExecutionStatus::Interrupting {
                last_known_status, ..
            } => last_known_status,
            ExecutionStatus::Interrupted {
                last_known_status, ..
            } => last_known_status,
        }
    }

    pub fn set_last_known_status(&mut self, status: WorkerStatusRecord) {
        match self {
            ExecutionStatus::Running { last_known_status } => *last_known_status = status,
            ExecutionStatus::Suspended { last_known_status } => *last_known_status = status,
            ExecutionStatus::Interrupting {
                last_known_status, ..
            } => *last_known_status = status,
            ExecutionStatus::Interrupted {
                last_known_status, ..
            } => *last_known_status = status,
        }
    }
}

/// Describes the various reasons a worker can run into a trap
#[derive(Clone, Debug)]
pub enum TrapType {
    /// Interrupted through Golem (including user interrupts, suspends, jumps, etc)
    Interrupt(InterruptKind),
    /// Called the WASI exit function
    Exit,
    /// Failed with an error
    Error(WorkerError),
}

impl TrapType {
    pub fn from_error<Ctx: WorkerCtx>(error: &anyhow::Error) -> TrapType {
        match error.root_cause().downcast_ref::<InterruptKind>() {
            Some(kind) => TrapType::Interrupt(kind.clone()),
            None => match Ctx::is_exit(error) {
                Some(_) => TrapType::Exit,
                None => match error.root_cause().downcast_ref::<Trap>() {
                    Some(&Trap::StackOverflow) => TrapType::Error(WorkerError::StackOverflow),
                    _ => TrapType::Error(WorkerError::Unknown(format!("{:?}", error))),
                },
            },
        }
    }
}

/// Encapsulates a worker error with the number of retries already attempted.
///
/// This can be calculated by reading the (end of the) oplog, and passed around for making
/// decisions about retries/recovery.
#[derive(Clone, Debug)]
pub struct LastError {
    pub error: WorkerError,
    pub retry_count: u64,
}

impl Display for LastError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, retried {} times", self.error, self.retry_count)
    }
}

#[cfg(test)]
mod shard_id_tests {
    use golem_common::model::TemplateId;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn test_hash() {
        let uuid = Uuid::parse_str("96c12379-4fff-4fa2-aa09-a4d96c029ac2").unwrap();

        let template_id = TemplateId(uuid);
        let worker_id = WorkerId {
            template_id,
            worker_name: "instanceName".to_string(),
        };
        let hash = ShardId::hash_worker_id(&worker_id);
        println!("hash: {:?}", hash);
        assert_eq!(hash, -6692039695739768661);
    }
}
