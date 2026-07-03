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
use std::any::{Any, type_name};

use wasmtime::component::{Accessor, HasSelf, Resource, StreamReader};
use wasmtime_wasi::IoView;

use crate::durable_host::blobstore::classify_blob_store_error;
use crate::durable_host::blobstore::types::{
    ContainerEntry, IncomingValueEntry, OutgoingValueEntry,
};
use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::metrics::storage::{
    STORAGE_TYPE_BLOB_STORE, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::preview2::wasi::blobstore::container::{
    Container, ContainerMetadata, Error, Host, HostContainer, HostContainerWithStore,
    IncomingValue, ObjectMetadata, ObjectName, OutgoingValue,
};
use crate::workerctx::WorkerCtx;

fn durable_worker_ctx_from_self<Ctx: WorkerCtx, T: 'static>(
    data: &mut T,
) -> &mut DurableWorkerCtx<Ctx> {
    (data as &mut dyn Any)
        .downcast_mut::<DurableWorkerCtx<Ctx>>()
        .unwrap_or_else(|| {
            panic!(
                "durable blobstore wrapper registered with unexpected store data type: expected {}, got {}",
                type_name::<DurableWorkerCtx<Ctx>>(),
                type_name::<T>(),
            )
        })
}

async fn list_objects_durable_access<Ctx: WorkerCtx, T: 'static>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    container: Resource<Container>,
) -> anyhow::Result<Result<Vec<ObjectName>, Error>> {
    let (environment_id, container_name, blob_store_service) = accessor.with(|mut host| {
        let ctx = host.get();
        let environment_id = ctx.state.owned_agent_id.environment_id();
        let container_name = ctx
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        Ok::<_, anyhow::Error>((
            environment_id,
            container_name,
            ctx.state.blob_store_service.clone(),
        ))
    })?;

    let mut handle = CallHandle::<BlobstoreContainerListObject, NotCancellable>::start_access(
        accessor,
        durable_worker_ctx_from_self::<Ctx, T>,
        HostRequestBlobStoreContainer {
            container: container_name.clone(),
        },
        DurableFunctionType::ReadRemote,
    )
    .await?;

    let result = 'resp: {
        if !handle.is_live() {
            match handle
                .replay_access(accessor, durable_worker_ctx_from_self::<Ctx, T>)
                .await?
            {
                CallReplayOutcome::Replayed(response) => break 'resp response,
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
        }

        let result = blob_store_service
            .list_objects(environment_id, container_name)
            .await;
        handle
            .complete_access(
                accessor,
                durable_worker_ctx_from_self::<Ctx, T>,
                HostResponseBlobStoreListObjects {
                    result: result.map_err(|err| err.to_string()),
                },
            )
            .await?
    };

    Ok(result.result)
}

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
        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let mut handle = CallHandle::<BlobstoreContainerGetData, NotCancellable>::start(
            self,
            HostRequestBlobStoreGetData {
                container: container_name.clone(),
                object: name.clone(),
                begin: start,
                end,
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

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
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle
                .complete(
                    self,
                    HostResponseBlobStoreGetData {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?
        };
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
        let length = data.len() as u64;

        let mut handle = CallHandle::<BlobstoreContainerWriteData, NotCancellable>::start(
            self,
            HostRequestBlobStoreWriteData {
                container: container_name.clone(),
                object: name.clone(),
                length,
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

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
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_bytes_written(
                    STORAGE_TYPE_BLOB_STORE,
                    &account_id,
                    &environment_id_str,
                    length,
                );
                record_storage_objects_written(
                    STORAGE_TYPE_BLOB_STORE,
                    &account_id,
                    &environment_id_str,
                    1,
                );
            }
            handle
                .complete(
                    self,
                    HostResponseBlobStoreUnit {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?
        };
        Ok(result.result)
    }

    async fn delete_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<(), Error>> {
        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let mut handle = CallHandle::<BlobstoreContainerDeleteObject, NotCancellable>::start(
            self,
            HostRequestBlobStoreContainerAndObject {
                container: container_name.clone(),
                object: name.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        // Unlike the other sites a durability error here is softened into a guest-visible `Error`
        // rather than propagated, so `complete`/`replay` are matched instead of `?`-ed.
        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await {
                    Ok(CallReplayOutcome::Replayed(response)) => break 'resp Ok(response),
                    Ok(CallReplayOutcome::Incomplete(live)) => handle = live,
                    Err(err) => break 'resp Err(err),
                }
            }

            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .delete_object(environment_id, container_name.clone(), name.clone())
                    .await;
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_objects_deleted(
                    STORAGE_TYPE_BLOB_STORE,
                    &account_id,
                    &environment_id_str,
                    1,
                );
            }
            handle
                .complete(
                    self,
                    HostResponseBlobStoreUnit {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await
                // Softened to a guest-visible error below (never trapped), so the call-owned trap
                // marker is irrelevant here: keep the inner `WorkerExecutorError`.
                .map_err(|e| e.source)
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
        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let count = names.len() as u64;

        let mut handle = CallHandle::<BlobstoreContainerDeleteObjects, NotCancellable>::start(
            self,
            HostRequestBlobStoreContainerAndObjects {
                container: container_name.clone(),
                objects: names.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .delete_objects(environment_id, container_name.clone(), names.clone())
                    .await;
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_objects_deleted(
                    STORAGE_TYPE_BLOB_STORE,
                    &account_id,
                    &environment_id_str,
                    count,
                );
            }
            handle
                .complete(
                    self,
                    HostResponseBlobStoreUnit {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?
        };

        Ok(result.result)
    }

    async fn has_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<bool, Error>> {
        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let mut handle = CallHandle::<BlobstoreContainerHasObject, NotCancellable>::start(
            self,
            HostRequestBlobStoreContainerAndObject {
                container: container_name.clone(),
                object: name.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .has_object(environment_id, container_name.clone(), name.clone())
                    .await;
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle
                .complete(
                    self,
                    HostResponseBlobStoreContains {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?
        };

        Ok(result.result)
    }

    async fn object_info(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata, Error>> {
        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let mut handle = CallHandle::<BlobstoreContainerObjectInfo, NotCancellable>::start(
            self,
            HostRequestBlobStoreContainerAndObject {
                container: container_name.clone(),
                object: name.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .object_info(environment_id, container_name.clone(), name.clone())
                    .await;
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle
                .complete(
                    self,
                    HostResponseBlobStoreObjectMetadata {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?
        };

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
        let environment_id = self.state.owned_agent_id.environment_id();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;

        let mut handle = CallHandle::<BlobstoreContainerClear, NotCancellable>::start(
            self,
            HostRequestBlobStoreContainer {
                container: container_name.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = loop {
                let result = self
                    .state
                    .blob_store_service
                    .clear(environment_id, container_name.clone())
                    .await;
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            if result.is_ok() {
                let account_id = self.created_by().to_string();
                let environment_id_str = environment_id.to_string();
                record_storage_objects_deleted(
                    STORAGE_TYPE_BLOB_STORE,
                    &account_id,
                    &environment_id_str,
                    1,
                );
            }
            handle
                .complete(
                    self,
                    HostResponseBlobStoreUnit {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?
        };

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

impl<U: Send + 'static, Ctx: WorkerCtx> HostContainerWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn list_objects(
        accessor: &Accessor<U, Self>,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<StreamReader<ObjectName>, Error>> {
        let result = list_objects_durable_access(accessor, container).await?;

        match result {
            Ok(objects) => accessor.with(|mut host| Ok(Ok(StreamReader::new(&mut host, objects)?))),
            Err(err) => Ok(Err(err)),
        }
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
