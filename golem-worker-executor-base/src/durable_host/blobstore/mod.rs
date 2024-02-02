pub mod container;
pub mod types;

use async_trait::async_trait;
use golem_common::model::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::WasiView;

use crate::durable_host::blobstore::types::ContainerEntry;
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::blobstore::blobstore::{
    Container, ContainerName, Error, Host, ObjectId,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        record_host_function_call("blobstore::blobstore", "create_container");
        let account_id = self.private_state.account_id.clone();
        let result = Durability::<Ctx, u64, SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::create_container",
            |ctx| {
                ctx.private_state
                    .blob_store_service
                    .create_container(account_id.clone(), name.clone())
            },
        )
        .await;
        match result {
            Ok(created_at) => {
                let container = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ContainerEntry::new(name, created_at))?;
                Ok(Ok(container))
            }
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        record_host_function_call("blobstore::blobstore", "get_container");
        let account_id = self.private_state.account_id.clone();
        let result = Durability::<Ctx, Option<u64>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::blobstore::get_container",
            |ctx| {
                ctx.private_state
                    .blob_store_service
                    .get_container(account_id.clone(), name.clone())
            },
        )
        .await;
        match result {
            Ok(Some(created_at)) => {
                let container = self
                    .as_wasi_view()
                    .table_mut()
                    .push(ContainerEntry::new(name, created_at))?;
                Ok(Ok(container))
            }
            Ok(None) => Ok(Err("Container not found".to_string())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<(), Error>> {
        record_host_function_call("blobstore::blobstore", "delete_container");
        let account_id = self.private_state.account_id.clone();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::delete_container",
            |ctx| {
                ctx.private_state
                    .blob_store_service
                    .delete_container(account_id.clone(), name.clone())
            },
        )
        .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<bool, Error>> {
        record_host_function_call("blobstore::blobstore", "container_exists");
        let account_id = self.private_state.account_id.clone();
        let result = Durability::<Ctx, bool, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::blobstore::container_exists",
            |ctx| {
                ctx.private_state
                    .blob_store_service
                    .container_exists(account_id.clone(), name.clone())
            },
        )
        .await;
        match result {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        record_host_function_call("blobstore::blobstore", "copy_object");
        let account_id = self.private_state.account_id.clone();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::copy_object",
            |ctx| {
                ctx.private_state.blob_store_service.copy_object(
                    account_id.clone(),
                    src.container.clone(),
                    src.object.clone(),
                    dest.container.clone(),
                    dest.object.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        record_host_function_call("blobstore::blobstore", "move_object");
        let account_id = self.private_state.account_id.clone();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::move_object",
            |ctx| {
                ctx.private_state.blob_store_service.move_object(
                    account_id.clone(),
                    src.container.clone(),
                    src.object.clone(),
                    dest.container.clone(),
                    dest.object.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }
}
