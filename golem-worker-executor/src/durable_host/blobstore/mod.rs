// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use golem_common::model::oplog::{
    DurableFunctionType, HostRequestBlobStoreContainer, HostRequestBlobStoreCopyOrMove,
    HostResponseBlobStoreContains, HostResponseBlobStoreOptionalTimestamp,
    HostResponseBlobStoreTimestamp, HostResponseBlobStoreUnit,
};
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::durable_host::blobstore::types::ContainerEntry;
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::{Durability, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::wasi::blobstore::blobstore::{
    Container, ContainerName, Error, Host, ObjectId,
};
use crate::services::blob_store::BlobStoreError;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions;

pub(crate) fn classify_blob_store_error(err: &BlobStoreError) -> HostFailureKind {
    match err {
        BlobStoreError::NotFound(_)
        | BlobStoreError::AlreadyExists(_)
        | BlobStoreError::PermissionDenied(_)
        | BlobStoreError::InvalidInput(_) => HostFailureKind::Permanent,
        BlobStoreError::TransientBackend(_) | BlobStoreError::Other(_) => {
            HostFailureKind::Transient
        }
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        let environment_id = self.state.owned_agent_id.environment_id();
        let mut durability = Durability::<host_functions::BlobstoreBlobstoreCreateContainer>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let svc = self.state.blob_store_service.clone();
            let result = loop {
                let result = svc
                    .create_container(environment_id, name.clone())
                    .await
                    .map(|_| name.clone());
                let result = match result {
                    Ok(name) => svc
                        .get_container(environment_id, name)
                        .await
                        .map(|r| r.unwrap()),
                    Err(e) => Err(e),
                };
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = HostResponseBlobStoreTimestamp {
                result: result.map_err(|err| err.to_string()),
            };

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
        let environment_id = self.state.owned_agent_id.environment_id();
        let mut durability = Durability::<host_functions::BlobstoreBlobstoreGetContainer>::new(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .get_container(environment_id, name.clone())
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = HostResponseBlobStoreOptionalTimestamp {
                result: result.map_err(|err| err.to_string()),
            };

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
        let environment_id = self.state.owned_agent_id.environment_id();
        let mut durability = Durability::<host_functions::BlobstoreBlobstoreDeleteContainer>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .delete_container(environment_id, name.clone())
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = HostResponseBlobStoreUnit {
                result: result.map_err(|err| err.to_string()),
            };

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
        let environment_id = self.state.owned_agent_id.environment_id();
        let mut durability = Durability::<host_functions::BlobstoreBlobstoreContainerExists>::new(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .container_exists(environment_id, name.clone())
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = HostResponseBlobStoreContains {
                result: result.map_err(|err| err.to_string()),
            };

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
        let environment_id = self.state.owned_agent_id.environment_id();
        let mut durability = Durability::<host_functions::BlobstoreBlobstoreCopyObject>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let input = HostRequestBlobStoreCopyOrMove {
                source_container: src.container,
                source_object: src.object,
                target_container: dest.container,
                target_object: dest.object,
            };
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .copy_object(
                        environment_id,
                        input.source_container.clone(),
                        input.source_object.clone(),
                        input.target_container.clone(),
                        input.target_object.clone(),
                    )
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = HostResponseBlobStoreUnit {
                result: result.map_err(|err| err.to_string()),
            };

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
        let environment_id = self.state.owned_agent_id.environment_id();
        let mut durability = Durability::<host_functions::BlobstoreBlobstoreMoveObject>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = if durability.is_live() {
            let input = HostRequestBlobStoreCopyOrMove {
                source_container: src.container,
                source_object: src.object,
                target_container: dest.container,
                target_object: dest.object,
            };
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .move_object(
                        environment_id,
                        input.source_container.clone(),
                        input.source_object.clone(),
                        input.target_container.clone(),
                        input.target_object.clone(),
                    )
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = HostResponseBlobStoreUnit {
                result: result.map_err(|err| err.to_string()),
            };

            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }
}
