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
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId, TraceId,
};
use golem_common::model::oplog::WorkerError;
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    ComponentFileSystemNode, ComponentType, ShardAssignment, ShardId, Timestamp, WorkerId,
    WorkerStatusRecord,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use nonempty_collections::NEVec;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use tracing::warn;
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

impl From<crate::preview2::golem_api_0_2_x::host::PersistenceLevel> for PersistenceLevel {
    fn from(value: crate::preview2::golem_api_0_2_x::host::PersistenceLevel) -> Self {
        match value {
            crate::preview2::golem_api_0_2_x::host::PersistenceLevel::PersistNothing => {
                PersistenceLevel::PersistNothing
            }
            crate::preview2::golem_api_0_2_x::host::PersistenceLevel::PersistRemoteSideEffects => {
                PersistenceLevel::PersistRemoteSideEffects
            }
            crate::preview2::golem_api_0_2_x::host::PersistenceLevel::Smart => {
                PersistenceLevel::Smart
            }
        }
    }
}

impl From<PersistenceLevel> for crate::preview2::golem_api_0_2_x::host::PersistenceLevel {
    fn from(value: PersistenceLevel) -> Self {
        match value {
            PersistenceLevel::PersistNothing => {
                crate::preview2::golem_api_0_2_x::host::PersistenceLevel::PersistNothing
            }
            PersistenceLevel::PersistRemoteSideEffects => {
                crate::preview2::golem_api_0_2_x::host::PersistenceLevel::PersistRemoteSideEffects
            }
            PersistenceLevel::Smart => {
                crate::preview2::golem_api_0_2_x::host::PersistenceLevel::Smart
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

pub struct InvocationContext {
    pub trace_id: TraceId,
    pub spans: HashMap<SpanId, Arc<InvocationContextSpan>>,
    pub root: Arc<InvocationContextSpan>,
    pub trace_states: Vec<String>,
}

impl InvocationContext {
    pub fn new(trace_id: Option<TraceId>) -> Self {
        let trace_id = trace_id.unwrap_or(TraceId::generate());
        let root = InvocationContextSpan::new(None);
        let mut spans = HashMap::new();
        spans.insert(root.span_id().clone(), root.clone());
        Self {
            trace_id,
            spans,
            root,
            trace_states: Vec::new(),
        }
    }

    pub fn from_stack(value: InvocationContextStack) -> Result<(Self, SpanId), String> {
        let root = value.spans.last().clone();
        let current_span_id = value.spans.first().span_id().clone();

        let mut spans = HashMap::new();
        for span in value.spans {
            spans.insert(span.span_id().clone(), span);
        }

        let result = Self {
            trace_id: value.trace_id,
            spans,
            root,
            trace_states: value.trace_states,
        };
        warn!("Initialized invocation context from stack: {result:?}, current span id: {current_span_id}");

        Ok((result, current_span_id))
    }

    /// Switch to the new invocation context but keep the existing open spans
    pub fn switch_to(&mut self, new_invocation_context: InvocationContext) {
        self.trace_id = new_invocation_context.trace_id;
        self.trace_states = new_invocation_context.trace_states;

        let root_span_id = new_invocation_context.root.span_id();
        let mut reassigned = HashSet::new();
        let mut to_update = Vec::new();
        for (span_id, new_span) in &new_invocation_context.spans {
            // If we already have one of the new spans, we keep the old one and update the links
            // This can happen with circular RPC invocations.

            if !self.spans.contains_key(&span_id) {
                to_update.push(new_span.clone()); // parent reference in this span may have to be replaced
                self.spans.insert(span_id.clone(), new_span.clone());
            } else {
                reassigned.insert(span_id); // references to this span must be updated
            }
        }

        for span in to_update {
            if let Some(parent_id) = span.parent() {
                if reassigned.contains(parent_id.span_id()) {
                    let parent = self.spans.get(parent_id.span_id()).unwrap().clone();
                    span.replace_parent(Some(parent));
                }
            }
        }

        self.root = self.spans.get(root_span_id).unwrap().clone();
    }

    pub fn start_span(
        &mut self,
        current_span_id: &SpanId,
        new_span_id: Option<SpanId>,
    ) -> Result<Arc<InvocationContextSpan>, String> {
        warn!("attempting to start new span in {current_span_id}");

        let current_span = self.span(current_span_id)?;
        let span = current_span.start_span(new_span_id);
        self.spans.insert(span.span_id().clone(), span.clone());
        warn!("started new span {} in {current_span_id}", span.span_id());
        Ok(span)
    }

    pub fn finish_span(&mut self, span_id: &SpanId) -> Result<Option<SpanId>, String> {
        warn!("finish span {span_id}");
        let span = self.span(span_id)?;
        let parent_id = span
            .parent()
            .as_ref()
            .map(|parent| parent.span_id().clone());
        self.spans.remove(span_id);
        Ok(parent_id)
    }

    pub fn get_attribute(
        &self,
        span_id: &SpanId,
        key: &str,
        inherit: bool,
    ) -> Result<Option<AttributeValue>, String> {
        let span = self.span(span_id)?;
        Ok(span.get_attribute(key, inherit))
    }

    pub fn get_attribute_chain(
        &self,
        span_id: &SpanId,
        key: &str,
    ) -> Result<Option<Vec<AttributeValue>>, String> {
        let span = self.span(span_id)?;
        Ok(span.get_attribute_chain(key))
    }

    pub fn get_attributes(
        &self,
        span_id: &SpanId,
        inherit: bool,
    ) -> Result<HashMap<String, Vec<AttributeValue>>, String> {
        let span = self.span(span_id)?;
        Ok(span.get_attributes(inherit))
    }

    pub fn set_attribute(
        &self,
        span_id: &SpanId,
        key: String,
        value: AttributeValue,
    ) -> Result<(), String> {
        let span = self.span(span_id)?;
        span.set_attribute(key, value);
        Ok(())
    }

    pub fn get_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        let mut result = Vec::new();
        let mut current = self.span(current_span_id).unwrap().clone();
        loop {
            result.push(current.clone());
            match current.parent() {
                Some(parent) => {
                    current = parent;
                }
                None => break,
            }
        }
        InvocationContextStack {
            trace_id: self.trace_id.clone(),
            spans: NEVec::try_from_vec(result).unwrap(), // result is always non-empty
            trace_states: self.trace_states.clone(),
        }
    }

    fn span(&self, span_id: &SpanId) -> Result<&Arc<InvocationContextSpan>, String> {
        self.spans
            .get(span_id)
            .ok_or_else(|| format!("Span {span_id} not found"))
    }
}

impl Debug for InvocationContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "InvocationContext trace_id={}", self.trace_id)?;
        writeln!(f, "  root span id={}", self.root.span_id())?;
        for span in self.spans.values() {
            writeln!(
                f,
                "  span {} parent={}: {}",
                span.span_id(),
                span.parent()
                    .map(|parent| parent.span_id().to_string())
                    .unwrap_or("none".to_string()),
                span.get_attributes(true)
                    .iter()
                    .map(|(key, values)| format!(
                        "{key}=[{}]",
                        values
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
        Ok(())
    }
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
