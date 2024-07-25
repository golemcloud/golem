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

pub mod container;
pub mod types;

use async_trait::async_trait;
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

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
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("blobstore::blobstore", "create_container");
        let account_id = self.state.owned_worker_id.account_id();
        let name_clone = name.clone();
        let result: Result<u64, anyhow::Error> = Durability::<Ctx, u64, SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::create_container",
            |ctx| {
                Box::pin(async move {
                    let _ = ctx
                        .state
                        .blob_store_service
                        .create_container(account_id.clone(), name_clone.clone())
                        .await?;
                    Ok(ctx
                        .state
                        .blob_store_service
                        .get_container(account_id.clone(), name_clone)
                        .await?
                        .unwrap())
                })
            },
        )
        .await;
        match result {
            Ok(created_at) => {
                let container = self
                    .as_wasi_view()
                    .table()
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
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("blobstore::blobstore", "get_container");
        let account_id = self.state.owned_worker_id.account_id();
        let result = Durability::<Ctx, Option<u64>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::blobstore::get_container",
            |ctx| {
                ctx.state
                    .blob_store_service
                    .get_container(account_id.clone(), name.clone())
            },
        )
        .await;
        match result {
            Ok(Some(created_at)) => {
                let container = self
                    .as_wasi_view()
                    .table()
                    .push(ContainerEntry::new(name, created_at))?;
                Ok(Ok(container))
            }
            Ok(None) => Ok(Err("Container not found".to_string())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<(), Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("blobstore::blobstore", "delete_container");
        let account_id = self.state.owned_worker_id.account_id();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::delete_container",
            |ctx| {
                ctx.state
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
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("blobstore::blobstore", "container_exists");
        let account_id = self.state.owned_worker_id.account_id();
        let result = Durability::<Ctx, bool, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::blobstore::container_exists",
            |ctx| {
                ctx.state
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
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("blobstore::blobstore", "copy_object");
        let account_id = self.state.owned_worker_id.account_id();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::copy_object",
            |ctx| {
                ctx.state.blob_store_service.copy_object(
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
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("blobstore::blobstore", "move_object");
        let account_id = self.state.owned_worker_id.account_id();
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::blobstore::move_object",
            |ctx| {
                ctx.state.blob_store_service.move_object(
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

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        (*self).create_container(name).await
    }

    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        (*self).get_container(name).await
    }

    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<(), Error>> {
        (*self).delete_container(name).await
    }

    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<bool, Error>> {
        (*self).container_exists(name).await
    }

    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        (*self).copy_object(src, dest).await
    }

    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        (*self).move_object(src, dest).await
    }
}
