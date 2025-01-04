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
use crate::model::PersistenceLevel;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use golem_common::model::oplog::{OplogEntry, OplogIndex, WrappedFunctionType};
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::error;

#[async_trait]
pub trait Durability<Ctx: WorkerCtx, SerializableInput, SerializableSuccess, SerializableErr> {
    /// A version of `wrap` allowing conversion between the success value and the serialized value within the mutable worker context.
    ///
    /// This can be used to fetch/register resources.
    ///
    /// Live mode:
    ///   value|error <- function()
    ///   serialized|serialized_err <- get_serializable(value) | error.into()
    ///   write_to_oplog(serialized|serialized_err)
    ///   return value|error
    ///
    /// Replay mode:
    ///   serialized|serialized_err <- read_from_oplog(serialized|serialized_err)
    ///   value|error <- put_serializable(serialized) | serialized_err.into()
    ///   return value|error
    async fn custom_wrap<Success, Err, AsyncFn, ToSerializable, FromSerializable>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        input: SerializableInput,
        function: AsyncFn,
        to_serializable: ToSerializable,
        from_serializable: FromSerializable,
    ) -> Result<Success, Err>
    where
        Success: Send + Sync,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        ToSerializable:
            FnOnce(&mut DurableWorkerCtx<Ctx>, &Success) -> Result<SerializableSuccess, Err> + Send,
        FromSerializable: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
                SerializableSuccess,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send
            + 'static,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess: Encode + Decode + Debug + Send + Sync,
        SerializableErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync;

    /// A version of `wrap` allowing conversion between the success value and the serialized value within the mutable worker context.
    /// Deserialization from oplog is also fully customizable, which makes this version suitable to implement
    /// backward compatibility tricks.
    ///
    /// Live mode:
    ///   value|error <- function()
    ///   serialized|serialized_err <- get_serializable(value) | error.into()
    ///   write_to_oplog(serialized|serialized_err)
    ///   return value|error
    ///
    /// Replay mode:
    ///   value|error <- load(oplog)
    ///   return value|error
    async fn full_custom_wrap<
        Intermediate,
        Success,
        Err,
        AsyncFn,
        ToSerializable,
        ToResult,
        FromSerialized,
    >(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        input: SerializableInput,
        function: AsyncFn,
        to_serializable: ToSerializable,
        to_result: ToResult,
        from_serialized: FromSerialized,
    ) -> Result<Success, Err>
    where
        Intermediate: Send + Sync,
        Success: Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Intermediate, Err>> + 'b + Send>>
            + Send,
        ToSerializable: FnOnce(&mut DurableWorkerCtx<Ctx>, &Intermediate) -> Result<SerializableSuccess, Err>
            + Send,
        ToResult: FnOnce(&mut DurableWorkerCtx<Ctx>, Intermediate) -> Result<Success, Err> + Send,
        FromSerialized: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
                Arc<dyn Oplog + Send + Sync>,
                &'b OplogEntry,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess: Encode + Decode + Debug + Send + Sync,
        SerializableErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync;

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
    async fn custom_wrap<Success, Err, AsyncFn, ToSerializable, FromSerializable>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        input: SerializableInput,
        function: AsyncFn,
        to_serializable: ToSerializable,
        from_serializable: FromSerializable,
    ) -> Result<Success, Err>
    where
        Success: Send + Sync,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        ToSerializable:
            FnOnce(&mut DurableWorkerCtx<Ctx>, &Success) -> Result<SerializableSuccess, Err> + Send,
        FromSerializable: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
                SerializableSuccess,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send
            + 'static,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess: Encode + Decode + Debug + Send + Sync,
        SerializableErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync,
    {
        <DurableWorkerCtx<Ctx> as Durability<
            Ctx,
            SerializableInput,
            SerializableSuccess,
            SerializableErr,
        >>::full_custom_wrap::<Success, Success, Err, AsyncFn, ToSerializable, _, _>(
            self,
            wrapped_function_type,
            function_name,
            input,
            function,
            to_serializable,
            |_, result| Ok(result),
            |ctx, oplog, entry| {
                Box::pin(async move {
                    let response: Result<SerializableSuccess, SerializableErr> =
                        DurableWorkerCtx::<Ctx>::default_load(oplog, entry).await;
                    match response {
                        Ok(serialized_success) => {
                            let success: Success =
                                from_serializable(ctx, serialized_success).await?;
                            Ok(success)
                        }
                        Err(serialized_err) => Err(serialized_err.into()),
                    }
                })
            },
        )
        .await
    }

    async fn full_custom_wrap<
        Intermediate,
        Success,
        Err,
        AsyncFn,
        ToSerializable,
        ToResult,
        FromSerialized,
    >(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        input: SerializableInput,
        function: AsyncFn,
        to_serializable: ToSerializable,
        to_result: ToResult,
        from_serialized: FromSerialized,
    ) -> Result<Success, Err>
    where
        Intermediate: Send + Sync,
        Success: Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Intermediate, Err>> + 'b + Send>>
            + Send,
        ToSerializable: FnOnce(&mut DurableWorkerCtx<Ctx>, &Intermediate) -> Result<SerializableSuccess, Err>
            + Send,
        ToResult: FnOnce(&mut DurableWorkerCtx<Ctx>, Intermediate) -> Result<Success, Err> + Send,
        FromSerialized: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
                Arc<dyn Oplog + Send + Sync>,
                &'b OplogEntry,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializableInput: Encode + Debug + Send + Sync + 'static,
        SerializableSuccess: Encode + Decode + Debug + Send + Sync,
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
            let intermediate = function(self).await;
            let serializable_result: Result<SerializableSuccess, SerializableErr> = intermediate
                .as_ref()
                .map_err(|err| err.into())
                .and_then(|result| to_serializable(self, result).map_err(|err| (&err).into()));

            self.write_to_oplog(
                &wrapped_function_type,
                function_name,
                begin_index,
                &input,
                &serializable_result,
            )
            .await?;

            intermediate.and_then(|value| to_result(self, value))
        } else {
            let (_, oplog_entry) = crate::get_oplog_entry!(
                self.state.replay_state,
                OplogEntry::ImportedFunctionInvoked,
                OplogEntry::ImportedFunctionInvokedV1
            )?;
            DurableWorkerCtx::<Ctx>::validate_oplog_entry(&oplog_entry, function_name)?;

            let oplog = self.state.oplog.clone();
            let result = from_serialized(self, oplog, &oplog_entry).await;

            self.state
                .end_function(&wrapped_function_type, begin_index)
                .await?;

            result
        }
    }

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

    async fn write_to_oplog<SerializedInput, SerializedSuccess, Err, SerializedErr>(
        &mut self,
        wrapped_function_type: &WrappedFunctionType,
        function_name: &str,
        begin_index: OplogIndex,
        serializable_input: &SerializedInput,
        serializable_result: &Result<SerializedSuccess, SerializedErr>,
    ) -> Result<(), Err>
    where
        Err: Send,
        SerializedInput: Encode + Debug + Send + Sync,
        SerializedSuccess: Encode + Debug + Send + Sync,
        SerializedErr: Encode + Debug + From<GolemError> + Into<Err> + Send + Sync,
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
                .await
                .map_err(|err| Into::<SerializedErr>::into(err).into())?;
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
