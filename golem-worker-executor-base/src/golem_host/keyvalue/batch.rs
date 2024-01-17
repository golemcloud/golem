use async_trait::async_trait;
use golem_common::model::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::{TableError, WasiView};

use crate::golem_host::keyvalue::error::ErrorEntry;
use crate::golem_host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::golem_host::{Durability, GolemCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::keyvalue::batch::{
    Bucket, Error, Host, IncomingValue, Key, Keys, OutgoingValue,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn get_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Keys,
    ) -> anyhow::Result<Result<Vec<Resource<IncomingValue>>, Resource<Error>>> {
        record_host_function_call("keyvalue::batch", "get_many");
        let account_id = self.private_state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let result = Durability::<Ctx, Vec<Option<Vec<u8>>>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem keyvalue::readwrite::get_many",
            |ctx| {
                ctx.private_state.key_value_service.get_many(
                    account_id.clone(),
                    bucket.clone(),
                    keys.clone(),
                )
            },
        )
        .await
        .map(|result| result.into_iter().collect::<Option<Vec<_>>>());
        match result {
            Ok(Some(values)) => {
                let incoming_values = values
                    .into_iter()
                    .map(|value| {
                        self.as_wasi_view()
                            .table_mut()
                            .push(IncomingValueEntry::new(value))
                    })
                    .collect::<Result<Vec<Resource<IncomingValue>>, _>>()?;
                Ok(Ok(incoming_values))
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

    async fn get_keys(&mut self, bucket: Resource<Bucket>) -> anyhow::Result<Keys> {
        record_host_function_call("keyvalue::batch", "get_keys");
        let account_id = self.private_state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let keys = Durability::<Ctx, Vec<String>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem keyvalue::readwrite::get_keys",
            |ctx| {
                ctx.private_state
                    .key_value_service
                    .get_keys(account_id.clone(), bucket.clone())
            },
        )
        .await?;
        Ok(keys)
    }

    async fn set_many(
        &mut self,
        bucket: Resource<Bucket>,
        key_values: Vec<(Key, Resource<OutgoingValue>)>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        record_host_function_call("keyvalue::batch", "set_many");
        let account_id = self.private_state.account_id.clone();
        let bucket = self
            .as_wasi_view()
            .table()
            .get::<BucketEntry>(&bucket)?
            .name
            .clone();
        let key_values = key_values
            .into_iter()
            .map(|(key, outgoing_value)| {
                let outgoing_value = self
                    .as_wasi_view()
                    .table()
                    .get::<OutgoingValueEntry>(&outgoing_value)?
                    .body
                    .read()
                    .unwrap()
                    .clone();
                Ok((key, outgoing_value))
            })
            .collect::<Result<Vec<(String, Vec<u8>)>, TableError>>()?;
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem keyvalue::readwrite::set_many",
            |ctx| {
                ctx.private_state.key_value_service.set_many(
                    account_id.clone(),
                    bucket.clone(),
                    key_values.clone(),
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

    async fn delete_many(
        &mut self,
        bucket: Resource<Bucket>,
        keys: Keys,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        record_host_function_call("keyvalue::batch", "delete_many");
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
            "golem keyvalue::readwrite::delete_many",
            |ctx| {
                ctx.private_state.key_value_service.delete_many(
                    account_id.clone(),
                    bucket.clone(),
                    keys.clone(),
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
}
