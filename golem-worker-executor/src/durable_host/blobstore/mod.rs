// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod container;
pub mod types;

use futures::TryFutureExt;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestBlobStoreContainer, HostRequestBlobStoreCopyOrMove,
    HostResponseBlobStoreContains, HostResponseBlobStoreOptionalTimestamp,
    HostResponseBlobStoreTimestamp, HostResponseBlobStoreUnit,
};
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::durable_host::blobstore::types::ContainerEntry;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::preview2::wasi::blobstore::blobstore::{
    Container, ContainerName, Error, Host, ObjectId,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        let environment_id = self.state.owned_worker_id.environment_id();
        let durability = Durability::<host_functions::BlobstoreBlobstoreCreateContainer>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let svc = self.state.blob_store_service.clone();
            let result = svc
                .create_container(environment_id, name.clone())
                .and_then(|_| svc.get_container(environment_id, name.clone()))
                .await
                .map(|r| r.unwrap())
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;

            let result = HostResponseBlobStoreTimestamp { result };

            durability
                .persist(
                    self,
                    HostRequestBlobStoreContainer {
                        container: name.clone(),
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(created_at) => {
                let container = self
                    .as_wasi_view()
                    .table()
                    .push(ContainerEntry::new(name, created_at))?;
                Ok(Ok(container))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        let environment_id = self.state.owned_worker_id.environment_id();
        let durability = Durability::<host_functions::BlobstoreBlobstoreGetContainer>::new(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let result = self
                .state
                .blob_store_service
                .get_container(environment_id, name.clone())
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;

            let result = HostResponseBlobStoreOptionalTimestamp { result };

            durability
                .persist(
                    self,
                    HostRequestBlobStoreContainer {
                        container: name.clone(),
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(Some(created_at)) => {
                let container = self
                    .as_wasi_view()
                    .table()
                    .push(ContainerEntry::new(name, created_at))?;
                Ok(Ok(container))
            }
            Ok(None) => Ok(Err("Container not found".to_string())),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<(), Error>> {
        let environment_id = self.state.owned_worker_id.environment_id();
        let durability = Durability::<host_functions::BlobstoreBlobstoreDeleteContainer>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let result = self
                .state
                .blob_store_service
                .delete_container(environment_id, name.clone())
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;

            let result = HostResponseBlobStoreUnit { result };

            durability
                .persist(
                    self,
                    HostRequestBlobStoreContainer { container: name },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }

    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<bool, Error>> {
        let environment_id = self.state.owned_worker_id.environment_id();
        let durability = Durability::<host_functions::BlobstoreBlobstoreContainerExists>::new(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let result = self
                .state
                .blob_store_service
                .container_exists(environment_id, name.clone())
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;

            let result = HostResponseBlobStoreContains { result };

            durability
                .persist(
                    self,
                    HostRequestBlobStoreContainer { container: name },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }

    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        let environment_id = self.state.owned_worker_id.environment_id();
        let durability = Durability::<host_functions::BlobstoreBlobstoreCopyObject>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let input = HostRequestBlobStoreCopyOrMove {
                source_container: src.container.clone(),
                source_object: src.object.clone(),
                target_container: dest.container.clone(),
                target_object: dest.object.clone(),
            };
            let result = self
                .state
                .blob_store_service
                .copy_object(
                    environment_id,
                    src.container,
                    src.object,
                    dest.container,
                    dest.object,
                )
                .await
                .map_err(|err| err.to_string());

            durability.try_trigger_retry(self, &result).await?;

            let result = HostResponseBlobStoreUnit { result };

            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }

    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        let environment_id = self.state.owned_worker_id.environment_id();
        let durability = Durability::<host_functions::BlobstoreBlobstoreMoveObject>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let input = HostRequestBlobStoreCopyOrMove {
                source_container: src.container.clone(),
                source_object: src.object.clone(),
                target_container: dest.container.clone(),
                target_object: dest.object.clone(),
            };
            let result = self
                .state
                .blob_store_service
                .move_object(
                    environment_id,
                    src.container,
                    src.object,
                    dest.container,
                    dest.object,
                )
                .await
                .map_err(|err| err.to_string());

            durability.try_trigger_retry(self, &result).await?;

            let result = HostResponseBlobStoreUnit { result };

            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }
}
