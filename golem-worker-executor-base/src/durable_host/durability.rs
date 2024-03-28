use crate::durable_host::DurableWorkerCtx;
use crate::error::GolemError;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use golem_common::model::oplog::{OplogEntry, WrappedFunctionType};
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

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
        Err: Send,
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
        SerializedSuccess: Encode + Decode + DeserializeOwned + Debug + Send,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send;

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
        Err: Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializedSuccess:
            Encode + Decode + DeserializeOwned + From<Success> + Into<Success> + Debug + Send,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send;
}

#[async_trait]
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
        Success: Clone + Send,
        Err: Send,
        AsyncFn: for<'b> FnOnce(
                &'b mut DurableWorkerCtx<Ctx>,
            )
                -> Pin<Box<dyn Future<Output = Result<Success, Err>> + 'b + Send>>
            + Send,
        SerializedSuccess:
            Encode + Decode + DeserializeOwned + From<Success> + Into<Success> + Debug + Send,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send,
    {
        self.state.consume_hint_entries().await;
        if self.state.is_live() {
            let result = function(self).await;
            let serializable_result: Result<SerializedSuccess, SerializedErr> = result
                .as_ref()
                .map(|result| result.clone().into())
                .map_err(|err| err.into());
            let oplog_entry = OplogEntry::imported_function_invoked(
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
            self.state.set_oplog_entry(oplog_entry).await;
            if matches!(wrapped_function_type, WrappedFunctionType::WriteRemote) {
                self.state.commit_oplog().await;
            }
            result
        } else {
            let oplog_entry =
                crate::get_oplog_entry!(self.state, OplogEntry::ImportedFunctionInvoked)
                    .map_err(|err| Into::<SerializedErr>::into(err).into())?;
            let response = oplog_entry
                .response::<Result<SerializedSuccess, SerializedErr>>()
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to deserialize function response: {:?}: {err}",
                        oplog_entry
                    )
                })
                .unwrap();

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
        Success: Send,
        Err: Send,
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
        SerializedSuccess: Encode + Decode + DeserializeOwned + Debug + Send,
        SerializedErr: Encode
            + Decode
            + DeserializeOwned
            + for<'b> From<&'b Err>
            + From<GolemError>
            + Into<Err>
            + Debug
            + Send,
    {
        self.state.consume_hint_entries().await;
        if self.state.is_live() {
            let result = function(self).await;
            let serializable_result: Result<SerializedSuccess, SerializedErr> = result
                .as_ref()
                .map_err(|err| err.into())
                .and_then(|result| get_serializable(self, result).map_err(|err| (&err).into()));

            let oplog_entry = OplogEntry::imported_function_invoked(
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
            self.state.set_oplog_entry(oplog_entry).await;
            if matches!(wrapped_function_type, WrappedFunctionType::WriteRemote) {
                self.state.commit_oplog().await;
            }
            result
        } else {
            let oplog_entry =
                crate::get_oplog_entry!(self.state, OplogEntry::ImportedFunctionInvoked)
                    .map_err(|err| Into::<SerializedErr>::into(err).into())?;
            let response = oplog_entry
                .response::<Result<SerializedSuccess, SerializedErr>>()
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to deserialize function response: {:?}: {err}",
                        oplog_entry
                    )
                })
                .unwrap();

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
