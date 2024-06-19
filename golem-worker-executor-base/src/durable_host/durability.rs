use crate::durable_host::DurableWorkerCtx;
use crate::error::GolemError;
use crate::model::PersistenceLevel;
use crate::services::oplog::OplogOps;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use golem_common::model::oplog::{OplogEntry, OplogIndex, WrappedFunctionType};
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use tracing::error;

#[async_trait]
pub trait Durability<Ctx: WorkerCtx, SerializedSuccess, SerializedErr> {
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
        Success: Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        GetFn:
            FnOnce(&mut DurableWorkerCtx<Ctx>, &Success) -> Result<SerializedSuccess, Err> + Send,
        PutFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
                SerializedSuccess,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializedSuccess: Encode + Decode + Debug + Send + Sync,
        SerializedErr: Encode
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
        SerializedSuccess: Encode + Decode + From<Success> + Into<Success> + Debug + Send + Sync,
        SerializedErr: Encode
            + Decode
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send
            + Sync;
}

#[async_trait]
impl<Ctx: WorkerCtx, SerializedSuccess: Sync, SerializedErr: Sync>
    Durability<Ctx, SerializedSuccess, SerializedErr> for DurableWorkerCtx<Ctx>
{
    async fn custom_wrap<Success, Err, AsyncFn, GetFn, PutFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
        function: AsyncFn,
        get_serializable: GetFn,
        put_serializable: PutFn,
    ) -> Result<Success, Err>
    where
        Success: Send,
        Err: From<GolemError> + Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        GetFn:
            FnOnce(&mut DurableWorkerCtx<Ctx>, &Success) -> Result<SerializedSuccess, Err> + Send,
        PutFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
                SerializedSuccess,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializedSuccess: Encode + Decode + Debug + Send,
        SerializedErr:
            Encode + Decode + for<'b> From<&'b Err> + From<GolemError> + Into<Err> + Debug + Send,
    {
        let begin_index = self
            .state
            .begin_function(&wrapped_function_type.clone())
            .await?;
        if self.state.is_live() || self.state.persistence_level == PersistenceLevel::PersistNothing
        {
            let result = function(self).await;
            let serializable_result: Result<SerializedSuccess, SerializedErr> = result
                .as_ref()
                .map_err(|err| err.into())
                .and_then(|result| get_serializable(self, result).map_err(|err| (&err).into()));

            self.write_to_oplog(
                &wrapped_function_type,
                function_name,
                begin_index,
                &serializable_result,
            )
            .await?;
            result
        } else {
            let (_, oplog_entry) =
                crate::get_oplog_entry!(self.state, OplogEntry::ImportedFunctionInvoked)?;
            DurableWorkerCtx::<Ctx>::validate_oplog_entry(&oplog_entry, function_name)?;
            let response = self
                .state
                .oplog
                .get_payload_of_entry::<Result<SerializedSuccess, SerializedErr>>(&oplog_entry)
                .await
                .map_err(|err| {
                    GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
                })?
                .unwrap();

            self.state
                .end_function(&wrapped_function_type, begin_index)
                .await?;

            match response {
                Ok(serialized_success) => {
                    let success = put_serializable(self, serialized_success).await?;
                    Ok(success)
                }
                Err(serialized_err) => Err(serialized_err.into()),
            }
        }
    }

    async fn wrap<Success, Err, AsyncFn>(
        &mut self,
        wrapped_function_type: WrappedFunctionType,
        function_name: &str,
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
        SerializedSuccess: Encode + Decode + From<Success> + Into<Success> + Debug + Send,
        SerializedErr:
            Encode + Decode + for<'b> From<&'b Err> + From<GolemError> + Into<Err> + Debug + Send,
    {
        let begin_index = self
            .state
            .begin_function(&wrapped_function_type.clone())
            .await?;
        if self.state.is_live() || self.state.persistence_level == PersistenceLevel::PersistNothing
        {
            let result = function(self).await;
            let serializable_result: Result<SerializedSuccess, SerializedErr> = result
                .as_ref()
                .map(|result| result.clone().into())
                .map_err(|err| err.into());

            self.write_to_oplog(
                &wrapped_function_type,
                function_name,
                begin_index,
                &serializable_result,
            )
            .await?;
            result
        } else {
            let (_, oplog_entry) =
                crate::get_oplog_entry!(self.state, OplogEntry::ImportedFunctionInvoked)?;
            DurableWorkerCtx::<Ctx>::validate_oplog_entry(&oplog_entry, function_name)?;
            let response = self
                .state
                .oplog
                .get_payload_of_entry::<Result<SerializedSuccess, SerializedErr>>(&oplog_entry)
                .await
                .map_err(|err| {
                    GolemError::unexpected_oplog_entry("ImportedFunctionInvoked payload", err)
                })?
                .unwrap();

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
    async fn write_to_oplog<SerializedSuccess, Err, SerializedErr>(
        &mut self,
        wrapped_function_type: &WrappedFunctionType,
        function_name: &str,
        begin_index: OplogIndex,
        serializable_result: &Result<SerializedSuccess, SerializedErr>,
    ) -> Result<(), Err>
    where
        Err: Send,
        SerializedSuccess: Encode + Debug + Send + Sync,
        SerializedErr: Encode + Debug + From<GolemError> + Into<Err> + Send + Sync,
    {
        if self.state.persistence_level != PersistenceLevel::PersistNothing {
            self.state
                .oplog
                .add_imported_function_invoked(
                    function_name.to_string(),
                    &serializable_result,
                    wrapped_function_type.clone(),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to serialize and store function response: {:?}: {err}",
                        serializable_result
                    )
                });
            self.state
                .end_function(wrapped_function_type, begin_index)
                .await
                .map_err(|err| Into::<SerializedErr>::into(err).into())?;
            if *wrapped_function_type == WrappedFunctionType::WriteRemote {
                self.state.oplog.commit().await;
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
        } else {
            Ok(())
        }
    }
}
