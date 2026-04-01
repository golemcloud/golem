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

use golem_common::model::oplog::host_functions::{
    BlobstoreContainerClear, BlobstoreContainerDeleteObject, BlobstoreContainerDeleteObjects,
    BlobstoreContainerGetData, BlobstoreContainerHasObject, BlobstoreContainerListObject,
    BlobstoreContainerObjectInfo, BlobstoreContainerWriteData,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestBlobStoreContainer, HostRequestBlobStoreContainerAndObject,
    HostRequestBlobStoreContainerAndObjects, HostRequestBlobStoreGetData,
    HostRequestBlobStoreWriteData, HostResponseBlobStoreContains, HostResponseBlobStoreGetData,
    HostResponseBlobStoreListObjects, HostResponseBlobStoreObjectMetadata,
    HostResponseBlobStoreUnit,
};
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::durable_host::blobstore::classify_blob_store_error;
use crate::durable_host::blobstore::types::{
    ContainerEntry, IncomingValueEntry, OutgoingValueEntry, StreamObjectNamesEntry,
};
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::wasi::blobstore::container::{
    Container, ContainerMetadata, Error, Host, HostContainer, HostStreamObjectNames, IncomingValue,
    ObjectMetadata, ObjectName, OutgoingValue, StreamObjectNames,
};
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> HostContainer for DurableWorkerCtx<Ctx> {
    async fn name(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<String, Error>> {
        self.observe_function_call("blobstore::container::container", "name");
        let name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        Ok(Ok(name))
    }

    async fn info(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<ContainerMetadata, Error>> {
        self.observe_function_call("blobstore::container::container", "info");
        let info = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| ContainerMetadata {
                name: container_entry.name.clone(),
                created_at: container_entry.created_at,
            })?;
        Ok(Ok(info))
    }

    async fn get_data(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<Resource<IncomingValue>, Error>> {
        let mut durability =
            Durability::<BlobstoreContainerGetData>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .get_data(
                        environment_id,
                        container_name.clone(),
                        name.clone(),
                        start,
                        end,
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
            let result = HostResponseBlobStoreGetData {
                result: result.map_err(|err| err.to_string()),
            };
            durability
                .persist(
                    self,
                    HostRequestBlobStoreGetData {
                        container: container_name,
                        object: name,
                        begin: start,
                        end,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;
        match result.result {
            Ok(get_data) => {
                let incoming_value = self
                    .as_wasi_view()
                    .table()
                    .push(IncomingValueEntry::new(get_data))?;
                Ok(Ok(incoming_value))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    async fn write_data(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
        data: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<(), Error>> {
        let mut durability =
            Durability::<BlobstoreContainerWriteData>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let data = self
            .as_wasi_view()
            .table()
            .get::<OutgoingValueEntry>(&data)
            .map(|outgoing_value_entry| outgoing_value_entry.body.read().unwrap().clone())?;

        let result = if durability.is_live() {
            let length = data.len() as u64;
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .write_data(
                        environment_id,
                        container_name.clone(),
                        name.clone(),
                        data.clone(),
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
            durability
                .persist(
                    self,
                    HostRequestBlobStoreWriteData {
                        container: container_name,
                        object: name,
                        length,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;
        Ok(result.result)
    }

    async fn list_objects(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<Resource<StreamObjectNames>, Error>> {
        let mut durability =
            Durability::<BlobstoreContainerListObject>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .list_objects(environment_id, container_name.clone())
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = HostResponseBlobStoreListObjects {
                result: result.map_err(|err| err.to_string()),
            };
            durability
                .persist(
                    self,
                    HostRequestBlobStoreContainer {
                        container: container_name,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(list_objects) => {
                let stream_object_names = self
                    .as_wasi_view()
                    .table()
                    .push(StreamObjectNamesEntry::new(list_objects))?;
                Ok(Ok(stream_object_names))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    async fn delete_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<(), Error>> {
        let mut durability = Durability::<BlobstoreContainerDeleteObject>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .delete_object(environment_id, container_name.clone(), name.clone())
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
                    HostRequestBlobStoreContainerAndObject {
                        container: container_name,
                        object: name,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{e:?}"))),
        }
    }

    async fn delete_objects(
        &mut self,
        container: Resource<Container>,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<(), Error>> {
        let mut durability = Durability::<BlobstoreContainerDeleteObjects>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .delete_objects(environment_id, container_name.clone(), names.clone())
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
                    HostRequestBlobStoreContainerAndObjects {
                        container: container_name,
                        objects: names,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }

    async fn has_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<bool, Error>> {
        let mut durability =
            Durability::<BlobstoreContainerHasObject>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .has_object(environment_id, container_name.clone(), name.clone())
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
                    HostRequestBlobStoreContainerAndObject {
                        container: container_name,
                        object: name,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }

    async fn object_info(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata, Error>> {
        let mut durability =
            Durability::<BlobstoreContainerObjectInfo>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .object_info(environment_id, container_name.clone(), name.clone())
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = HostResponseBlobStoreObjectMetadata {
                result: result.map_err(|err| err.to_string()),
            };

            durability
                .persist(
                    self,
                    HostRequestBlobStoreContainerAndObject {
                        container: container_name,
                        object: name,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(object_info) => {
                let object_info = ObjectMetadata {
                    name: object_info.name,
                    container: object_info.container,
                    created_at: object_info.created_at,
                    size: object_info.size,
                };
                Ok(Ok(object_info))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    async fn clear(&mut self, container: Resource<Container>) -> anyhow::Result<Result<(), Error>> {
        let mut durability =
            Durability::<BlobstoreContainerClear>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let result = if durability.is_live() {
            let _result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .clear(environment_id, container_name.clone())
                    .await;
                match durability
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = self
                .state
                .blob_store_service
                .clear(environment_id, container_name.clone())
                .await;
            durability
                .try_trigger_retry(self, &result, classify_blob_store_error)
                .await?;

            let result = HostResponseBlobStoreUnit {
                result: result.map_err(|err| err.to_string()),
            };

            durability
                .persist(
                    self,
                    HostRequestBlobStoreContainer {
                        container: container_name,
                    },
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }

    async fn drop(&mut self, container: Resource<Container>) -> anyhow::Result<()> {
        self.observe_function_call("blobstore::container::container", "drop");
        self.as_wasi_view()
            .table()
            .delete::<ContainerEntry>(container)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostStreamObjectNames for DurableWorkerCtx<Ctx> {
    async fn read_stream_object_names(
        &mut self,
        self_: Resource<StreamObjectNames>,
        len: u64,
    ) -> anyhow::Result<Result<(Vec<ObjectName>, bool), Error>> {
        self.observe_function_call(
            "blobstore::container::stream_object_names",
            "read_stream_object_names",
        );
        let names = self
            .as_wasi_view()
            .table()
            .get::<StreamObjectNamesEntry>(&self_)
            .map(|stream_object_names_entry| stream_object_names_entry.names.clone())?;
        let mut names = names.write().unwrap();
        let mut result = Vec::new();
        for _ in 0..len {
            if let Some(name) = names.pop() {
                result.push(name);
            } else {
                return Ok(Ok((result, true)));
            }
        }
        Ok(Ok((result, false)))
    }

    async fn skip_stream_object_names(
        &mut self,
        self_: Resource<StreamObjectNames>,
        num: u64,
    ) -> anyhow::Result<Result<(u64, bool), Error>> {
        self.observe_function_call(
            "blobstore::container::stream_object_names",
            "skip_stream_object_names",
        );
        let names = self
            .as_wasi_view()
            .table()
            .get(&self_)
            .map(|stream_object_names_entry| stream_object_names_entry.names.clone())?;
        let mut names = names.write().unwrap();
        let mut result = 0;
        for _ in 0..num {
            if names.pop().is_some() {
                result += 1;
            } else {
                return Ok(Ok((result, true)));
            }
        }
        Ok(Ok((result, false)))
    }

    async fn drop(&mut self, rep: Resource<StreamObjectNames>) -> anyhow::Result<()> {
        self.observe_function_call("blobstore::container::stream_object_names", "drop");
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
