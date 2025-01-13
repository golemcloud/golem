// Copyright 2024-2025 Golem Cloud
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

pub mod public_oplog;

use crate::error::{GolemError, WorkerOutOfMemory};
use crate::workerctx::WorkerCtx;
use bincode::{Decode, Encode};
use bytes::Bytes;
use futures::Stream;
use golem_common::model::oplog::WorkerError;
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    ComponentFileSystemNode, ComponentType, ShardAssignment, ShardId, Timestamp, WorkerId,
    WorkerStatusRecord,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use wasmtime::Trap;

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
    pub total_linear_memory_size: u64,
}

impl WorkerConfig {
    pub fn new(
        worker_id: WorkerId,
        component_version: u64,
        worker_args: Vec<String>,
        mut worker_env: Vec<(String, String)>,
        deleted_regions: DeletedRegions,
        total_linear_memory_size: u64,
    ) -> WorkerConfig {
        let worker_name = worker_id.worker_name.clone();
        let component_id = worker_id.component_id;
        let component_version = component_version.to_string();
        worker_env.retain(|(key, _)| {
            key != "GOLEM_WORKER_NAME"
                && key != "GOLEM_COMPONENT_ID"
                && key != "GOLEM_COMPONENT_VERSION"
        });
        worker_env.push((String::from("GOLEM_WORKER_NAME"), worker_name));
        worker_env.push((String::from("GOLEM_COMPONENT_ID"), component_id.to_string()));
        worker_env.push((String::from("GOLEM_COMPONENT_VERSION"), component_version));
        WorkerConfig {
            args: worker_args,
            env: worker_env,
            deleted_regions,
            total_linear_memory_size,
        }
    }
}

/// Information about the available resources for the worker.
#[derive(Debug, Clone)]
pub struct CurrentResourceLimits {
    /// The available fuel to borrow
    pub fuel: i64,
    /// The maximum amount of memory that can be used by the worker
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
    Loading {
        last_known_status: WorkerStatusRecord,
        component_type: ComponentType,
        timestamp: Timestamp,
    },
    Running {
        last_known_status: WorkerStatusRecord,
        component_type: ComponentType,
        timestamp: Timestamp,
    },
    Suspended {
        last_known_status: WorkerStatusRecord,
        component_type: ComponentType,
        timestamp: Timestamp,
    },
    Interrupting {
        interrupt_kind: InterruptKind,
        await_interruption: Arc<tokio::sync::broadcast::Sender<()>>,
        last_known_status: WorkerStatusRecord,
        component_type: ComponentType,
        timestamp: Timestamp,
    },
}

impl ExecutionStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, ExecutionStatus::Running { .. })
    }

    pub fn last_known_status(&self) -> &WorkerStatusRecord {
        match self {
            ExecutionStatus::Loading {
                last_known_status, ..
            } => last_known_status,
            ExecutionStatus::Running {
                last_known_status, ..
            } => last_known_status,
            ExecutionStatus::Suspended {
                last_known_status, ..
            } => last_known_status,
            ExecutionStatus::Interrupting {
                last_known_status, ..
            } => last_known_status,
        }
    }

    pub fn set_last_known_status(&mut self, status: WorkerStatusRecord) {
        match self {
            ExecutionStatus::Loading {
                last_known_status, ..
            } => *last_known_status = status,
            ExecutionStatus::Running {
                last_known_status, ..
            } => *last_known_status = status,
            ExecutionStatus::Suspended {
                last_known_status, ..
            } => *last_known_status = status,
            ExecutionStatus::Interrupting {
                last_known_status, ..
            } => *last_known_status = status,
        }
    }

    pub fn timestamp(&self) -> Timestamp {
        match self {
            ExecutionStatus::Loading { timestamp, .. } => *timestamp,
            ExecutionStatus::Running { timestamp, .. } => *timestamp,
            ExecutionStatus::Suspended { timestamp, .. } => *timestamp,
            ExecutionStatus::Interrupting { timestamp, .. } => *timestamp,
        }
    }

    pub fn component_type(&self) -> ComponentType {
        match self {
            ExecutionStatus::Loading { component_type, .. } => *component_type,
            ExecutionStatus::Running { component_type, .. } => *component_type,
            ExecutionStatus::Suspended { component_type, .. } => *component_type,
            ExecutionStatus::Interrupting { component_type, .. } => *component_type,
        }
    }

    pub fn set_component_type(&mut self, new_component_type: ComponentType) {
        match self {
            ExecutionStatus::Loading { component_type, .. } => *component_type = new_component_type,
            ExecutionStatus::Running { component_type, .. } => *component_type = new_component_type,
            ExecutionStatus::Suspended { component_type, .. } => {
                *component_type = new_component_type
            }
            ExecutionStatus::Interrupting { component_type, .. } => {
                *component_type = new_component_type
            }
        }
    }
}

/// Describes the various reasons a worker can run into a trap
#[derive(Clone, Debug)]
pub enum TrapType {
    /// Interrupted through Golem (including user interrupts, suspends, jumps, etc.)
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
                    _ => match error.root_cause().downcast_ref::<WorkerOutOfMemory>() {
                        Some(_) => TrapType::Error(WorkerError::OutOfMemory),
                        None => match error.root_cause().downcast_ref::<GolemError>() {
                            Some(GolemError::InvalidRequest { details }) => {
                                TrapType::Error(WorkerError::InvalidRequest(details.clone()))
                            }
                            _ => TrapType::Error(WorkerError::Unknown(format!("{:#}", error))),
                        },
                    },
                },
            },
        }
    }

    pub fn as_golem_error(&self, error_logs: &str) -> Option<GolemError> {
        match self {
            TrapType::Interrupt(InterruptKind::Interrupt) => {
                Some(GolemError::runtime("Interrupted via the Golem API"))
            }
            TrapType::Error(error) => match error {
                WorkerError::InvalidRequest(msg) => Some(GolemError::invalid_request(msg.clone())),
                _ => Some(GolemError::runtime(error.to_string(error_logs))),
            },
            TrapType::Exit => Some(GolemError::runtime("Process exited")),
            _ => None,
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
    pub stderr: String,
    pub retry_count: u64,
}

impl Display for LastError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, retried {} times",
            self.error.to_string(&self.stderr),
            self.retry_count
        )
    }
}

#[derive(Copy, Clone, Debug, PartialOrd, PartialEq)]
pub enum PersistenceLevel {
    PersistNothing,
    PersistRemoteSideEffects,
    Smart,
}

impl From<crate::preview2::golem::api0_2_0::host::PersistenceLevel> for PersistenceLevel {
    fn from(value: crate::preview2::golem::api0_2_0::host::PersistenceLevel) -> Self {
        match value {
            crate::preview2::golem::api0_2_0::host::PersistenceLevel::PersistNothing => {
                PersistenceLevel::PersistNothing
            }
            crate::preview2::golem::api0_2_0::host::PersistenceLevel::PersistRemoteSideEffects => {
                PersistenceLevel::PersistRemoteSideEffects
            }
            crate::preview2::golem::api0_2_0::host::PersistenceLevel::Smart => {
                PersistenceLevel::Smart
            }
        }
    }
}

impl From<PersistenceLevel> for crate::preview2::golem::api0_2_0::host::PersistenceLevel {
    fn from(value: PersistenceLevel) -> Self {
        match value {
            PersistenceLevel::PersistNothing => {
                crate::preview2::golem::api0_2_0::host::PersistenceLevel::PersistNothing
            }
            PersistenceLevel::PersistRemoteSideEffects => {
                crate::preview2::golem::api0_2_0::host::PersistenceLevel::PersistRemoteSideEffects
            }
            PersistenceLevel::Smart => {
                crate::preview2::golem::api0_2_0::host::PersistenceLevel::Smart
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LookupResult {
    New,
    Pending,
    Interrupted,
    Complete(Result<TypeAnnotatedValue, GolemError>),
}

#[derive(Clone, Debug)]
pub enum ListDirectoryResult {
    Ok(Vec<ComponentFileSystemNode>),
    NotFound,
    NotADirectory,
}

pub enum ReadFileResult {
    Ok(Pin<Box<dyn Stream<Item = Result<Bytes, GolemError>> + Send + 'static>>),
    NotFound,
    NotAFile,
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use uuid::Uuid;

    use golem_common::model::ComponentId;

    use super::*;

    #[test]
    fn test_hash() {
        let uuid = Uuid::parse_str("96c12379-4fff-4fa2-aa09-a4d96c029ac2").unwrap();

        let component_id = ComponentId(uuid);
        let worker_id = WorkerId {
            component_id,
            worker_name: "instanceName".to_string(),
        };
        let hash = ShardId::hash_worker_id(&worker_id);
        println!("hash: {:?}", hash);
        assert_eq!(hash, -6692039695739768661);
    }
}
