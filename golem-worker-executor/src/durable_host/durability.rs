// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::model::TrapType;
use crate::preview2::golem::durability::durability;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::{HasOplog, HasWorker};
use crate::worker::RetryDecision;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    PersistenceLevel,
};
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::IntoValueAndType;
use std::fmt::{Debug, Display};
use tracing::error;
use wasmtime::component::Resource;
use wasmtime_wasi::{dynamic_subscribe, DynPollable, DynamicPollable, Pollable};

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
    response: HostResponse,
    function_type: DurableFunctionType,
    entry_version: OplogEntryVersion,
}

#[async_trait]
pub trait DurabilityHost {
    /// Observes a function call (produces logs and metrics)
    fn observe_function_call(&self, interface: &str, function: &str);

    /// Marks the beginning of a durable function.
    ///
    /// There must be a corresponding call to `end_durable_function` after the function has
    /// performed its work (it can be ended in a different context, for example, after an async
    /// pollable operation has been completed)
    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, WorkerExecutorError>;

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
    ) -> Result<(), WorkerExecutorError>;

    /// Gets the current durable execution state
    fn durable_execution_state(&self) -> DurableExecutionState;

    /// Writes a record to the worker's oplog representing a durable function invocation
    async fn persist_durable_function_invocation(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    );

    /// Reads the next persisted durable function invocation from the oplog during replay
    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<PersistedDurableFunctionInvocation, WorkerExecutorError>;

    /// Checks if the current retry policy allows more retries, and if yes, then returns
    /// with `Err(failure)`. This error should be directly returned from host function
    /// implementations, triggering a retry.
    ///
    /// If retrying is not possible, the function returns Ok(()) and the host function
    /// can continue persisting the failed result permanently.
    async fn try_trigger_retry(&mut self, failure: Error) -> anyhow::Result<()>;
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
            durability::DurableFunctionType::WriteRemoteTransaction(oplog_index) => {
                DurableFunctionType::WriteRemoteTransaction(oplog_index.map(OplogIndex::from_u64))
            }
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
            DurableFunctionType::WriteRemoteTransaction(oplog_index) => {
                durability::DurableFunctionType::WriteRemoteTransaction(
                    oplog_index.map(|idx| idx.into()),
                )
            }
        }
    }
}

impl From<OplogEntryVersion> for durability::OplogEntryVersion {
    fn from(value: OplogEntryVersion) -> Self {
        let _ = value;
        durability::OplogEntryVersion::V2
    }
}

impl From<PersistedDurableFunctionInvocation> for durability::PersistedDurableFunctionInvocation {
    fn from(value: PersistedDurableFunctionInvocation) -> Self {
        durability::PersistedDurableFunctionInvocation {
            timestamp: value.timestamp.into(),
            function_name: value.function_name,
            response: value.response.into_value_and_type().into(),
            function_type: value.function_type.into(),
            entry_version: value.entry_version.into(),
        }
    }
}

impl<Ctx: WorkerCtx> durability::HostLazyInitializedPollable for DurableWorkerCtx<Ctx> {
    async fn new(&mut self) -> anyhow::Result<Resource<LazyInitializedPollableEntry>> {
        DurabilityHost::observe_function_call(self, "durability::lazy_initialized_pollable", "new");
        let lazy_pollable = self.table().push(LazyInitializedPollableEntry::Empty)?;
        Ok(lazy_pollable)
    }

    async fn set(
        &mut self,
        self_: Resource<LazyInitializedPollableEntry>,
        pollable: Resource<DynPollable>,
    ) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "durability::lazy_initialized_pollable", "set");
        let entry = self.table().get_mut(&self_)?;
        *entry = LazyInitializedPollableEntry::Subscribed { pollable };
        Ok(())
    }

    async fn subscribe(
        &mut self,
        self_: Resource<LazyInitializedPollableEntry>,
    ) -> anyhow::Result<Resource<DynPollable>> {
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
        request: durability::ValueAndType,
        response: durability::ValueAndType,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<()> {
        DurabilityHost::persist_durable_function_invocation(
            self,
            HostFunctionName::Custom(function_name),
            &HostRequest::Custom(request.into()),
            &HostResponse::Custom(response.into()),
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
}

#[async_trait]
impl<Ctx: WorkerCtx> DurabilityHost for DurableWorkerCtx<Ctx> {
    fn observe_function_call(&self, interface: &str, function: &str) {
        record_host_function_call(interface, function);
    }

    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        self.process_pending_replay_events().await?;
        let oplog_index = self.begin_function(function_type).await?;
        Ok(oplog_index)
    }

    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        self.end_function(function_type, begin_index).await?;
        if function_type == &DurableFunctionType::WriteRemote
            || matches!(function_type, DurableFunctionType::WriteRemoteBatched(_))
            || matches!(
                function_type,
                DurableFunctionType::WriteRemoteTransaction(_)
            )
            || forced_commit
        {
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                .await;
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
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    ) {
        self.public_state
            .worker()
            .oplog()
            .add_host_call(function_name, request, response, function_type)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to serialize and store durable function invocation: {err}")
            });
    }

    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<PersistedDurableFunctionInvocation, WorkerExecutorError> {
        if self.state.persistence_level == PersistenceLevel::PersistNothing {
            Err(WorkerExecutorError::runtime(
                "Trying to replay an durable invocation in a PersistNothing block",
            ))
        } else {
            let (_, oplog_entry) =
                crate::get_oplog_entry!(self.state.replay_state, OplogEntry::HostCall)?;
            match oplog_entry {
                OplogEntry::HostCall {
                    timestamp,
                    function_name,
                    durable_function_type,
                    response,
                    ..
                } => {
                    let response = self
                        .public_state
                        .worker()
                        .oplog()
                        .download_payload(response)
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "HostCall payload cannot be downloaded: {err}"
                            ))
                        })?;
                    Ok(PersistedDurableFunctionInvocation {
                        timestamp,
                        function_name: function_name.to_string(),
                        response,
                        function_type: durable_function_type,
                        entry_version: OplogEntryVersion::V2,
                    })
                }
                _ => Err(WorkerExecutorError::unexpected_oplog_entry(
                    "HostCall",
                    format!("{oplog_entry:?}"),
                )),
            }
        }
    }

    async fn try_trigger_retry(&mut self, failure: Error) -> anyhow::Result<()> {
        let latest_status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        let current_retry_point = self.state.current_retry_point;

        let default_retry_config = &self.state.config.retry;
        let retry_config = self
            .state
            .overridden_retry_policy
            .as_ref()
            .unwrap_or(default_retry_config)
            .clone();
        let trap_type = TrapType::from_error::<Ctx>(&failure, current_retry_point);
        let decision = Self::get_recovery_decision_on_trap(
            &retry_config,
            &latest_status.current_retry_count,
            &trap_type,
        );

        match decision {
            RetryDecision::Immediate
            | RetryDecision::Delayed(_)
            | RetryDecision::ReacquirePermits => Err(failure),
            RetryDecision::None | RetryDecision::TryStop(_) => Ok(()),
        }
    }
}

#[derive(Debug)]
pub enum OplogEntryVersion {
    V2,
}

pub struct Durability<Pair: HostPayloadPair> {
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    durable_execution_state: DurableExecutionState,
    _phantom: std::marker::PhantomData<Pair>,
}

impl<Pair: HostPayloadPair> Durability<Pair> {
    pub async fn new(
        ctx: &mut impl DurabilityHost,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError> {
        ctx.observe_function_call(Pair::INTERFACE, Pair::FUNCTION);

        let begin_index = ctx.begin_durable_function(&function_type).await?;
        let durable_execution_state = ctx.durable_execution_state();

        Ok(Self {
            function_type,
            begin_index,
            durable_execution_state,
            _phantom: std::marker::PhantomData,
        })
    }

    pub fn is_live(&self) -> bool {
        self.durable_execution_state.is_live
    }

    /// Checks if the current retry policy allows more retries, and if yes, then returns
    /// with `Err(failure)`. This error should be directly returned from host function
    /// implementations, triggering a retry.
    ///
    /// If retrying is not possible, the function returns Ok(()) and the host function
    /// can continue persisting the failed result permanently.
    pub async fn try_trigger_retry<Ok, Err: Display>(
        &self,
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
    ) -> anyhow::Result<()> {
        if let Err(err) = result {
            ctx.try_trigger_retry(Error::msg(err.to_string())).await
        } else {
            Ok(())
        }
    }

    pub async fn persist(
        &self,
        ctx: &mut impl DurabilityHost,
        request: Pair::Req,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        let response = self
            .persist_raw(ctx, request.into(), response.into())
            .await?;
        Ok(response.try_into().unwrap()) // Assuming converting to HostResponse and back always succeeds
    }

    pub async fn persist_raw(
        &self,
        ctx: &mut impl DurabilityHost,
        request: HostRequest,
        response: HostResponse,
    ) -> Result<HostResponse, WorkerExecutorError> {
        if self.durable_execution_state.snapshotting_mode.is_none() {
            ctx.persist_durable_function_invocation(
                Pair::HOST_FUNCTION_NAME,
                &request,
                &response,
                self.function_type.clone(),
            )
            .await;
            ctx.end_durable_function(&self.function_type, self.begin_index, false)
                .await?;
        }
        Ok(response)
    }

    pub async fn replay(
        &self,
        ctx: &mut impl DurabilityHost,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        let response = self.replay_raw(ctx).await?;
        response
            .try_into()
            .map_err(|err| WorkerExecutorError::unexpected_oplog_entry("HostResponse", err))
    }

    pub async fn replay_raw(
        &self,
        ctx: &mut impl DurabilityHost,
    ) -> Result<HostResponse, WorkerExecutorError> {
        let oplog_entry = ctx.read_persisted_durable_function_invocation().await?;

        let function_name = Pair::FQFN;
        Self::validate_oplog_entry(&oplog_entry, function_name)?;

        ctx.end_durable_function(&self.function_type, self.begin_index, false)
            .await?;

        Ok(oplog_entry.response)
    }

    fn validate_oplog_entry(
        oplog_entry: &PersistedDurableFunctionInvocation,
        expected_function_name: &str,
    ) -> Result<(), WorkerExecutorError> {
        if oplog_entry.function_name != expected_function_name {
            error!(
                "Unexpected imported function call entry in oplog: expected {}, got {}",
                expected_function_name, oplog_entry.function_name
            );
            Err(WorkerExecutorError::unexpected_oplog_entry(
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
    Subscribed { pollable: Resource<DynPollable> },
}

#[async_trait]
impl Pollable for LazyInitializedPollableEntry {
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

impl DynamicPollable for LazyInitializedPollableEntry {
    fn override_index(&self) -> Option<u32> {
        match self {
            LazyInitializedPollableEntry::Empty => None,
            LazyInitializedPollableEntry::Subscribed { pollable } => Some(pollable.rep()),
        }
    }
}
