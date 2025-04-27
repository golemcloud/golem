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

use crate::durable_host::DurableWorkerCtx;
use crate::error::GolemError;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::golem::durability::durability;
use crate::preview2::golem::durability::durability::{
    PersistedTypedDurableFunctionInvocation, Pollable,
};
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use golem_common::model::oplog::{DurableFunctionType, OplogEntry, OplogIndex, PersistenceLevel};
use golem_common::model::Timestamp;
use golem_common::serialization::{deserialize, serialize, try_deserialize};
use golem_wasm_rpc::{IntoValue, IntoValueAndType, ValueAndType};
use std::fmt::Debug;
use std::marker::PhantomData;
use tracing::error;
use wasmtime::component::Resource;
use wasmtime_wasi::{dynamic_subscribe, DynamicSubscribe, Subscribe};

#[derive(Debug)]
pub struct DurableExecutionState {
    pub is_live: bool,
    pub persistence_level: PersistenceLevel,
    pub snapshotting_mode: Option<PersistenceLevel>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PersistedDurableFunctionInvocation {
    timestamp: Timestamp,
    function_name: String,
    response: Vec<u8>,
    function_type: DurableFunctionType,
    oplog_entry_version: OplogEntryVersion,
}

impl PersistedDurableFunctionInvocation {
    pub fn response_as_value_and_type(&self) -> Result<ValueAndType, GolemError> {
        deserialize(&self.response)
            .map_err(|err| GolemError::runtime(format!("Failed to deserialize payload: {err}")))
    }
}

#[async_trait]
pub trait DurabilityHost {
    /// Observes a function call (produces logs and metrics)
    fn observe_function_call(&self, interface: &str, function: &str);

    /// Marks the beginning of a durable function.
    ///
    /// There must be a corresponding call to `end_durable_function` after the function has
    /// performed its work (it can be ended in a different context, for example after an async
    /// pollable operation has been completed)
    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, GolemError>;

    /// Marks the end of a durable function
    ///
    /// This is a pair of `begin_durable_function` and should be called after the durable function
    /// has performed and persisted or replayed its work. The `begin_index` should be the index
    /// returned by `begin_durable_function`.
    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), GolemError>;

    /// Gets the current durable execution state
    fn durable_execution_state(&self) -> DurableExecutionState;

    /// Writes a record to the worker's oplog representing a durable function invocation
    async fn persist_durable_function_invocation(
        &self,
        function_name: String,
        request: &[u8],
        response: &[u8],
        function_type: DurableFunctionType,
    );

    /// Writes a record to the worker's oplog representing a durable function invocation
    ///
    /// The request and response are defined as pairs of value and type, which makes it
    /// self-describing for observers of oplogs. This is the recommended way to persist
    /// third-party function invocations.
    async fn persist_typed_durable_function_invocation(
        &self,
        function_name: String,
        request: ValueAndType,
        response: ValueAndType,
        function_type: DurableFunctionType,
    );

    /// Reads the next persisted durable function invocation from the oplog during replay
    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<PersistedDurableFunctionInvocation, GolemError>;
}

impl From<durability::DurableFunctionType> for DurableFunctionType {
    fn from(value: durability::DurableFunctionType) -> Self {
        match value {
            durability::DurableFunctionType::WriteRemote => DurableFunctionType::WriteRemote,
            durability::DurableFunctionType::WriteLocal => DurableFunctionType::WriteLocal,
            durability::DurableFunctionType::WriteRemoteBatched(oplog_index) => {
                DurableFunctionType::WriteRemoteBatched(oplog_index.map(OplogIndex::from_u64))
            }
            durability::DurableFunctionType::ReadRemote => DurableFunctionType::ReadRemote,
            durability::DurableFunctionType::ReadLocal => DurableFunctionType::ReadLocal,
        }
    }
}

impl From<DurableFunctionType> for durability::DurableFunctionType {
    fn from(value: DurableFunctionType) -> Self {
        match value {
            DurableFunctionType::WriteRemote => durability::DurableFunctionType::WriteRemote,
            DurableFunctionType::WriteLocal => durability::DurableFunctionType::WriteLocal,
            DurableFunctionType::WriteRemoteBatched(oplog_index) => {
                durability::DurableFunctionType::WriteRemoteBatched(
                    oplog_index.map(|idx| idx.into()),
                )
            }
            DurableFunctionType::ReadRemote => durability::DurableFunctionType::ReadRemote,
            DurableFunctionType::ReadLocal => durability::DurableFunctionType::ReadLocal,
        }
    }
}

impl From<OplogEntryVersion> for durability::OplogEntryVersion {
    fn from(value: OplogEntryVersion) -> Self {
        match value {
            OplogEntryVersion::V1 => durability::OplogEntryVersion::V1,
            OplogEntryVersion::V2 => durability::OplogEntryVersion::V2,
        }
    }
}

impl From<PersistedDurableFunctionInvocation> for durability::PersistedDurableFunctionInvocation {
    fn from(value: PersistedDurableFunctionInvocation) -> Self {
        durability::PersistedDurableFunctionInvocation {
            timestamp: value.timestamp.into(),
            function_name: value.function_name,
            response: value.response,
            function_type: value.function_type.into(),
            entry_version: value.oplog_entry_version.into(),
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> durability::HostLazyInitializedPollable for DurableWorkerCtx<Ctx> {
    async fn new(&mut self) -> anyhow::Result<Resource<LazyInitializedPollableEntry>> {
        DurabilityHost::observe_function_call(self, "durability::lazy_initialized_pollable", "new");
        let lazy_pollable = self.table().push(LazyInitializedPollableEntry::Empty)?;
        Ok(lazy_pollable)
    }

    async fn set(
        &mut self,
        self_: Resource<LazyInitializedPollableEntry>,
        pollable: Resource<Pollable>,
    ) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "durability::lazy_initialized_pollable", "set");
        let entry = self.table().get_mut(&self_)?;
        *entry = LazyInitializedPollableEntry::Subscribed { pollable };
        Ok(())
    }

    async fn subscribe(
        &mut self,
        self_: Resource<LazyInitializedPollableEntry>,
    ) -> anyhow::Result<Resource<Pollable>> {
        DurabilityHost::observe_function_call(
            self,
            "durability::lazy_initialized_pollable",
            "subscribe",
        );

        dynamic_subscribe(self.table(), self_, None)
    }

    async fn drop(&mut self, rep: Resource<LazyInitializedPollableEntry>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(
            self,
            "durability::lazy_initialized_pollable",
            "drop",
        );

        let entry = self.table().delete(rep)?;
        if let LazyInitializedPollableEntry::Subscribed { pollable } = entry {
            let _ = self.table().delete(pollable)?;
        }

        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> durability::Host for DurableWorkerCtx<Ctx> {
    async fn observe_function_call(
        &mut self,
        iface: String,
        function: String,
    ) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, &iface, &function);
        Ok(())
    }

    async fn begin_durable_function(
        &mut self,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<durability::OplogIndex> {
        let oplog_idx = DurabilityHost::begin_durable_function(self, &function_type.into()).await?;
        Ok(oplog_idx.into())
    }

    async fn end_durable_function(
        &mut self,
        function_type: durability::DurableFunctionType,
        begin_index: durability::OplogIndex,
        forced_commit: bool,
    ) -> anyhow::Result<()> {
        DurabilityHost::end_durable_function(
            self,
            &function_type.into(),
            OplogIndex::from_u64(begin_index),
            forced_commit,
        )
        .await?;
        Ok(())
    }

    async fn current_durable_execution_state(
        &mut self,
    ) -> anyhow::Result<durability::DurableExecutionState> {
        let state = DurabilityHost::durable_execution_state(self);
        Ok(durability::DurableExecutionState {
            is_live: state.is_live,
            persistence_level: match state.persistence_level {
                PersistenceLevel::PersistNothing => durability::PersistenceLevel::PersistNothing,
                PersistenceLevel::PersistRemoteSideEffects => {
                    durability::PersistenceLevel::PersistRemoteSideEffects
                }
                PersistenceLevel::Smart => durability::PersistenceLevel::Smart,
            },
        })
    }

    async fn persist_durable_function_invocation(
        &mut self,
        function_name: String,
        request: Vec<u8>,
        response: Vec<u8>,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<()> {
        DurabilityHost::persist_durable_function_invocation(
            self,
            function_name,
            &request,
            &response,
            function_type.into(),
        )
        .await;
        Ok(())
    }

    async fn persist_typed_durable_function_invocation(
        &mut self,
        function_name: String,
        request: durability::ValueAndType,
        response: durability::ValueAndType,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<()> {
        DurabilityHost::persist_typed_durable_function_invocation(
            self,
            function_name,
            request.into(),
            response.into(),
            function_type.into(),
        )
        .await;
        Ok(())
    }

    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> anyhow::Result<durability::PersistedDurableFunctionInvocation> {
        let invocation = DurabilityHost::read_persisted_durable_function_invocation(self).await?;
        Ok(invocation.into())
    }

    async fn read_persisted_typed_durable_function_invocation(
        &mut self,
    ) -> anyhow::Result<PersistedTypedDurableFunctionInvocation> {
        let invocation = DurabilityHost::read_persisted_durable_function_invocation(self).await?;
        let response = invocation.response_as_value_and_type()?;
        let untyped: durability::PersistedDurableFunctionInvocation = invocation.into();
        Ok(PersistedTypedDurableFunctionInvocation {
            timestamp: untyped.timestamp,
            function_name: untyped.function_name,
            response: response.into(),
            function_type: untyped.function_type,
            entry_version: untyped.entry_version,
        })
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> DurabilityHost for DurableWorkerCtx<Ctx> {
    fn observe_function_call(&self, interface: &str, function: &str) {
        record_host_function_call(interface, function);
    }

    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, GolemError> {
        self.state.begin_function(function_type).await
    }

    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), GolemError> {
        self.state.end_function(function_type, begin_index).await?;
        if function_type == &DurableFunctionType::WriteRemote
            || matches!(function_type, DurableFunctionType::WriteRemoteBatched(_))
            || forced_commit
        {
            self.state.oplog.commit(CommitLevel::DurableOnly).await;
        }
        Ok(())
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: self.state.is_live(),
            persistence_level: self.state.persistence_level,
            snapshotting_mode: self.state.snapshotting_mode,
        }
    }

    async fn persist_durable_function_invocation(
        &self,
        function_name: String,
        request: &[u8],
        response: &[u8],
        function_type: DurableFunctionType,
    ) {
        self.state
            .oplog
            .add_raw_imported_function_invoked(function_name, request, response, function_type)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to serialize and store durable function invocation: {err}")
            });
    }

    async fn persist_typed_durable_function_invocation(
        &self,
        function_name: String,
        request: ValueAndType,
        response: ValueAndType,
        function_type: DurableFunctionType,
    ) {
        let request = serialize(&request).unwrap_or_else(|err| {
                panic!("failed to serialize request ({request:?}) for persisting durable function invocation: {err}")
            }).to_vec();
        let response = serialize(&response).unwrap_or_else(|err| {
                panic!("failed to serialize response ({response:?}) for persisting durable function invocation: {err}")
            }).to_vec();

        self.state
            .oplog
            .add_raw_imported_function_invoked(function_name, &request, &response, function_type)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to serialize and store durable function invocation: {err}")
            });
    }

    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<PersistedDurableFunctionInvocation, GolemError> {
        if self.state.persistence_level == PersistenceLevel::PersistNothing {
            Err(GolemError::runtime(
                "Trying to replay an durable invocation in a PersistNothing block",
            ))
        } else {
            let (_, oplog_entry) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::ImportedFunctionInvoked,
                OplogEntry::ImportedFunctionInvokedV1
            )?;

            let bytes = self
                .state
                .oplog
                .get_raw_payload_of_entry(&oplog_entry)
                .await
                .map_err(|err| {
                    GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
                })?
                .unwrap();

            match oplog_entry {
                OplogEntry::ImportedFunctionInvoked {
                    timestamp,
                    function_name,
                    wrapped_function_type,
                    ..
                } => Ok(PersistedDurableFunctionInvocation {
                    timestamp,
                    function_name,
                    response: bytes.to_vec(),
                    function_type: wrapped_function_type,
                    oplog_entry_version: OplogEntryVersion::V2,
                }),
                OplogEntry::ImportedFunctionInvokedV1 {
                    timestamp,
                    function_name,
                    wrapped_function_type,
                    ..
                } => Ok(PersistedDurableFunctionInvocation {
                    timestamp,
                    function_name,
                    response: bytes.to_vec(),
                    function_type: wrapped_function_type,
                    oplog_entry_version: OplogEntryVersion::V1,
                }),
                _ => Err(GolemError::unexpected_oplog_entry(
                    "ImportedFunctionInvoked",
                    format!("{:?}", oplog_entry),
                )),
            }
        }
    }
}

#[derive(Debug)]
pub enum OplogEntryVersion {
    V1,
    V2,
}

pub struct Durability<SOk, SErr> {
    interface: &'static str,
    function: &'static str,
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    durable_execution_state: DurableExecutionState,
    _sok: PhantomData<SOk>,
    _serr: PhantomData<SErr>,
}

impl<SOk, SErr> Durability<SOk, SErr> {
    pub async fn new(
        ctx: &mut impl DurabilityHost,
        interface: &'static str,
        function: &'static str,
        function_type: DurableFunctionType,
    ) -> Result<Self, GolemError> {
        ctx.observe_function_call(interface, function);

        let begin_index = ctx.begin_durable_function(&function_type).await?;
        let durable_execution_state = ctx.durable_execution_state();

        Ok(Self {
            interface,
            function,
            function_type,
            begin_index,
            durable_execution_state,
            _sok: PhantomData,
            _serr: PhantomData,
        })
    }

    pub fn is_live(&self) -> bool {
        self.durable_execution_state.is_live
    }

    pub async fn persist<SIn, Ok, Err>(
        &self,
        ctx: &mut impl DurabilityHost,
        input: SIn,
        result: Result<Ok, Err>,
    ) -> Result<Ok, Err>
    where
        Ok: Clone,
        Err: From<SErr> + From<GolemError> + Send + Sync,
        SIn: Debug + Encode + Send + Sync,
        SErr: Debug + Encode + for<'a> From<&'a Err> + From<GolemError> + Send + Sync,
        SOk: Debug + Encode + From<Ok> + Send + Sync,
    {
        let serializable_result: Result<SOk, SErr> = result
            .as_ref()
            .map(|result| result.clone().into())
            .map_err(|err| err.into());

        self.persist_serializable(ctx, input, serializable_result)
            .await
            .map_err(|err| {
                let err: SErr = err.into();
                let err: Err = err.into();
                err
            })?;
        result
    }

    pub async fn persist_serializable<SIn>(
        &self,
        ctx: &mut impl DurabilityHost,
        input: SIn,
        result: Result<SOk, SErr>,
    ) -> Result<(), GolemError>
    where
        SIn: Debug + Encode + Send + Sync,
        SOk: Debug + Encode + Send + Sync,
        SErr: Debug + Encode + Send + Sync,
    {
        if self.durable_execution_state.snapshotting_mode.is_none() {
            let function_name = self.function_name();
            let serialized_input = serialize(&input).unwrap_or_else(|err| {
                panic!("failed to serialize input ({input:?}) for persisting durable function invocation: {err}")
            }).to_vec();
            let serialized_result = serialize(&result).unwrap_or_else(|err| {
                panic!("failed to serialize result ({result:?}) for persisting durable function invocation: {err}")
            }).to_vec();

            ctx.persist_durable_function_invocation(
                function_name.to_string(),
                &serialized_input,
                &serialized_result,
                self.function_type.clone(),
            )
            .await;
            ctx.end_durable_function(&self.function_type, self.begin_index, false)
                .await?;
        }
        Ok(())
    }

    pub async fn persist_typed_value<SIn>(
        &self,
        ctx: &mut impl DurabilityHost,
        input: SIn,
        result: Result<SOk, SErr>,
    ) -> Result<(), GolemError>
    where
        SIn: Debug + IntoValue + Send + Sync,
        SOk: Debug + IntoValue + Send + Sync,
        SErr: Debug + IntoValue + Send + Sync,
    {
        if self.durable_execution_state.snapshotting_mode.is_none() {
            let function_name = self.function_name();
            let input_value = input.into_value_and_type();
            let result_value = result.into_value_and_type();

            ctx.persist_typed_durable_function_invocation(
                function_name.to_string(),
                input_value,
                result_value,
                self.function_type.clone(),
            )
            .await;
            ctx.end_durable_function(&self.function_type, self.begin_index, false)
                .await?;
        }
        Ok(())
    }

    pub async fn replay_raw(
        &self,
        ctx: &mut impl DurabilityHost,
    ) -> Result<(Bytes, OplogEntryVersion), GolemError> {
        let oplog_entry = ctx.read_persisted_durable_function_invocation().await?;

        let function_name = self.function_name();
        Self::validate_oplog_entry(&oplog_entry, &function_name)?;

        ctx.end_durable_function(&self.function_type, self.begin_index, false)
            .await?;

        Ok((oplog_entry.response.into(), oplog_entry.oplog_entry_version))
    }

    pub async fn replay_serializable(
        &self,
        ctx: &mut impl DurabilityHost,
    ) -> Result<Result<SOk, SErr>, GolemError>
    where
        SOk: Decode,
        SErr: Decode,
    {
        let (bytes, _) = self.replay_raw(ctx).await?;
        let result: Result<SOk, SErr> = try_deserialize(&bytes)
            .map_err(|err| {
                GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
            })?
            .expect("Payload is empty");
        Ok(result)
    }

    pub async fn replay<Ok, Err>(&self, ctx: &mut impl DurabilityHost) -> Result<Ok, Err>
    where
        Ok: From<SOk>,
        Err: From<SErr> + From<GolemError>,
        SErr: Debug + Encode + Decode + From<GolemError> + Send + Sync,
        SOk: Debug + Encode + Decode + Send + Sync,
    {
        Self::replay_serializable(self, ctx)
            .await?
            .map(|sok| sok.into())
            .map_err(|serr| serr.into())
    }

    fn function_name(&self) -> String {
        if self.interface.is_empty() {
            // For backward compatibility - some of the recorded function names were not following the pattern
            self.function.to_string()
        } else {
            format!("{}::{}", self.interface, self.function)
        }
    }

    fn validate_oplog_entry(
        oplog_entry: &PersistedDurableFunctionInvocation,
        expected_function_name: &str,
    ) -> Result<(), GolemError> {
        if oplog_entry.function_name != expected_function_name {
            error!(
                "Unexpected imported function call entry in oplog: expected {}, got {}",
                expected_function_name, oplog_entry.function_name
            );
            Err(GolemError::unexpected_oplog_entry(
                expected_function_name,
                oplog_entry.function_name.clone(),
            ))
        } else {
            Ok(())
        }
    }
}

pub enum LazyInitializedPollableEntry {
    Empty,
    Subscribed { pollable: Resource<Pollable> },
}

#[async_trait]
impl Subscribe for LazyInitializedPollableEntry {
    async fn ready(&mut self) {
        match self {
            LazyInitializedPollableEntry::Empty => {
                // Empty pollable is always ready
            }
            LazyInitializedPollableEntry::Subscribed { .. } => {
                unreachable!("The dynamic pollable override should prevent this from being called")
            }
        }
    }
}

#[async_trait]
impl DynamicSubscribe for LazyInitializedPollableEntry {
    fn override_index(&self) -> Option<u32> {
        match self {
            LazyInitializedPollableEntry::Empty => None,
            LazyInitializedPollableEntry::Subscribed { pollable } => Some(pollable.rep()),
        }
    }
}
