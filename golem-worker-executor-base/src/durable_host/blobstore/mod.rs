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

pub mod container;
pub mod types;

use crate::durable_host::blobstore::types::ContainerEntry;
use crate::durable_host::DurableWorkerCtx;
use crate::preview2::wasi::blobstore::blobstore::{
    Container, ContainerName, Error, Host, ObjectId,
};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use futures_util::TryFutureExt;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        let account_id = self.state.owned_worker_id.account_id();
        let svc = self.state.blob_store_service.clone();
        let result = svc
            .create_container(account_id.clone(), name.clone())
            .and_then(|_| svc.get_container(account_id, name.clone()))
            .await
            .map(|r| r.unwrap());

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
        let account_id = self.state.owned_worker_id.account_id();

        let result = self
            .state
            .blob_store_service
            .get_container(account_id, name.clone())
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
        let account_id = self.state.owned_worker_id.account_id();
        let result = self
            .state
            .blob_store_service
            .delete_container(account_id, name.clone())
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
        let account_id = self.state.owned_worker_id.account_id();
        let result = self
            .state
            .blob_store_service
            .container_exists(account_id, name.clone())
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
        let account_id = self.state.owned_worker_id.account_id();
        let result = self
            .state
            .blob_store_service
            .copy_object(
                account_id,
                src.container,
                src.object,
                dest.container,
                dest.object,
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
        let account_id = self.state.owned_worker_id.account_id();

        let result = self
            .state
            .blob_store_service
            .move_object(
                account_id,
                src.container,
                src.object,
                dest.container,
                dest.object,
            )
            .await;

        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }
}
