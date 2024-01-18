use async_trait::async_trait;
use golem_common::model::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::WasiView;

use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::durable_host::{Durability, DurableWorkerCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::keyvalue::readwrite::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<Resource<IncomingValue>, Resource<Error>>> {
        record_host_function_call("keyvalue::readwrite", "get");
        let account_id = self.private_state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let result = Durability::<Ctx, Option<Vec<u8>>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem keyvalue::readwrite::get",
            |ctx| {
                ctx.private_state.key_value_service.get(
                    account_id.clone(),
                    bucket.clone(),
                    key.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(Some(value)) => {
                let incoming_value = self
                    .as_wasi_view()
                    .table_mut()
                    .push(IncomingValueEntry::new(value))?;
                Ok(Ok(incoming_value))
            }
            Ok(None) => {
                let error = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ErrorEntry::new("Key not found".to_string()))?;
                Ok(Err(error))
            }
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
        outgoing_value: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        record_host_function_call("keyvalue::readwrite", "set");
        let account_id = self.private_state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let outgoing_value = self
            .as_wasi_view()
            .table()
            .get::<OutgoingValueEntry>(&outgoing_value)?
            .body
            .read()
            .unwrap()
            .clone();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem keyvalue::readwrite::set",
            |ctx| {
                ctx.private_state.key_value_service.set(
                    account_id.clone(),
                    bucket.clone(),
                    key.clone(),
                    outgoing_value.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn delete(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        record_host_function_call("keyvalue::readwrite", "delete");
        let account_id = self.private_state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem keyvalue::readwrite::delete",
            |ctx| {
                ctx.private_state.key_value_service.delete(
                    account_id.clone(),
                    bucket.clone(),
                    key.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn exists(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        record_host_function_call("keyvalue::readwrite", "exists");
        let account_id = self.private_state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let result = Durability::<Ctx, bool, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem keyvalue::readwrite::exists",
            |ctx| {
                ctx.private_state.key_value_service.exists(
                    account_id.clone(),
                    bucket.clone(),
                    key.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => {
                let error = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }
}
