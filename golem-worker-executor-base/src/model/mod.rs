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
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;
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

#[derive(Debug, Clone)]
pub struct TraceId(String);

impl TraceId {
    pub fn generate() -> Self {
        Self(format!("{:x}", Uuid::new_v4().as_u128()))
    }
}

impl Display for TraceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpanId(String);

impl SpanId {
    pub fn generate() -> Self {
        let (lo, hi) = Uuid::new_v4().as_u64_pair();
        Self(format!("{:x}", lo ^ hi))
    }
}

impl Display for SpanId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub enum AttributeValue {
    String(String),
}

impl From<AttributeValue> for golem_api_grpc::proto::golem::worker::AttributeValue {
    fn from(value: AttributeValue) -> Self {
        match value {
            AttributeValue::String(value) => Self {
                value: Some(
                    golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                        value,
                    ),
                ),
            },
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::AttributeValue> for AttributeValue {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AttributeValue,
    ) -> Result<Self, Self::Error> {
        match value.value {
            Some(golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                value,
            )) => Ok(Self::String(value)),
            _ => Err("Invalid attribute value".to_string()),
        }
    }
}

#[derive(Clone)]
pub struct FlatInvocationContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub start: Timestamp,
    pub attributes: HashMap<String, AttributeValue>,
}

impl From<FlatInvocationContext> for golem_api_grpc::proto::golem::worker::InvocationSpan {
    fn from(value: FlatInvocationContext) -> Self {
        let mut attributes = HashMap::new();
        for (key, value) in value.attributes {
            attributes.insert(key, value.into());
        }
        Self {
            trace_id: value.trace_id.0,
            span_id: value.span_id.0,
            start: Some(value.start.into()),
            attributes,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::InvocationSpan> for FlatInvocationContext {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::InvocationSpan,
    ) -> Result<Self, Self::Error> {
        let mut attributes = HashMap::new();
        for (key, value) in value.attributes {
            attributes.insert(key, value.try_into()?);
        }
        Ok(Self {
            trace_id: TraceId(value.trace_id),
            span_id: SpanId(value.span_id),
            start: value.start.ok_or_else(|| "Missing timestamp".to_string())?.into(),
            attributes,
        })
    }
}

pub struct InvocationContextSpan {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent: Option<Arc<InvocationContextSpan>>,
    pub start: Timestamp,
    attributes: RwLock<HashMap<String, AttributeValue>>,
}

impl InvocationContextSpan {
    fn new(trace_id: Option<TraceId>, span_id: Option<SpanId>) -> Arc<Self> {
        let trace_id = trace_id.unwrap_or(TraceId::generate());
        let span_id = span_id.unwrap_or(SpanId::generate());
        Arc::new(Self {
            trace_id,
            span_id,
            parent: None,
            start: Timestamp::now_utc(),
            attributes: RwLock::new(HashMap::new()),
        })
    }

    fn start_span(self: &Arc<Self>, span_id: Option<SpanId>) -> Arc<Self> {
        Self::new(Some(self.trace_id.clone()), span_id)
    }

    async fn get_attribute(&self, key: &str) -> Option<AttributeValue> {
        let mut current = self;
        loop {
            let attributes = current.attributes.read().await;
            match attributes.get(key) {
                Some(value) => break Some(value.clone()),
                None => match current.parent.as_ref() {
                    Some(parent) => {
                        current = parent;
                    }
                    None => break None,
                },
            }
        }
    }

    async fn get_attributes(&self) -> HashMap<String, AttributeValue> {
        let flattened = self.flatten().await;
        flattened.attributes
    }

    async fn set_attribute(&self, key: String, value: AttributeValue) {
        self.attributes.write().await.insert(key, value);
    }

    async fn flatten(&self) -> FlatInvocationContext {
        let mut flattened = FlatInvocationContext {
            trace_id: self.trace_id.clone(),
            span_id: self.span_id.clone(),
            start: self.start,
            attributes: HashMap::new(),
        };
        self.flatten_to(&mut flattened).await;
        flattened
    }

    async fn flatten_to(&self, flattened: &mut FlatInvocationContext) {
        let mut current = self;
        loop {
            let attributes = current.attributes.read().await;
            for (key, value) in &*attributes {
                if !flattened.attributes.contains_key(key) {
                    flattened.attributes.insert(key.clone(), value.clone());
                }
            }
            if let Some(parent) = &current.parent {
                current = parent;
            } else {
                break;
            }
        }
    }
}

pub struct InvocationContext {
    pub trace_id: TraceId,
    pub spans: HashMap<SpanId, Arc<InvocationContextSpan>>,
    pub root: Arc<InvocationContextSpan>,
}

impl InvocationContext {
    pub fn start_span(
        &mut self,
        current_span_id: SpanId,
        new_span_id: Option<SpanId>,
    ) -> Result<Arc<InvocationContextSpan>, String> {
        let current_span = self.span(current_span_id)?;
        let span = current_span.start_span(new_span_id);
        self.spans.insert(span.span_id.clone(), span.clone());
        Ok(span)
    }

    pub fn finish_span(&mut self, span_id: SpanId) -> Result<Option<SpanId>, String> {
        let span = self.span(span_id.clone())?;
        let parent_id = span.parent.as_ref().map(|parent| parent.span_id.clone());
        self.spans.remove(&span_id);
        Ok(parent_id)
    }

    pub async fn get_attribute(
        &self,
        span_id: SpanId,
        key: &str,
    ) -> Result<Option<AttributeValue>, String> {
        let span = self.span(span_id)?;
        Ok(span.get_attribute(key).await)
    }

    pub async fn get_attributes(
        &self,
        span_id: SpanId,
    ) -> Result<HashMap<String, AttributeValue>, String> {
        let span = self.span(span_id)?;
        Ok(span.get_attributes().await)
    }

    pub async fn set_attribute(
        &self,
        span_id: SpanId,
        key: String,
        value: AttributeValue,
    ) -> Result<(), String> {
        let span = self.span(span_id)?;
        span.set_attribute(key, value).await;
        Ok(())
    }

    pub async fn flatten(&self, span_id: SpanId) -> Result<FlatInvocationContext, String> {
        let span = self.span(span_id)?;
        Ok(span.flatten().await)
    }

    fn span(&self, span_id: SpanId) -> Result<&Arc<InvocationContextSpan>, String> {
        Ok(self
            .spans
            .get(&span_id)
            .ok_or_else(|| format!("Span {span_id} not found"))?)
    }
}

impl From<FlatInvocationContext> for InvocationContext {
    fn from(flat: FlatInvocationContext) -> Self {
        let mut spans = HashMap::new();
        let root = Arc::new(InvocationContextSpan {
            trace_id: flat.trace_id.clone(),
            span_id: flat.span_id.clone(),
            parent: None,
            start: flat.start,
            attributes: RwLock::new(flat.attributes),
        });
        spans.insert(root.span_id.clone(), root.clone());
        Self {
            trace_id: flat.trace_id,
            spans,
            root,
        }
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
