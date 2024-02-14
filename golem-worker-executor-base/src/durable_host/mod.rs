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

// WASI Host implementation for Golem, delegating to the core WASI implementation (wasmtime_wasi)
// implementing the Golem specific instrumentation on top of it.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::string::FromUtf8Error;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::error::{is_interrupt, is_suspend, GolemError};
use crate::invocation::invoke_worker;
use crate::model::{CurrentResourceLimits, ExecutionStatus, InterruptKind, WorkerConfig};
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::BlobStoreService;
use crate::services::golem_config::GolemConfig;
use crate::services::invocation_key::{InvocationKeyService, LookupResult};
use crate::services::key_value::KeyValueService;
use crate::services::promise::PromiseService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::HasAll;
use crate::wasi_host::managed_stdio::ManagedStandardIo;
use crate::workerctx::{
    ExternalOperations, InvocationHooks, InvocationManagement, IoCapturing, PublicWorkerIo,
    StatusManagement, WorkerCtx,
};
use anyhow::anyhow;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use cap_std::ambient_authority;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, PromiseId, Timestamp, VersionedWorkerId, WorkerId,
    WorkerMetadata, WorkerStatus,
};
use golem_common::model::{OplogEntry, WrappedFunctionType};
use golem_wasm_rpc::Value;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tracing::{debug, info};
use wasmtime::component::{Instance, Resource};
use wasmtime::AsContextMut;
use wasmtime_wasi::preview2::{I32Exit, ResourceTable, Stderr, Subscribe, WasiCtx, WasiView};
use wasmtime_wasi_http::types::{
    default_send_request, HostFutureIncomingResponse, OutgoingRequest,
};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::durable_host::io::{ManagedStdErr, ManagedStdIn, ManagedStdOut};
use crate::metrics::wasm::{record_number_of_replayed_functions, record_resume_worker};
use crate::services::oplog::OplogService;
use crate::services::recovery::{RecoveryDecision, RecoveryManagement};
use crate::services::scheduler::SchedulerService;
use crate::services::HasOplogService;
use crate::wasi_host;

pub mod blobstore;
mod cli;
mod clocks;
mod filesystem;
mod golem;
mod http;
pub mod io;
pub mod keyvalue;
mod logging;
mod random;
pub mod serialized;
mod sockets;
mod wasm_rpc;

/// Partial implementation of the WorkerCtx interfaces for adding durable execution to workers.
pub struct DurableWorkerCtx<Ctx: WorkerCtx> {
    table: ResourceTable,
    wasi: WasiCtx,
    wasi_http: WasiHttpCtx,
    pub worker_id: VersionedWorkerId,
    pub public_state: PublicDurableWorkerState,
    private_state: PrivateDurableWorkerState<Ctx>,
    #[allow(unused)]
    temp_dir: Arc<TempDir>,
    execution_status: Arc<RwLock<ExecutionStatus>>,
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.private_state.is_live()
    }

    pub fn is_replay(&self) -> bool {
        self.private_state.is_replay()
    }

    pub async fn set_oplog_entry(&mut self, oplog_entry: OplogEntry) {
        self.private_state.set_oplog_entry(oplog_entry).await
    }

    pub async fn commit_oplog(&mut self) {
        self.private_state.commit_oplog().await
    }

    async fn get_oplog_entry_imported_function_invoked<'de, R>(&mut self) -> Result<R, GolemError>
    where
        R: Decode + DeserializeOwned,
    {
        self.private_state
            .get_oplog_entry_imported_function_invoked()
            .await
    }

    pub async fn get_oplog_entry_exported_function_invoked(
        &mut self,
    ) -> Result<
        Option<(
            String,
            Vec<Value>,
            Option<InvocationKey>,
            Option<CallingConvention>,
        )>,
        GolemError,
    > {
        self.private_state
            .get_oplog_entry_exported_function_invoked()
            .await
    }

    pub async fn get_oplog_entry_exported_function_completed(
        &mut self,
    ) -> Result<Option<Vec<Value>>, GolemError> {
        self.private_state
            .get_oplog_entry_exported_function_completed()
            .await
    }

    pub async fn consume_hint_entries(&mut self) {
        self.private_state.consume_hint_entries().await
    }

    #[allow(unused)]
    pub async fn dump_remaining_oplog(&self) {
        let current = self.private_state.oplog_idx as usize;
        let entries = self
            .private_state
            .oplog_service
            .read(
                &self.private_state.worker_id,
                0,
                self.private_state.oplog_size,
            )
            .await;
        let mut dump = String::new();
        dump.push_str(&format!(
            "\nOplog dump for {}\n",
            self.private_state.worker_id
        ));
        for (idx, entry) in entries.iter().enumerate() {
            let mark = if idx == current { "*" } else { " " };
            dump.push_str(&format!("{} {}: {:?}\n", mark, idx, entry));
        }
        dump.push_str(&format!(
            "End of oplog dump for {}\n",
            self.private_state.worker_id
        ));
        debug!("{}", dump);
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub async fn create(
        worker_id: VersionedWorkerId,
        account_id: AccountId,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<Ctx>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError> {
        let temp_dir = Arc::new(tempfile::Builder::new().prefix("golem").tempdir().map_err(
            |e| GolemError::runtime(format!("Failed to create temporary directory: {e}")),
        )?);
        debug!(
            "Created temporary file system root for {:?} at {:?}",
            worker_id,
            temp_dir.path()
        );
        let root_dir = cap_std::fs::Dir::open_ambient_dir(temp_dir.path(), ambient_authority())
            .map_err(|e| GolemError::runtime(format!("Failed to open temporary directory: {e}")))?;

        let oplog_size = oplog_service.get_size(&worker_id.worker_id).await;

        let stdio =
            ManagedStandardIo::new(worker_id.worker_id.clone(), invocation_key_service.clone());
        let stdin = ManagedStdIn::from_standard_io(stdio.clone()).await;
        let stdout = ManagedStdOut::from_standard_io(stdio.clone());
        let stderr = ManagedStdErr::from_stderr(Stderr);

        wasi_host::create_context(
            &worker_config.args,
            &worker_config.env,
            root_dir,
            temp_dir.path().to_path_buf(),
            stdin,
            stdout,
            stderr,
            |duration| anyhow!(SuspendForSleep(duration)),
            config.suspend.suspend_after,
            |wasi, table| {
                let wasi_http = WasiHttpCtx;
                DurableWorkerCtx {
                    table,
                    wasi,
                    wasi_http,
                    worker_id: worker_id.clone(),
                    public_state: PublicDurableWorkerState {
                        promise_service: promise_service.clone(),
                        event_service: event_service.clone(),
                        managed_stdio: stdio,
                    },
                    private_state: PrivateDurableWorkerState {
                        buffer: VecDeque::new(),
                        oplog_idx: 0,
                        oplog_size,
                        oplog_service,
                        promise_service: promise_service.clone(),
                        scheduler_service,
                        worker_service: worker_service.clone(),
                        invocation_key_service,
                        key_value_service,
                        blob_store_service,
                        config: config.clone(),
                        worker_id: worker_id.worker_id.clone(),
                        account_id: account_id.clone(),
                        current_invocation_key: None,
                        active_workers: active_workers.clone(),
                        recovery_management,
                    },
                    temp_dir,
                    execution_status,
                }
            },
        )
        .map_err(|e| GolemError::runtime(format!("Could not create WASI context: {e}")))
    }

    pub fn get_public_state(&self) -> &PublicDurableWorkerState {
        &self.public_state
    }

    pub fn worker_id(&self) -> &VersionedWorkerId {
        &self.worker_id
    }

    pub fn is_exit(error: &anyhow::Error) -> Option<i32> {
        error
            .root_cause()
            .downcast_ref::<I32Exit>()
            .map(|exit| exit.0)
    }

    pub fn as_wasi_view(&mut self) -> DurableWorkerCtxWasiView<Ctx> {
        DurableWorkerCtxWasiView(self)
    }

    pub fn as_wasi_http_view(&mut self) -> DurableWorkerCtxWasiHttpView<Ctx> {
        DurableWorkerCtxWasiHttpView(self)
    }

    pub async fn create_promise(&self, oplog_idx: i32) -> PromiseId {
        self.public_state
            .promise_service
            .create(&self.worker_id.worker_id, oplog_idx)
            .await
    }

    pub async fn poll_promise(&self, id: PromiseId) -> Result<Option<Vec<u8>>, GolemError> {
        self.public_state.promise_service.poll(id).await
    }

    pub async fn complete_promise(&self, id: PromiseId, data: Vec<u8>) -> Result<bool, GolemError> {
        self.public_state.promise_service.complete(id, data).await
    }

    pub fn check_interrupt(&self) -> Option<InterruptKind> {
        let execution_status = self.execution_status.read().unwrap().clone();
        match execution_status {
            ExecutionStatus::Interrupting { interrupt_kind, .. } => Some(interrupt_kind),
            ExecutionStatus::Interrupted { interrupt_kind } => Some(interrupt_kind),
            _ => None,
        }
    }

    pub fn set_suspended(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running => {
                *execution_status = ExecutionStatus::Suspended;
            }
            ExecutionStatus::Suspended => {}
            ExecutionStatus::Interrupting {
                interrupt_kind,
                await_interruption,
            } => {
                *execution_status = ExecutionStatus::Interrupted { interrupt_kind };
                await_interruption.send(()).ok();
            }
            ExecutionStatus::Interrupted { .. } => {}
        }
    }

    pub fn set_running(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running => {}
            ExecutionStatus::Suspended => {
                *execution_status = ExecutionStatus::Running;
            }
            ExecutionStatus::Interrupting { .. } => {}
            ExecutionStatus::Interrupted { .. } => {}
        }
    }

    pub async fn get_worker_status(&self) -> WorkerStatus {
        match self
            .private_state
            .worker_service
            .get(&self.worker_id.worker_id)
            .await
        {
            Some(metadata) => {
                if metadata.last_known_status.oplog_idx == self.private_state.oplog_idx {
                    metadata.last_known_status.status
                } else {
                    WorkerStatus::Running
                }
            }
            None => WorkerStatus::Idle,
        }
    }

    pub async fn store_worker_status(&self, status: WorkerStatus) {
        let oplog_idx = self.private_state.oplog_idx;
        self.private_state
            .worker_service
            .update_status(&self.worker_id.worker_id, status, oplog_idx)
            .await
    }

    pub fn get_stdio(&self) -> ManagedStandardIo {
        self.public_state.managed_stdio.clone()
    }

    pub async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.get_stdio()
            .get_current_invocation_key()
            .await
            .or(self.private_state.get_current_invocation_key())
    }

    pub fn get_current_invocation_result(&self) -> LookupResult {
        match &self.private_state.current_invocation_key {
            Some(key) => self
                .private_state
                .invocation_key_service
                .lookup_key(&self.private_state.worker_id, key),
            None => LookupResult::Invalid,
        }
    }
}

pub(crate) trait Durability<Ctx: WorkerCtx, SerializedSuccess, SerializedErr> {
    /// Wrap a WASI call with durability handling
    ///
    /// The function checks if the execution is live, and if so performs the function and then
    /// saves its results into the oplog. If the execution is not live, it reads the previously
    /// saved results from the oplog and returns them.
    ///
    /// Type parameters:
    /// - `AsyncFn`: the async WASI function to perform, expected to return with a Result of `Success` or `Err`
    /// - `Success`: The type of the success value returned by the WASI function
    /// - `Err`: The type of the error value returned by the WASI function. There need to be a conversion from `GolemError`
    ///    to `Err` to be able to return internal failures.
    /// - `SerializedSuccess`: The type of the success value serialized into the oplog. It has to be encodeable/decodeable
    ///   and convertable from/to `Success`
    /// - `SerializedErr`: The type of the error value serialized into the oplog. It has to be encodeable/decodeable and
    ///    convertable from/to `Err`
    ///
    /// Parameters:
    /// - `wrapped_function_type`: The type of the wrapped function, it is a combination of being local or remote, and
    ///   being read or write
    /// - `function_name`: The name of the function, used for logging
    /// - `function`: The async WASI function to perform
    async fn wrap<Success, Err, AsyncFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        function: AsyncFn,
    ) -> Result<Success, Err>
    where
        Success: Clone,
        AsyncFn: for<'b> FnOnce(
            &'b mut DurableWorkerCtx<Ctx>,
        )
            -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>,
        SerializedSuccess:
            Encode + Decode + DeserializeOwned + From<Success> + Into<Success> + Debug,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug;

    /// A version of `wrap` allowing conversion between the success value and the serialized value within the mutable worker context.
    ///
    /// This can be used to fetch/register resources.
    async fn custom_wrap<Success, Err, AsyncFn, GetFn, PutFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        function: AsyncFn,
        get_serializable: GetFn,
        put_serializable: PutFn,
    ) -> Result<Success, Err>
    where
        AsyncFn: for<'b> FnOnce(
            &'b mut DurableWorkerCtx<Ctx>,
        )
            -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>,
        GetFn: FnOnce(&mut DurableWorkerCtx<Ctx>, &Success) -> Result<SerializedSuccess, Err>,
        PutFn: for<'b> FnOnce(
            &'b mut DurableWorkerCtx<Ctx>,
            SerializedSuccess,
        )
            -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>,
        SerializedSuccess: Encode + Decode + DeserializeOwned + Debug,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug;
}

impl<Ctx: WorkerCtx, SerializedSuccess, SerializedErr>
    Durability<Ctx, SerializedSuccess, SerializedErr> for DurableWorkerCtx<Ctx>
{
    async fn wrap<Success, Err, AsyncFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        function: AsyncFn,
    ) -> Result<Success, Err>
    where
        Success: Clone,
        AsyncFn: for<'b> FnOnce(
            &'b mut DurableWorkerCtx<Ctx>,
        )
            -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>,
        SerializedSuccess:
            Encode + Decode + DeserializeOwned + From<Success> + Into<Success> + Debug,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug,
    {
        self.consume_hint_entries().await;
        if self.is_live() {
            let result = function(self).await;
            let serializable_result: Result<SerializedSuccess, SerializedErr> = result
                .as_ref()
                .map(|result| result.clone().into())
                .map_err(|err| err.into());
            let oplog_entry = OplogEntry::imported_function_invoked(
                Timestamp::now_utc(),
                function_name.to_string(),
                &serializable_result,
                wrapped_function_type.clone(),
            )
            .unwrap_or_else(|err| {
                panic!(
                    "failed to serialize function response: {:?}: {err}",
                    serializable_result
                )
            });
            self.set_oplog_entry(oplog_entry).await;
            if matches!(wrapped_function_type, WrappedFunctionType::WriteRemote) {
                self.commit_oplog().await;
            }
            result
        } else {
            let response = self
                .get_oplog_entry_imported_function_invoked::<Result<SerializedSuccess, SerializedErr>>()
                .await.map_err(|err| Into::<SerializedErr>::into(err).into())?;
            response
                .map(|serialized_success| serialized_success.into())
                .map_err(|serialized_err| serialized_err.into())
        }
    }

    async fn custom_wrap<Success, Err, AsyncFn, GetFn, PutFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        function: AsyncFn,
        get_serializable: GetFn,
        put_serializable: PutFn,
    ) -> Result<Success, Err>
    where
        AsyncFn: for<'b> FnOnce(
            &'b mut DurableWorkerCtx<Ctx>,
        )
            -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>,
        GetFn: FnOnce(&mut DurableWorkerCtx<Ctx>, &Success) -> Result<SerializedSuccess, Err>,
        PutFn: for<'b> FnOnce(
            &'b mut DurableWorkerCtx<Ctx>,
            SerializedSuccess,
        )
            -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>,
        SerializedSuccess: Encode + Decode + DeserializeOwned + Debug,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug,
    {
        self.consume_hint_entries().await;
        if self.is_live() {
            let result = function(self).await;
            let serializable_result: Result<SerializedSuccess, SerializedErr> = result
                .as_ref()
                .map_err(|err| err.into())
                .and_then(|result| get_serializable(self, result).map_err(|err| (&err).into()));

            let oplog_entry = OplogEntry::imported_function_invoked(
                Timestamp::now_utc(),
                function_name.to_string(),
                &serializable_result,
                wrapped_function_type.clone(),
            )
            .unwrap_or_else(|err| {
                panic!(
                    "failed to serialize function response: {:?}: {err}",
                    serializable_result
                )
            });
            self.set_oplog_entry(oplog_entry).await;
            if matches!(wrapped_function_type, WrappedFunctionType::WriteRemote) {
                self.commit_oplog().await;
            }
            result
        } else {
            let response = self
                .get_oplog_entry_imported_function_invoked::<Result<SerializedSuccess, SerializedErr>>()
                .await.map_err(|err| Into::<SerializedErr>::into(err).into())?;
            match response {
                Ok(serialized_success) => {
                    let success = put_serializable(self, serialized_success).await?;
                    Ok(success)
                }
                Err(serialized_err) => Err(serialized_err.into()),
            }
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationManagement for DurableWorkerCtx<Ctx> {
    async fn set_current_invocation_key(&mut self, invocation_key: Option<InvocationKey>) {
        self.private_state
            .set_current_invocation_key(invocation_key)
    }

    async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.get_current_invocation_key().await
    }

    async fn interrupt_invocation_key(&mut self, key: &InvocationKey) {
        self.private_state.interrupt_invocation_key(key).await
    }

    async fn resume_invocation_key(&mut self, key: &InvocationKey) {
        self.private_state.resume_invocation_key(key).await
    }

    async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Value>, GolemError>,
    ) {
        self.private_state.confirm_invocation_key(key, vals).await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> IoCapturing for DurableWorkerCtx<Ctx> {
    async fn start_capturing_stdout(&mut self, provided_stdin: String) {
        self.public_state
            .managed_stdio
            .start_single_stdio_call(provided_stdin)
            .await
    }

    async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error> {
        self.public_state
            .managed_stdio
            .finish_single_stdio_call()
            .await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> StatusManagement for DurableWorkerCtx<Ctx> {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        self.check_interrupt()
    }

    fn set_suspended(&self) {
        self.set_suspended()
    }

    fn set_running(&self) {
        self.set_running()
    }

    async fn get_worker_status(&self) -> WorkerStatus {
        self.get_worker_status().await
    }

    async fn store_worker_status(&self, status: WorkerStatus) {
        self.store_worker_status(status).await
    }

    async fn deactivate(&self) {
        debug!("deactivating worker {}", self.worker_id);
        self.private_state
            .active_workers
            .remove(&self.worker_id.worker_id);
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationHooks for DurableWorkerCtx<Ctx> {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        calling_convention: Option<&CallingConvention>,
    ) -> anyhow::Result<()> {
        let proto_function_input: Vec<golem_wasm_rpc::protobuf::Val> = function_input
            .iter()
            .map(|value| value.clone().into())
            .collect();
        let oplog_entry = OplogEntry::exported_function_invoked(
            Timestamp::now_utc(),
            full_function_name.to_string(),
            &proto_function_input,
            self.get_current_invocation_key().await,
            calling_convention.cloned(),
        )
        .unwrap_or_else(|err| {
            panic!(
                "could not encode function input for {full_function_name} on {}: {err}",
                self.worker_id()
            )
        });

        self.set_oplog_entry(oplog_entry).await;
        self.commit_oplog().await;
        Ok(())
    }

    async fn on_invocation_failure(&mut self, error: &anyhow::Error) -> Result<(), anyhow::Error> {
        self.consume_hint_entries().await;
        let is_live_after = self.is_live();

        let is_interrupt = is_interrupt(error);
        let is_suspend = is_suspend(error);

        if is_live_after && !is_interrupt && !is_suspend {
            self.set_oplog_entry(OplogEntry::Error {
                timestamp: Timestamp::now_utc(),
            })
            .await;

            self.commit_oplog().await;
        }

        Ok(())
    }

    async fn on_invocation_failure_deactivated(
        &mut self,
        error: &anyhow::Error,
    ) -> Result<WorkerStatus, anyhow::Error> {
        let previous_tries = self.private_state.trailing_error_count().await;
        let decision = self
            .private_state
            .recovery_management
            .schedule_recovery_for_error(&self.worker_id, previous_tries, error)
            .await;

        let oplog_idx = self.private_state.get_oplog_size().await;
        debug!(
            "Recovery decision for {}#{} because of error {} after {} tries: {:?}",
            self.worker_id, oplog_idx, error, previous_tries, decision
        );

        let is_interrupt = is_interrupt(error);
        let is_suspend = is_suspend(error);
        match decision {
            RecoveryDecision::None => {
                if is_interrupt {
                    Ok(WorkerStatus::Interrupted)
                } else if is_suspend {
                    Ok(WorkerStatus::Suspended)
                } else {
                    Ok(WorkerStatus::Failed)
                }
            }
            RecoveryDecision::Immediate => Ok(WorkerStatus::Retrying),
            RecoveryDecision::Delayed(_) => Ok(WorkerStatus::Retrying),
        }
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: Vec<Value>,
    ) -> Result<Option<Vec<Value>>, anyhow::Error> {
        self.consume_hint_entries().await;
        let is_live_after = self.is_live();

        if is_live_after {
            let proto_output: Vec<golem_wasm_rpc::protobuf::Val> =
                output.iter().map(|value| value.clone().into()).collect();
            let oplog_entry = OplogEntry::exported_function_completed(
                Timestamp::now_utc(),
                &proto_output,
                consumed_fuel,
            )
            .unwrap_or_else(|err| {
                panic!("could not encode function result for {full_function_name}: {err}")
            });

            self.set_oplog_entry(oplog_entry).await;
            self.set_oplog_entry(OplogEntry::Suspend {
                timestamp: Timestamp::now_utc(),
            })
            .await;
            self.commit_oplog().await;
        } else {
            let response = self.get_oplog_entry_exported_function_completed().await?;

            if let Some(function_output) = response {
                let is_diverged = function_output != output;
                if is_diverged {
                    return Err(anyhow!("Function {:?} with inputs {:?} has diverged! Output was {:?} when function was replayed but was {:?} when function was originally invoked", full_function_name, function_input, output, function_output));
                }
            }
        }

        debug!("Function finished with {:?}", output);

        // Return indicating that it is done
        Ok(Some(output))
    }
}

pub trait DurableWorkerCtxView<Ctx: WorkerCtx> {
    fn durable_ctx(&self) -> &DurableWorkerCtx<Ctx>;
    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Ctx>;
}

#[async_trait]
impl<Ctx: WorkerCtx + DurableWorkerCtxView<Ctx>> ExternalOperations<Ctx> for DurableWorkerCtx<Ctx> {
    type ExtraDeps = Ctx::ExtraDeps;

    async fn set_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        status: WorkerStatus,
    ) {
        let last_oplog_idx = last_oplog_idx(this, worker_id).await;
        this.worker_service()
            .update_status(worker_id, status, last_oplog_idx)
            .await;
    }

    async fn get_worker_retry_count<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> u32 {
        trailing_error_count(this, worker_id).await
    }

    async fn get_assumed_worker_status<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> WorkerStatus {
        let last_oplog_idx = last_oplog_idx(this, worker_id).await;
        let worker_status = match metadata {
            Some(metadata) => {
                if metadata.last_known_status.oplog_idx == last_oplog_idx {
                    debug!("get_assumed_instance_status for {worker_id}: stored last oplog idx matches current one, using stored status");
                    metadata.last_known_status.status.clone()
                } else {
                    debug!("get_assumed_instance_status for {worker_id}: stored last oplog idx ({}) does not match current one ({last_oplog_idx}), using stored status", metadata.last_known_status.oplog_idx);
                    WorkerStatus::Running
                }
            }
            None => WorkerStatus::Idle,
        };
        debug!("get_assumed_instance_status for {worker_id}: assuming status {worker_status:?}");
        worker_status
    }

    async fn prepare_instance(
        worker_id: &VersionedWorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<(), GolemError> {
        debug!("Starting prepare_instance for {}", worker_id);
        let start = Instant::now();
        let mut count = 0;
        let result = loop {
            let cont = store.as_context().data().durable_ctx().is_replay();

            if cont {
                let oplog_entry = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .get_oplog_entry_exported_function_invoked()
                    .await?;
                match oplog_entry {
                    None => break Ok(()),
                    Some((function_name, function_input, invocation_key, calling_convention)) => {
                        debug!("prepare_instance invoking function {function_name} on {worker_id}");
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_invocation_key(invocation_key)
                            .await;

                        let finished = invoke_worker(
                            function_name.to_string(),
                            function_input,
                            store,
                            instance,
                            &calling_convention.unwrap_or(CallingConvention::Component),
                            false, // we know it was not live before, because cont=true
                        )
                        .await;

                        if !finished {
                            break Err(GolemError::failed_to_resume_instance(
                                worker_id.worker_id.clone(),
                            ));
                        } else {
                            let result = store
                                .as_context()
                                .data()
                                .durable_ctx()
                                .get_current_invocation_result();
                            if matches!(result, LookupResult::Complete(Err(_))) {
                                // TODO: include the inner error in the failure?
                                break Err(GolemError::failed_to_resume_instance(
                                    worker_id.worker_id.clone(),
                                ));
                            }
                        }

                        count += 1;
                    }
                }
            } else {
                break Ok(());
            }
        };
        record_resume_worker(start.elapsed());
        record_number_of_replayed_functions(count);
        debug!("Finished prepare_instance for {}", worker_id);
        result
    }

    async fn record_last_known_limits<T: HasAll<Ctx> + Send + Sync>(
        _this: &T,
        _account_id: &AccountId,
        _last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        Ok(())
    }

    async fn on_worker_deleted<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        this.oplog_service().delete(worker_id).await;
        Ok(())
    }

    async fn on_shard_assignment_changed<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
    ) -> Result<(), anyhow::Error> {
        info!("Recovering instances");

        let instances = this.worker_service().get_running_workers_in_shards().await;

        debug!("Recovering running instances: {:?}", instances);

        for instance in instances {
            let worker_id = instance.worker_id;
            let previous_tries = Self::get_worker_retry_count(this, &worker_id.worker_id).await;
            let decision = this
                .recovery_management()
                .schedule_recovery_on_startup(&worker_id, previous_tries)
                .await;
            debug!(
                "Recovery decision for {} after {} tries: {:?}",
                worker_id, previous_tries, decision
            );
        }

        info!("Finished recovering instances");
        Ok(())
    }
}

async fn last_oplog_idx<T: HasOplogService>(this: &T, worker_id: &WorkerId) -> i32 {
    this.oplog_service().get_size(worker_id).await
}

async fn trailing_error_count<T: HasOplogService>(this: &T, worker_id: &WorkerId) -> u32 {
    let mut idx = this.oplog_service().get_size(worker_id).await;
    let mut count = 0;
    if idx == 0 {
        0
    } else {
        loop {
            let oplog_entry = this.oplog_service().read(worker_id, idx - 1, 1).await;
            match oplog_entry.first()
                .unwrap_or_else(|| panic!("Internal error: op log for {} has size greater than zero but no entry at last index", worker_id)) {
                OplogEntry::Error { .. } => {
                    count += 1;
                    if idx > 0 {
                        idx -= 1;
                        continue;
                    } else {
                        break count;
                    }
                }
                _ => break count,
            }
        }
    }
}

pub struct PrivateDurableWorkerState<Ctx: WorkerCtx> {
    buffer: VecDeque<OplogEntry>,
    oplog_idx: i32,
    oplog_size: i32,
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
    invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    key_value_service: Arc<dyn KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
    config: Arc<GolemConfig>,
    pub worker_id: WorkerId,
    pub account_id: AccountId,
    current_invocation_key: Option<InvocationKey>,
    pub active_workers: Arc<ActiveWorkers<Ctx>>,
    pub recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
}

impl<Ctx: WorkerCtx> PrivateDurableWorkerState<Ctx> {
    pub async fn commit_oplog(&mut self) {
        let worker_id = &self.worker_id;
        let mut arrays: Vec<OplogEntry> = Vec::new();
        self.buffer.iter().for_each(|oplog_entry| {
            arrays.push(oplog_entry.clone());
        });
        self.buffer.clear();
        self.oplog_service.append(worker_id, &arrays).await
    }

    pub async fn get_oplog_size(&mut self) -> i32 {
        self.oplog_service.get_size(&self.worker_id).await
    }

    pub async fn read_oplog(&self, idx: i32, n: i32) -> Vec<OplogEntry> {
        self.oplog_service.read(&self.worker_id, idx, n).await
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.oplog_idx >= self.oplog_size
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    async fn set_oplog_entry(&mut self, oplog_entry: OplogEntry) {
        assert!(self.is_live());
        self.buffer.push_back(oplog_entry);
        if self.buffer.len() > self.config.oplog.max_operations_before_commit as usize {
            self.commit_oplog().await;
        }
        self.oplog_idx += 1;
        self.oplog_size += 1;
    }

    pub async fn get_oplog_entry(&mut self) -> OplogEntry {
        assert!(self.is_replay());
        let oplog_entries = self.read_oplog(self.oplog_idx, 1).await;
        let oplog_entry = oplog_entries[0].clone();
        self.oplog_idx += 1;
        oplog_entry
    }

    async fn get_oplog_entry_imported_function_invoked<'de, R>(&mut self) -> Result<R, GolemError>
    where
        R: Decode + DeserializeOwned,
    {
        loop {
            let oplog_entry = self.get_oplog_entry().await;
            match oplog_entry {
                OplogEntry::ImportedFunctionInvoked { .. } => {
                    break Ok(oplog_entry
                        .response()
                        .unwrap_or_else(|err| {
                            panic!(
                                "failed to deserialize function response: {:?}: {err}",
                                oplog_entry
                            )
                        })
                        .unwrap());
                }
                OplogEntry::Suspend { .. } => (),
                OplogEntry::Error { .. } => (),
                OplogEntry::Debug { message, .. } => debug!("Debug: {}", message),
                _ => {
                    break Err(GolemError::unexpected_oplog_entry(
                        "ImportedFunctionInvoked",
                        format!("{:?}", oplog_entry),
                    ));
                }
            }
        }
    }

    async fn get_oplog_entry_exported_function_invoked(
        &mut self,
    ) -> Result<
        Option<(
            String,
            Vec<Value>,
            Option<InvocationKey>,
            Option<CallingConvention>,
        )>,
        GolemError,
    > {
        loop {
            if self.is_replay() {
                let oplog_entry = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionInvoked {
                        function_name,
                        invocation_key,
                        calling_convention,
                        ..
                    } => {
                        let request = oplog_entry
                            .payload_as_val_array()
                            .expect("failed to deserialize function request payload")
                            .unwrap();
                        let request = request
                            .into_iter()
                            .map(|val| {
                                val.try_into()
                                    .expect("failed to decode serialized protobuf value")
                            })
                            .collect::<Vec<Value>>();
                        break Ok(Some((
                            function_name.to_string(),
                            request,
                            invocation_key.clone(),
                            calling_convention.clone(),
                        )));
                    }
                    OplogEntry::Suspend { .. } => (),
                    OplogEntry::Error { .. } => (),
                    OplogEntry::Debug { message, .. } => debug!("Debug: {}", message),
                    _ => {
                        break Err(GolemError::unexpected_oplog_entry(
                            "ExportedFunctionInvoked",
                            format!("{:?}", oplog_entry),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    async fn get_oplog_entry_exported_function_completed(
        &mut self,
    ) -> Result<Option<Vec<Value>>, GolemError> {
        loop {
            if self.is_replay() {
                let oplog_entry = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionCompleted { .. } => {
                        let response = oplog_entry
                            .payload_as_val_array()
                            .expect("failed to deserialize function response payload")
                            .unwrap();
                        let response = response
                            .into_iter()
                            .map(|val| {
                                val.try_into()
                                    .expect("failed to decode serialized protobuf value")
                            })
                            .collect();
                        break Ok(Some(response));
                    }
                    OplogEntry::Suspend { .. } => (),
                    OplogEntry::Debug { message, .. } => debug!("Debug: {}", message),
                    _ => {
                        break Err(GolemError::unexpected_oplog_entry(
                            "ExportedFunctionCompleted",
                            format!("{:?}", oplog_entry),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    /// Consumes Suspend and Error entries which are hints for the server to decide whether to
    /// keep workers in memory or allow them to rerun etc but contain no actionable information
    /// during replay
    async fn consume_hint_entries(&mut self) {
        loop {
            if self.is_replay() {
                let oplog_entry = self.get_oplog_entry().await;
                match oplog_entry {
                    OplogEntry::Suspend { .. } => (),
                    OplogEntry::Error { .. } => (),
                    _ => {
                        self.oplog_idx -= 1;
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    pub async fn sleep_until(&self, when: chrono::DateTime<chrono::Utc>) -> Result<(), GolemError> {
        let promise_id = self
            .promise_service
            .create(&self.worker_id, self.oplog_idx)
            .await;

        let schedule_id = self.scheduler_service.schedule(when, promise_id).await;
        debug!(
            "Schedule added to awake suspended worker at {} with id {}",
            when.to_rfc3339(),
            schedule_id
        );

        Ok(())
    }

    pub async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Value>, GolemError>,
    ) {
        self.invocation_key_service
            .confirm_key(&self.worker_id, key, vals)
    }

    pub async fn interrupt_invocation_key(&mut self, key: &InvocationKey) {
        self.invocation_key_service
            .interrupt_key(&self.worker_id, key)
    }

    pub async fn resume_invocation_key(&mut self, key: &InvocationKey) {
        self.invocation_key_service.resume_key(&self.worker_id, key)
    }

    pub fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.current_invocation_key.clone()
    }

    pub fn set_current_invocation_key(&mut self, invocation_key: Option<InvocationKey>) {
        self.current_invocation_key = invocation_key;
    }

    /// Counts the number of Error entries that are at the end of the oplog. This equals to the number of retries that have been attempted.
    pub async fn trailing_error_count(&self) -> u32 {
        trailing_error_count(self, &self.worker_id).await
    }
}

impl<Ctx: WorkerCtx> HasOplogService for PrivateDurableWorkerState<Ctx> {
    fn oplog_service(&self) -> Arc<dyn OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

#[derive(Clone)]
pub struct PublicDurableWorkerState {
    pub promise_service: Arc<dyn PromiseService + Send + Sync>,
    pub event_service: Arc<dyn WorkerEventService + Send + Sync>,
    pub managed_stdio: ManagedStandardIo,
}

#[async_trait]
impl PublicWorkerIo for PublicDurableWorkerState {
    fn event_service(&self) -> Arc<dyn WorkerEventService + Send + Sync> {
        self.event_service.clone()
    }

    async fn enqueue(&self, message: Bytes, invocation_key: InvocationKey) {
        self.managed_stdio.enqueue(message, invocation_key).await
    }
}

pub struct DurableWorkerCtxWasiView<'a, Ctx: WorkerCtx>(&'a mut DurableWorkerCtx<Ctx>);

pub struct DurableWorkerCtxWasiHttpView<'a, Ctx: WorkerCtx>(&'a mut DurableWorkerCtx<Ctx>);

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
pub struct SuspendForSleep(Duration);

impl Display for SuspendForSleep {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Suspended for sleep {} ms", self.0.as_millis())
    }
}

impl Error for SuspendForSleep {}

// This wrapper forces the compiler to choose the wasmtime_wasi implementations for T: WasiView
impl<'a, Ctx: WorkerCtx> WasiView for DurableWorkerCtxWasiView<'a, Ctx> {
    fn table(&self) -> &ResourceTable {
        &self.0.table
    }

    fn table_mut(&mut self) -> &mut ResourceTable {
        &mut self.0.table
    }

    fn ctx(&self) -> &WasiCtx {
        &self.0.wasi
    }

    fn ctx_mut(&mut self) -> &mut WasiCtx {
        &mut self.0.wasi
    }
}

impl<'a, Ctx: WorkerCtx> WasiHttpView for DurableWorkerCtxWasiHttpView<'a, Ctx> {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.0.wasi_http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.0.table
    }

    fn send_request(
        &mut self,
        request: OutgoingRequest,
    ) -> anyhow::Result<Resource<HostFutureIncomingResponse>>
    where
        Self: Sized,
    {
        if self.0.is_replay() {
            // If this is a replay, we must not actually send the request, but we have to store it in the
            // FutureIncomingResponse because it is possible that there wasn't any response recorded in the oplog.
            // If that is the case, the request has to be sent as soon as we get into live mode and trying to await
            // or poll the response future.
            let fut = self
                .table()
                .push(HostFutureIncomingResponse::deferred(request))?;
            Ok(fut)
        } else {
            default_send_request(self, request)
        }
    }
}

struct Ready {}

#[async_trait]
impl Subscribe for Ready {
    async fn ready(&mut self) {}
}
