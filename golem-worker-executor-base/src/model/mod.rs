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
                            Some(GolemError::ParamTypeMismatch { details }) => {
                                TrapType::Error(WorkerError::InvalidRequest(details.clone()))
                            }
                            Some(GolemError::ValueMismatch { details }) => {
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

impl From<crate::preview2::golem_api_1_x::host::PersistenceLevel> for PersistenceLevel {
    fn from(value: crate::preview2::golem_api_1_x::host::PersistenceLevel) -> Self {
        match value {
            crate::preview2::golem_api_1_x::host::PersistenceLevel::PersistNothing => {
                PersistenceLevel::PersistNothing
            }
            crate::preview2::golem_api_1_x::host::PersistenceLevel::PersistRemoteSideEffects => {
                PersistenceLevel::PersistRemoteSideEffects
            }
            crate::preview2::golem_api_1_x::host::PersistenceLevel::Smart => {
                PersistenceLevel::Smart
            }
        }
    }
}

impl From<PersistenceLevel> for crate::preview2::golem_api_1_x::host::PersistenceLevel {
    fn from(value: PersistenceLevel) -> Self {
        match value {
            PersistenceLevel::PersistNothing => {
                crate::preview2::golem_api_1_x::host::PersistenceLevel::PersistNothing
            }
            PersistenceLevel::PersistRemoteSideEffects => {
                crate::preview2::golem_api_1_x::host::PersistenceLevel::PersistRemoteSideEffects
            }
            PersistenceLevel::Smart => {
                crate::preview2::golem_api_1_x::host::PersistenceLevel::Smart
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
        let root = InvocationContextSpan::local().build();
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

            if !self.spans.contains_key(span_id) {
                to_update.push(new_span.clone()); // parent reference in this span may have to be replaced
                self.spans.insert(span_id.clone(), new_span.clone());
            } else {
                reassigned.insert(span_id); // references to this span must be updated
            }
        }

        for span in to_update {
            if let Some(parent) = span.parent() {
                if reassigned.contains(parent.span_id()) {
                    let parent = self.spans.get(parent.span_id()).unwrap().clone();
                    span.replace_parent(Some(parent));
                }
            }
            if let Some(linked_context) = span.linked_context() {
                if reassigned.contains(linked_context.span_id()) {
                    let linked_context = self.spans.get(linked_context.span_id()).unwrap().clone();
                    span.add_link(linked_context);
                }
            }
        }

        self.root = self.spans.get(root_span_id).unwrap().clone();
    }

    /// Checks whether the span given by `look_for` is a member of the invocation context stack
    /// starting from the span given by `current_span_id`.
    ///
    /// Linked span contexts are also taken into account.
    pub fn has_in_stack(&self, current_span_id: &SpanId, look_for: &SpanId) -> bool {
        let mut linked = Vec::new();
        let mut current = self.span(current_span_id).unwrap().clone();
        loop {
            let result = loop {
                if current.span_id() == look_for {
                    break true;
                }
                if let Some(linked_context) = current.linked_context() {
                    linked.push(linked_context.clone());
                }
                match current.parent() {
                    Some(parent) => {
                        current = parent;
                    }
                    None => break false,
                }
            };

            if !result {
                if let Some(linked_context) = linked.pop() {
                    current = linked_context;
                } else {
                    break false;
                }
            } else {
                break result;
            }
        }
    }

    pub fn get(&self, span_id: &SpanId) -> Result<Arc<InvocationContextSpan>, String> {
        Ok(self.span(span_id)?.clone())
    }

    pub fn start_span(
        &mut self,
        current_span_id: &SpanId,
        new_span_id: Option<SpanId>,
    ) -> Result<Arc<InvocationContextSpan>, String> {
        let current_span = self.span(current_span_id)?;
        let span = current_span.start_span(new_span_id);
        self.add_span(span.clone());
        Ok(span)
    }

    pub fn add_span(&mut self, span: Arc<InvocationContextSpan>) {
        self.spans.insert(span.span_id().clone(), span);
    }

    pub fn finish_span(&mut self, span_id: &SpanId) -> Result<Option<SpanId>, String> {
        let span = self.span(span_id)?;
        let parent_id = span
            .parent()
            .as_ref()
            .map(|parent| parent.span_id().clone());
        self.spans.remove(span_id);
        Ok(parent_id)
    }

    pub fn add_link(&self, span_id: &SpanId, target_span_id: &SpanId) -> Result<(), String> {
        let span = self.span(span_id)?;
        let target_span = self.span(target_span_id)?;
        span.add_link(target_span.clone());
        Ok(())
    }

    /// Gets the attribute value for the given key for the given span.
    ///
    /// When `inherit` is true, the attribute is looked up in the parent spans
    /// if it is not found in the current span. The first match is returned.
    ///
    /// When `inherit` is false only the current span is searched.
    ///
    /// For linked invocation contexts, if the attribute is not found
    /// in the current span and `inherit` is true, the attribute is looked
    /// up in the linked context before going up the parent chain. The linked
    /// context's parent chain is not searched.
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

    pub fn get_stack(&self, current_span_id: &SpanId) -> Result<InvocationContextStack, String> {
        let mut result = Vec::new();
        let mut current = self.span(current_span_id)?.clone();
        loop {
            result.push(current.clone());
            match current.parent() {
                Some(parent) => {
                    current = parent;
                }
                None => break,
            }
        }
        Ok(InvocationContextStack {
            trace_id: self.trace_id.clone(),
            spans: NEVec::try_from_vec(result).unwrap(), // result is always non-empty
            trace_states: self.trace_states.clone(),
        })
    }

    /// Clones every element of the stack belonging to the given current span id, and sets
    /// the inherited flag to true on them, without changing the spans in this invocation context.
    pub fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        let mut clones = HashMap::new();
        let mut result = Vec::new();
        let mut current = self.span(current_span_id).unwrap().clone();
        loop {
            let clone = current.as_inherited();
            clones.insert(clone.span_id().clone(), clone.clone());
            result.push(clone);

            match current.parent() {
                Some(parent) => {
                    current = parent;
                }
                None => break,
            }
        }
        for span in &result {
            if let Some(parent) = span.parent() {
                let parent_id = parent.span_id();
                let parent_clone = clones.get(parent_id).unwrap();
                span.replace_parent(Some(parent_clone.clone()));
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
    use super::*;
    use golem_common::model::ComponentId;
    use test_r::test;
    use tracing::info;
    use uuid::Uuid;

    fn example_trace_id_1() -> TraceId {
        TraceId::from_string("4bf92f3577b34da6a3ce929d0e0e4736").unwrap()
    }

    fn example_trace_id_2() -> TraceId {
        TraceId::from_string("4bf92f3577b34da6a3ce929d0e0e4737").unwrap()
    }

    fn example_span_id_1() -> SpanId {
        SpanId::from_string("cddd89c618fb7bf3").unwrap()
    }

    fn example_span_id_2() -> SpanId {
        SpanId::from_string("00f067aa0ba902b7").unwrap()
    }

    fn example_span_id_3() -> SpanId {
        SpanId::from_string("d0fa4a9110f2dcab").unwrap()
    }

    fn example_span_id_4() -> SpanId {
        SpanId::from_string("4a840260c6879c88").unwrap()
    }

    fn example_span_id_5() -> SpanId {
        SpanId::from_string("04d81050b3163556").unwrap()
    }

    fn example_span_id_6() -> SpanId {
        SpanId::from_string("b7027ded25941641").unwrap()
    }

    fn example_span_id_7() -> SpanId {
        SpanId::from_string("b7027ded25941642").unwrap()
    }

    fn s(s: &str) -> AttributeValue {
        AttributeValue::String(s.to_string())
    }

    // span1 -> span2 -> span5 -> span6
    // span3 -> span4 /
    fn example_stack_1() -> InvocationContextStack {
        let timestamp = Timestamp::from(1724701930000);

        let root_span = InvocationContextSpan::external_parent(example_span_id_1());
        let trace_states = vec!["state1=x".to_string(), "state2=y".to_string()];

        let span2 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_2())
            .with_parent(root_span.clone())
            .with_inherited(true)
            .build();
        span2.set_attribute("x".to_string(), AttributeValue::String("1".to_string()));
        span2.set_attribute("y".to_string(), AttributeValue::String("2".to_string()));

        let span3 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_3())
            .build();
        span3.set_attribute("w".to_string(), AttributeValue::String("4".to_string()));

        let span4 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_4())
            .with_parent(span3)
            .build();
        span4.set_attribute("y".to_string(), AttributeValue::String("22".to_string()));

        let span5 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_5())
            .with_parent(span2.clone())
            .with_linked_context(span4)
            .build();
        span5.set_attribute("x".to_string(), AttributeValue::String("11".to_string()));
        span5.set_attribute("z".to_string(), AttributeValue::String("3".to_string()));

        let span6 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_6())
            .with_parent(span5.clone())
            .build();
        span6.set_attribute("z".to_string(), AttributeValue::String("33".to_string()));
        span6.set_attribute("a".to_string(), AttributeValue::String("0".to_string()));

        let mut stack = InvocationContextStack::new(example_trace_id_1(), root_span, trace_states);
        stack.push(span2);
        stack.push(span5);
        stack.push(span6);

        stack
    }

    #[test]
    fn test_hash() {
        let uuid = Uuid::parse_str("96c12379-4fff-4fa2-aa09-a4d96c029ac2").unwrap();

        let component_id = ComponentId(uuid);
        let worker_id = WorkerId {
            component_id,
            worker_name: "instanceName".to_string(),
        };
        let hash = ShardId::hash_worker_id(&worker_id);
        info!("hash: {:?}", hash);
        assert_eq!(hash, -6692039695739768661);
    }

    #[test]
    fn has_in_stack() {
        let stack = example_stack_1();
        let (ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        assert!(ctx.has_in_stack(&current_id, &example_span_id_1()));
        assert!(ctx.has_in_stack(&current_id, &example_span_id_2()));
        assert!(ctx.has_in_stack(&current_id, &example_span_id_3()));
        assert!(ctx.has_in_stack(&current_id, &example_span_id_4()));
        assert!(ctx.has_in_stack(&current_id, &example_span_id_5()));
        assert!(ctx.has_in_stack(&current_id, &example_span_id_6()));
        assert!(!ctx.has_in_stack(&current_id, &example_span_id_7()));
    }

    #[test]
    fn start_span() {
        let stack = example_stack_1();
        let (mut ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let span7 = ctx
            .start_span(&current_id, Some(example_span_id_7()))
            .unwrap();
        assert_eq!(span7.span_id(), &example_span_id_7());
        assert_eq!(span7.parent().unwrap().span_id(), &example_span_id_6());
    }

    #[test]
    fn finish_span() {
        let stack = example_stack_1();
        let (mut ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let span7 = ctx
            .start_span(&current_id, Some(example_span_id_7()))
            .unwrap();
        ctx.finish_span(span7.span_id()).unwrap();
        assert!(ctx.get_stack(span7.span_id()).is_err());
    }

    #[test]
    fn get_attribute_no_inherited() {
        let stack = example_stack_1();
        let (ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let x = ctx.get_attribute(&current_id, "x", false).unwrap();
        let y = ctx.get_attribute(&current_id, "y", false).unwrap();
        let z = ctx.get_attribute(&current_id, "z", false).unwrap();
        let w = ctx.get_attribute(&current_id, "w", false).unwrap();
        let a = ctx.get_attribute(&current_id, "a", false).unwrap();

        assert_eq!(x, None);
        assert_eq!(y, None);
        assert_eq!(z, Some(AttributeValue::String("33".to_string())));
        assert_eq!(w, None);
        assert_eq!(a, Some(AttributeValue::String("0".to_string())));
    }

    #[test]
    fn get_attribute() {
        let stack = example_stack_1();
        let (ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let x = ctx.get_attribute(&current_id, "x", true).unwrap();
        let y = ctx.get_attribute(&current_id, "y", true).unwrap();
        let z = ctx.get_attribute(&current_id, "z", true).unwrap();
        let w = ctx.get_attribute(&current_id, "w", true).unwrap();
        let a = ctx.get_attribute(&current_id, "a", true).unwrap();

        assert_eq!(x, Some(s("11"))); // found in the parent chain
        assert_eq!(y, Some(s("22"))); // overridden by the linked context
        assert_eq!(z, Some(s("33"))); // found in current
        assert_eq!(w, None); // defined in the linked context's parent span, not returned here
        assert_eq!(a, Some(s("0"))); // found in current
    }

    #[test]
    fn get_attribute_chain() {
        let stack = example_stack_1();
        let (ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let x = ctx.get_attribute_chain(&current_id, "x").unwrap();
        let y = ctx.get_attribute_chain(&current_id, "y").unwrap();
        let z = ctx.get_attribute_chain(&current_id, "z").unwrap();
        let w = ctx.get_attribute_chain(&current_id, "w").unwrap();
        let a = ctx.get_attribute_chain(&current_id, "a").unwrap();

        assert_eq!(x, Some(vec![s("11"), s("1")]));
        assert_eq!(y, Some(vec![s("22"), s("2")]));
        assert_eq!(z, Some(vec![s("33"), s("3")]));
        assert_eq!(w, None);
        assert_eq!(a, Some(vec![s("0")]));
    }

    #[test]
    fn get_attributes() {
        let stack = example_stack_1();
        let (ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let attributes = ctx.get_attributes(&current_id, true).unwrap();

        assert_eq!(attributes.len(), 4);
        assert_eq!(attributes.get("x").unwrap(), &[s("11"), s("1")]);
        assert_eq!(attributes.get("y").unwrap(), &[s("22"), s("2")]);
        assert_eq!(attributes.get("z").unwrap(), &[s("33"), s("3")]);
        assert_eq!(attributes.get("a").unwrap(), &[s("0")]);
    }

    #[test]
    fn get_stack() {
        let stack = example_stack_1();
        let (ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let stack = ctx.get_stack(&current_id).unwrap();

        assert_eq!(stack.spans.len().get(), 4);
        assert_eq!(stack.spans[0].span_id(), &example_span_id_6());
        assert_eq!(stack.spans[1].span_id(), &example_span_id_5());
        assert_eq!(stack.spans[2].span_id(), &example_span_id_2());
        assert_eq!(stack.spans[3].span_id(), &example_span_id_1());
    }

    #[test]
    fn clone_as_inherited_stack() {
        let stack = example_stack_1();
        let (ctx, current_id) = InvocationContext::from_stack(stack).unwrap();

        let inherited_stack = ctx.clone_as_inherited_stack(&current_id);
        let original_stack = ctx.get_stack(&current_id).unwrap();

        assert_eq!(inherited_stack.spans.len().get(), 4);
        assert_eq!(inherited_stack.spans[0].span_id(), &example_span_id_6());
        assert!(inherited_stack.spans[0].inherited());
        assert_eq!(inherited_stack.spans[1].span_id(), &example_span_id_5());
        assert!(inherited_stack.spans[1].inherited());
        assert_eq!(inherited_stack.spans[2].span_id(), &example_span_id_2());
        assert!(inherited_stack.spans[2].inherited());
        assert_eq!(inherited_stack.spans[3].span_id(), &example_span_id_1());
        assert!(inherited_stack.spans[3].inherited());

        assert_eq!(original_stack.spans.len().get(), 4);
        assert!(!original_stack.spans[0].inherited());
        assert!(!original_stack.spans[1].inherited());
        assert!(original_stack.spans[2].inherited());
        assert!(original_stack.spans[3].inherited());
    }

    #[test]
    fn switch_to() {
        let stack1 = example_stack_1();
        let (mut ctx, _current_id) = InvocationContext::from_stack(stack1.clone()).unwrap();

        let mut stack2 = InvocationContextStack::new(
            example_trace_id_2(),
            stack1.spans.last().clone(),
            vec!["state3=z".to_string()],
        );
        let span7 = InvocationContextSpan::local()
            .with_span_id(example_span_id_7())
            .with_parent(stack1.spans.first().clone())
            .build();
        span7.set_attribute("a".to_string(), AttributeValue::String("00".to_string()));
        stack2.push(span7);

        let (ctx2, current_id2) = InvocationContext::from_stack(stack2).unwrap();
        ctx.switch_to(ctx2);

        assert_eq!(ctx.trace_id, example_trace_id_2());
        assert_eq!(ctx.trace_states, vec!["state3=z".to_string()]);
        assert_eq!(ctx.root.span_id(), &example_span_id_1());

        let x = ctx.get_attribute_chain(&current_id2, "x").unwrap();
        let y = ctx.get_attribute_chain(&current_id2, "y").unwrap();
        let z = ctx.get_attribute_chain(&current_id2, "z").unwrap();
        let w = ctx.get_attribute_chain(&current_id2, "w").unwrap();
        let a = ctx.get_attribute_chain(&current_id2, "a").unwrap();

        assert_eq!(x, Some(vec![s("11"), s("1")]));
        assert_eq!(y, Some(vec![s("22"), s("2")]));
        assert_eq!(z, Some(vec![s("33"), s("3")]));
        assert_eq!(w, None);
        assert_eq!(a, Some(vec![s("00"), s("0")]));
    }
}
