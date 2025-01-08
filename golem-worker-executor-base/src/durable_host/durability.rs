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
use crate::model::PersistenceLevel;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use golem_common::model::oplog::{OplogEntry, OplogIndex, WrappedFunctionType};
use golem_common::serialization::try_deserialize;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use tracing::error;

pub enum OplogEntryVersion {
    V1,
    V2,
}

// TODO: is_live and replay can be merged
// TODO: is SErr always SerializableError? - sometimes SerializableStreamError
pub struct Durability2<Ctx, SOk, SErr> {
    package: &'static str,
    function: &'static str,
    function_type: WrappedFunctionType,
    begin_index: OplogIndex,
    is_live: bool,
    persistence_level: PersistenceLevel,
    _ctx: PhantomData<Ctx>,
    _sok: PhantomData<SOk>,
    _serr: PhantomData<SErr>,
}

impl<Ctx: WorkerCtx, SOk, SErr> Durability2<Ctx, SOk, SErr> {
    pub async fn new(
        ctx: &mut DurableWorkerCtx<Ctx>,
        package: &'static str,
        function: &'static str,
        function_type: WrappedFunctionType,
    ) -> Result<Self, GolemError> {
        record_host_function_call(package, function);

        let begin_index = ctx.state.begin_function(&function_type).await?;

        Ok(Self {
            package,
            function,
            function_type,
            begin_index,
            is_live: ctx.state.is_live(),
            persistence_level: ctx.state.persistence_level.clone(),
            _ctx: PhantomData,
            _sok: PhantomData,
            _serr: PhantomData,
        })
    }

    pub fn is_live(&self) -> bool {
        self.is_live || self.persistence_level == PersistenceLevel::PersistNothing
    }

    pub async fn persist<SIn, Ok, Err>(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
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
        ctx: &mut DurableWorkerCtx<Ctx>,
        input: SIn,
        serializable_result: Result<SOk, SErr>,
    ) -> Result<(), GolemError>
    where
        SIn: Debug + Encode + Send + Sync,
        SOk: Debug + Encode + Send + Sync,
        SErr: Debug + Encode + Send + Sync,
    {
        let function_name = self.function_name();
        ctx.write_to_oplog::<SIn, SOk, SErr>(
            &self.function_type,
            &function_name,
            self.begin_index,
            &input,
            &serializable_result,
        )
        .await
    }

    pub async fn replay_raw(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<(Bytes, OplogEntryVersion), GolemError> {
        let (_, oplog_entry) = crate::get_oplog_entry!(
            ctx.state.replay_state,
            OplogEntry::ImportedFunctionInvoked,
            OplogEntry::ImportedFunctionInvokedV1
        )?;

        let version = if matches!(oplog_entry, OplogEntry::ImportedFunctionInvokedV1 { .. }) {
            OplogEntryVersion::V1
        } else {
            OplogEntryVersion::V2
        };

        let function_name = self.function_name();
        DurableWorkerCtx::<Ctx>::validate_oplog_entry(&oplog_entry, &function_name)?;

        let bytes = ctx
            .state
            .oplog
            .get_raw_payload_of_entry(&oplog_entry)
            .await
            .map_err(|err| {
                GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
            })?
            .unwrap();

        ctx.state
            .end_function(&self.function_type, self.begin_index)
            .await?;

        Ok((bytes, version))
    }

    pub async fn replay_serializable(
        &self,
        ctx: &mut DurableWorkerCtx<Ctx>,
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

    pub async fn replay<Ok, Err>(&self, ctx: &mut DurableWorkerCtx<Ctx>) -> Result<Ok, Err>
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
        if self.package.is_empty() {
            // For backward compatibility - some of the recorded function names were not following the pattern
            self.function.to_string()
        } else {
            format!("{}::{}", self.package, self.function)
        }
    }
}

#[async_trait]
pub trait Durability<Ctx: WorkerCtx, SerializableInput, SerializableSuccess, SerializableErr> {
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
        input: SerializableInput,
        function: AsyncFn,
    ) -> Result<Success, Err>
    where
        Success: Clone + Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess:
            Encode + Decode + From<Success> + Into<Success> + Debug + Send + Sync + 'static,
        SerializableErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync
            + 'static;

    async fn wrap_conditionally<Success, Err, AsyncFn, ConditionFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        input: SerializableInput,
        function: AsyncFn,
        persist: ConditionFn,
    ) -> Result<Success, Err>
    where
        Success: Clone + Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        ConditionFn: FnOnce(&Result<Success, Err>) -> bool + Send,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess: Encode + Decode + From<Success> + Into<Success> + Debug + Send + Sync,
        SerializableErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync;
}

#[async_trait]
impl<Ctx: WorkerCtx, SerializableInput, SerializableSuccess, SerializableErr>
    Durability<Ctx, SerializableInput, SerializableSuccess, SerializableErr>
    for DurableWorkerCtx<Ctx>
{
    async fn wrap<Success, Err, AsyncFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        input: SerializableInput,
        function: AsyncFn,
    ) -> Result<Success, Err>
    where
        Success: Clone + Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess:
            Encode + Decode + From<Success> + Into<Success> + Debug + Send + Sync + 'static,
        SerializableErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync
            + 'static,
    {
        <DurableWorkerCtx<Ctx> as Durability<
            Ctx,
            SerializableInput,
            SerializableSuccess,
            SerializableErr,
        >>::wrap_conditionally::<Success, Err, AsyncFn, _>(
            self,
            wrapped_function_type,
            function_name,
            input,
            function,
            |_: &Result<Success, Err>| true,
        )
        .await
    }

    async fn wrap_conditionally<Success, Err, AsyncFn, ConditionFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        input: SerializableInput,
        function: AsyncFn,
        persist: ConditionFn,
    ) -> Result<Success, Err>
    where
        Success: Clone + Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        ConditionFn: FnOnce(&Result<Success, Err>) -> bool + Send,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess: Encode + Decode + From<Success> + Into<Success> + Debug + Send + Sync,
        SerializableErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync,
    {
        let begin_index = self
            .state
            .begin_function(&wrapped_function_type.clone())
            .await?;
        if self.state.is_live() || self.state.persistence_level == PersistenceLevel::PersistNothing
        {
            let result = function(self).await;
            if persist(&result) {
                let serializable_result: Result<SerializableSuccess, SerializableErr> = result
                    .as_ref()
                    .map(|result| result.clone().into())
                    .map_err(|err| err.into());

                self.write_to_oplog(
                    &wrapped_function_type,
                    function_name,
                    begin_index,
                    &input,
                    &serializable_result,
                )
                .await?;
            }
            result
        } else {
            let (_, oplog_entry) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::ImportedFunctionInvoked,
                OplogEntry::ImportedFunctionInvokedV1
            )?;
            DurableWorkerCtx::<Ctx>::validate_oplog_entry(&oplog_entry, function_name)?;
            let response: Result<SerializableSuccess, SerializableErr> =
                DurableWorkerCtx::<Ctx>::default_load(self.state.oplog.clone(), &oplog_entry).await;

            self.state
                .end_function(&wrapped_function_type, begin_index)
                .await?;

            response
                .map(|serialized_success| serialized_success.into())
                .map_err(|serialized_err| serialized_err.into())
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub async fn default_load<SerializableSuccess, SerializableErr>(
        oplog: Arc<dyn Oplog + Send + Sync>,
        entry: &OplogEntry,
    ) -> Result<SerializableSuccess, SerializableErr>
    where
        SerializableSuccess: Encode + Decode + Debug + Send + Sync,
        SerializableErr: Encode + Decode + Debug + From<GolemError> + Send + Sync,
    {
        oplog
            .get_payload_of_entry::<Result<SerializableSuccess, SerializableErr>>(entry)
            .await
            .map_err(|err| {
                GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
            })?
            .unwrap()
    }

    pub async fn try_default_load<SerializableSuccess, SerializableErr>(
        oplog: Arc<dyn Oplog + Send + Sync>,
        entry: &OplogEntry,
    ) -> Result<Result<SerializableSuccess, SerializableErr>, GolemError>
    where
        SerializableSuccess: Encode + Decode + Debug + Send + Sync,
        SerializableErr: Encode + Decode + Debug + From<GolemError> + Send + Sync,
    {
        oplog
            .get_payload_of_entry::<Result<SerializableSuccess, SerializableErr>>(entry)
            .await
            .map_err(|err| {
                GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
            })
            .map(|result| result.unwrap())
    }

    async fn write_to_oplog<SerializedInput, SerializedSuccess, SerializedErr>(
        &mut self,
        wrapped_function_type: &WrappedFunctionType,
        function_name: &str,
        begin_index: OplogIndex,
        serializable_input: &SerializedInput,
        serializable_result: &Result<SerializedSuccess, SerializedErr>,
    ) -> Result<(), GolemError>
    where
        SerializedInput: Encode + Debug + Send + Sync,
        SerializedSuccess: Encode + Debug + Send + Sync,
        SerializedErr: Encode + Debug + Send + Sync,
    {
        if self.state.persistence_level != PersistenceLevel::PersistNothing {
            self.state
                .oplog
                .add_imported_function_invoked(
                    function_name.to_string(),
                    &serializable_input,
                    &serializable_result,
                    wrapped_function_type.clone(),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to serialize and store function request ({:?}) and response ({:?}): {err}",
                        serializable_input,
                        serializable_result
                    )
                });
            self.state
                .end_function(wrapped_function_type, begin_index)
                .await?;
            if *wrapped_function_type == WrappedFunctionType::WriteRemote
                || matches!(
                    *wrapped_function_type,
                    WrappedFunctionType::WriteRemoteBatched(_)
                )
            {
                self.state.oplog.commit(CommitLevel::DurableOnly).await;
            }
        }
        Ok(())
    }

    fn validate_oplog_entry(
        oplog_entry: &OplogEntry,
        expected_function_name: &str,
    ) -> Result<(), GolemError> {
        if let OplogEntry::ImportedFunctionInvoked { function_name, .. } = oplog_entry {
            if function_name != expected_function_name {
                error!(
                    "Unexpected imported function call entry in oplog: expected {}, got {}",
                    expected_function_name, function_name
                );
                Err(GolemError::unexpected_oplog_entry(
                    expected_function_name,
                    function_name,
                ))
            } else {
                Ok(())
            }
        } else if let OplogEntry::ImportedFunctionInvokedV1 { function_name, .. } = oplog_entry {
            if function_name != expected_function_name {
                error!(
                    "Unexpected imported function call entry in oplog: expected {}, got {}",
                    expected_function_name, function_name
                );
                Err(GolemError::unexpected_oplog_entry(
                    expected_function_name,
                    function_name,
                ))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}
