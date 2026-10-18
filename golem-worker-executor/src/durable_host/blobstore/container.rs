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
use wasmtime::component::{Accessor, HasSelf, Resource, StreamReader};
use wasmtime_wasi::IoView;

use crate::durable_host::blobstore::types::{
    ContainerEntry, IncomingValueEntry, OutgoingValueEntry,
};
use crate::durable_host::blobstore::{
    BlobProviderOperation, authorize_targets, await_blob_provider, classify_blob_store_error,
    container_target, object_target,
};
use crate::durable_host::concurrent::{
    CallReplayOutcome, DurableCallSession, NotCancellable, ResolvedCall,
    reserve_invocation_id_before_await,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::metrics::storage::{
    STORAGE_TYPE_BLOB_STORE, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::preview2::wasi::blobstore::container::{
    Container, ContainerMetadata, Error, Host, HostContainer, HostContainerWithStore,
    IncomingValue, ObjectMetadata, ObjectName, OutgoingValue,
};
use crate::services::blob_store::require_known_blob_result;
use crate::workerctx::WorkerCtx;
use golem_common::model::card::BlobVerb;

async fn list_objects_durable_access<Ctx: WorkerCtx, T: 'static>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    container: Resource<Container>,
) -> anyhow::Result<Result<Vec<ObjectName>, Error>> {
    reserve_invocation_id_before_await(accessor, accessor.getter())?;
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

    let targets = accessor.with(|mut host| {
        let ctx = host.get();
        container_target(ctx, BlobVerb::List, &container_name).map(|target| vec![target])
    });

    let request = HostRequestBlobStoreContainer {
        container: container_name.clone(),
    };
    let result =
        DurableCallSession::<BlobstoreContainerListObject, NotCancellable>::invoke_deniable_access(
            accessor,
            accessor.getter(),
            request,
            DurableFunctionType::ReadRemote,
            targets,
            async || {
                let interrupt = accessor.with(|mut host| host.get().create_interrupt_signal());
                let result = await_blob_provider(
                    BlobProviderOperation::Read,
                    interrupt,
                    blob_store_service.list_objects(environment_id, container_name),
                )
                .await
                .map_err(anyhow::Error::new)?;
                Ok::<_, anyhow::Error>(HostResponseBlobStoreListObjects {
                    result: result.map_err(|err| err.to_string()),
                })
            },
        )
        .await?;

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

        let begun = DurableCallSession::<BlobstoreContainerGetData, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let (result, admission) = 'resp: {
            let (mut handle, admission) = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let authorization =
                        match object_target(self, BlobVerb::Read, &container_name, &name) {
                            Ok(target) => authorize_targets(self, &[target]).await?,
                            Err(err) => Err(err),
                        };
                    let request = HostRequestBlobStoreGetData {
                        container: container_name.clone(),
                        object: name.clone(),
                        begin: start,
                        end,
                    };
                    match authorization {
                        Ok(permit) => (begun.start_live(self, request).await?, Some(permit)),
                        Err(denial) => {
                            break 'resp (
                                begun.start_live_denied(self, request, denial).await?,
                                None,
                            );
                        }
                    }
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp (response, None),
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                    (handle, None)
                }
            };

            let result = loop {
                let result = await_blob_provider(
                    BlobProviderOperation::Read,
                    self.create_interrupt_signal(),
                    self.state.blob_store_service.get_data(
                        environment_id,
                        container_name.clone(),
                        name.clone(),
                        start,
                        end,
                    ),
                )
                .await
                .map_err(|kind| handle.trap(anyhow::Error::new(kind)))?;
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_blob_store_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let response = handle
                .complete(
                    self,
                    HostResponseBlobStoreGetData {
                        result: result.map_err(|err| err.to_string()),
                    },
                )
                .await?;
            (response, admission)
        };
        match result.result {
            Ok(get_data) => {
                let incoming_value = self
                    .as_wasi_view()
                    .table()
                    .push(IncomingValueEntry::new(get_data, admission))?;
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
        let begun = DurableCallSession::<BlobstoreContainerWriteData, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            let (mut handle, environment_id, container_name, data, length) =
                match begun.resolve(self).await? {
                    ResolvedCall::Live(begun) => {
                        let environment_id = self.state.owned_agent_id.environment_id();
                        let container_name = self
                            .as_wasi_view()
                            .table()
                            .get::<ContainerEntry>(&container)
                            .map(|container_entry| container_entry.name.clone())?;
                        let target = object_target(self, BlobVerb::Write, &container_name, &name);
                        let denied = match target {
                            Ok(target) => authorize_targets(self, &[target]).await?.err(),
                            Err(err) => Some(err),
                        };
                        let body = self
                            .as_wasi_view()
                            .table()
                            .get::<OutgoingValueEntry>(&data)?
                            .body
                            .clone();
                        if let Some(denial) = denied {
                            let request = HostRequestBlobStoreWriteData {
                                container: container_name,
                                object: name,
                                length: body.read().unwrap().len() as u64,
                            };
                            break 'resp begun.start_live_denied(self, request, denial).await?;
                        }
                        let data = body.read().unwrap().clone();
                        let length = data.len() as u64;
                        let request = HostRequestBlobStoreWriteData {
                            container: container_name.clone(),
                            object: name.clone(),
                            length,
                        };
                        // Persist Start before quota admission and storage I/O. A completed call
                        // replays its recorded response without charging again, while an incomplete
                        // retry resumes under the same Start before repeating the live effect.
                        (
                            begun.start_live(self, request).await?,
                            environment_id,
                            container_name,
                            data,
                            length,
                        )
                    }
                    ResolvedCall::Replay(mut handle) => {
                        match handle.replay(self).await? {
                            CallReplayOutcome::Replayed(response) => break 'resp response,
                            CallReplayOutcome::Incomplete(live) => handle = live,
                        }
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
                            .map(|entry| entry.body.read().unwrap().clone())?;
                        let length = data.len() as u64;
                        (handle, environment_id, container_name, data, length)
                    }
                };

            let result = loop {
                let result = await_blob_provider(
                    BlobProviderOperation::Mutation,
                    self.create_interrupt_signal(),
                    self.state.blob_store_service.write_data(
                        self.account_resource_limits(),
                        environment_id,
                        &container_name,
                        &name,
                        &data,
                    ),
                )
                .await
                .map_err(|kind| handle.trap(anyhow::Error::new(kind)))?;
                let result = require_known_blob_result(result)
                    .map_err(|failure| handle.trap(anyhow::Error::new(failure)))?;
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
                        result: result.map(|_| ()).map_err(|err| err.to_string()),
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

        let begun = DurableCallSession::<BlobstoreContainerDeleteObject, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        // Unlike the other sites a durability error of an allowed call is softened into a
        // guest-visible `Error` rather than propagated, so `complete`/`replay` are matched instead
        // of `?`-ed.
        let result = 'resp: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = match object_target(self, BlobVerb::Delete, &container_name, &name)
                    {
                        Ok(target) => authorize_targets(self, &[target]).await?.err(),
                        Err(err) => Some(err),
                    };
                    let request = HostRequestBlobStoreContainerAndObject {
                        container: container_name.clone(),
                        object: name.clone(),
                    };
                    if let Some(denial) = denied {
                        break 'resp Ok(begun.start_live_denied(self, request, denial).await?);
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await {
                        Ok(CallReplayOutcome::Replayed(response)) => break 'resp Ok(response),
                        Ok(CallReplayOutcome::Incomplete(live)) => handle = live,
                        Err(err) => break 'resp Err(err),
                    }
                    handle
                }
            };

            let result = loop {
                let result = await_blob_provider(
                    BlobProviderOperation::Mutation,
                    self.create_interrupt_signal(),
                    self.state.blob_store_service.delete_object(
                        self.account_resource_limits(),
                        environment_id,
                        container_name.clone(),
                        name.clone(),
                    ),
                )
                .await
                .map_err(|kind| handle.trap(anyhow::Error::new(kind)))?;
                let result = require_known_blob_result(result)
                    .map_err(|failure| handle.trap(anyhow::Error::new(failure)))?;
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
                    result
                        .as_ref()
                        .map_or(0, |mutation| mutation.objects_deleted),
                );
            }
            handle
                .complete(
                    self,
                    HostResponseBlobStoreUnit {
                        result: result.map(|_| ()).map_err(|err| err.to_string()),
                    },
                )
                .await
                // Softened to a guest-visible error below (never trapped), so the call-owned trap
                // marker is irrelevant here: keep the inner `WorkerExecutorError`.
                .map_err(|e| e.source)
        };

        match result {
            Ok(response) => Ok(response.result),
            Err(error) => Ok(Err(format!("{error:?}"))),
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

        let begun = DurableCallSession::<BlobstoreContainerDeleteObjects, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let targets = names
                        .iter()
                        .map(|name| object_target(self, BlobVerb::Delete, &container_name, name))
                        .collect::<Result<Vec<_>, _>>();
                    let denied = match targets {
                        Ok(targets) => authorize_targets(self, &targets).await?.err(),
                        Err(err) => Some(err),
                    };
                    let request = HostRequestBlobStoreContainerAndObjects {
                        container: container_name.clone(),
                        objects: names.clone(),
                    };
                    if let Some(denial) = denied {
                        break 'resp begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                    handle
                }
            };

            let result = loop {
                let result = await_blob_provider(
                    BlobProviderOperation::Mutation,
                    self.create_interrupt_signal(),
                    self.state.blob_store_service.delete_objects(
                        self.account_resource_limits(),
                        environment_id,
                        &container_name,
                        &names,
                    ),
                )
                .await
                .map_err(|kind| handle.trap(anyhow::Error::new(kind)))?;
                let result = require_known_blob_result(result)
                    .map_err(|failure| handle.trap(anyhow::Error::new(failure)))?;
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
                    result
                        .as_ref()
                        .map_or(count, |mutation| mutation.objects_deleted),
                );
            }
            handle
                .complete(
                    self,
                    HostResponseBlobStoreUnit {
                        result: result.map(|_| ()).map_err(|err| err.to_string()),
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

        let begun = DurableCallSession::<BlobstoreContainerHasObject, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'resp: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = match object_target(self, BlobVerb::Read, &container_name, &name) {
                        Ok(target) => authorize_targets(self, &[target]).await?.err(),
                        Err(err) => Some(err),
                    };
                    let request = HostRequestBlobStoreContainerAndObject {
                        container: container_name.clone(),
                        object: name.clone(),
                    };
                    if let Some(denial) = denied {
                        break 'resp begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                    handle
                }
            };

            let result = loop {
                let result = await_blob_provider(
                    BlobProviderOperation::Read,
                    self.create_interrupt_signal(),
                    self.state.blob_store_service.has_object(
                        environment_id,
                        container_name.clone(),
                        name.clone(),
                    ),
                )
                .await
                .map_err(|kind| handle.trap(anyhow::Error::new(kind)))?;
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

        let begun = DurableCallSession::<BlobstoreContainerObjectInfo, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'resp: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = match object_target(self, BlobVerb::Read, &container_name, &name) {
                        Ok(target) => authorize_targets(self, &[target]).await?.err(),
                        Err(err) => Some(err),
                    };
                    let request = HostRequestBlobStoreContainerAndObject {
                        container: container_name.clone(),
                        object: name.clone(),
                    };
                    if let Some(denial) = denied {
                        break 'resp begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                    handle
                }
            };

            let result = loop {
                let result = await_blob_provider(
                    BlobProviderOperation::Read,
                    self.create_interrupt_signal(),
                    self.state.blob_store_service.object_info(
                        environment_id,
                        container_name.clone(),
                        name.clone(),
                    ),
                )
                .await
                .map_err(|kind| handle.trap(anyhow::Error::new(kind)))?;
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

        let begun = DurableCallSession::<BlobstoreContainerClear, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'resp: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = match container_target(self, BlobVerb::Delete, &container_name) {
                        Ok(target) => authorize_targets(self, &[target]).await?.err(),
                        Err(err) => Some(err),
                    };
                    let request = HostRequestBlobStoreContainer {
                        container: container_name.clone(),
                    };
                    if let Some(denial) = denied {
                        break 'resp begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(mut handle) => {
                    match handle.replay(self).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                    handle
                }
            };

            let result = loop {
                let result = await_blob_provider(
                    BlobProviderOperation::Mutation,
                    self.create_interrupt_signal(),
                    self.state.blob_store_service.clear(
                        self.account_resource_limits(),
                        environment_id,
                        container_name.clone(),
                    ),
                )
                .await
                .map_err(|kind| handle.trap(anyhow::Error::new(kind)))?;
                let result = require_known_blob_result(result)
                    .map_err(|failure| handle.trap(anyhow::Error::new(failure)))?;
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
                    result
                        .as_ref()
                        .map_or(0, |mutation| mutation.objects_deleted),
                );
            }
            handle
                .complete(
                    self,
                    HostResponseBlobStoreUnit {
                        result: result.map(|_| ()).map_err(|err| err.to_string()),
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
