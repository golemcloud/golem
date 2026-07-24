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

use anyhow::anyhow;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::host_functions::{
    P3KeyvalueCacheDelete, P3KeyvalueCacheExists, P3KeyvalueCacheGet, P3KeyvalueCacheGetOrSet,
    P3KeyvalueCacheSet, P3KeyvalueCacheVacancyDrop, P3KeyvalueCacheVacancyFill,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestKVCacheKey, HostRequestKVCacheKeyAndTtl,
    HostRequestKVCacheKeyValueAndTtl, HostResponseKVDelete, HostResponseKVGet, HostResponseKVUnit,
};
use wasmtime::component::{Accessor, HasSelf, Resource};

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable};
use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{
    CACHE_BUCKET, CacheFillState, IncomingValueEntry, OutgoingValueEntry,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::cache::{
    Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry,
    Host, HostFutureExistsResult, HostFutureExistsResultWithStore, HostFutureGetOrSetResult,
    HostFutureGetOrSetResultWithStore, HostFutureGetResult, HostFutureGetResultWithStore,
    HostFutureResult, HostFutureResultWithStore, HostVacancy, HostVacancyWithStore, HostWithStore,
    IncomingValue, Key, OutgoingValue, Vacancy,
};
use crate::workerctx::WorkerCtx;

pub struct CacheFutureGetResultEntry {
    handle: Option<CallHandle<P3KeyvalueCacheGet, Cancellable>>,
    environment_id: EnvironmentId,
    key: Key,
}

pub struct CacheFutureExistsResultEntry {
    handle: Option<CallHandle<P3KeyvalueCacheExists, Cancellable>>,
    environment_id: EnvironmentId,
    key: Key,
}

pub struct CacheFutureResultEntry {
    operation: CacheFutureResultOperation,
}

pub struct CacheFutureGetOrSetResultEntry {
    handle: Option<CallHandle<P3KeyvalueCacheGetOrSet, Cancellable>>,
    environment_id: EnvironmentId,
    key: Key,
}

pub struct CacheVacancyEntry {
    environment_id: EnvironmentId,
    key: Key,
    filled: bool,
}

enum CacheFutureResultOperation {
    Set {
        handle: Option<CallHandle<P3KeyvalueCacheSet, Cancellable>>,
        environment_id: EnvironmentId,
        key: Key,
        value: Vec<u8>,
    },
    Delete {
        handle: Option<CallHandle<P3KeyvalueCacheDelete, Cancellable>>,
        environment_id: EnvironmentId,
        key: Key,
    },
}

fn missing_handle(function: &str) -> anyhow::Error {
    anyhow!("keyvalue::cache::{function} future result already consumed")
}

fn push_error<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    error: String,
) -> anyhow::Result<Resource<Error>> {
    Ok(ctx.table().push(ErrorEntry::new(error))?)
}

fn push_incoming_value<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    value: Vec<u8>,
) -> anyhow::Result<Resource<IncomingValue>> {
    Ok(ctx.table().push(IncomingValueEntry::new(value))?)
}

impl<Ctx: WorkerCtx> HostFutureGetResult for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureGetResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn drop(
        accessor: &Accessor<U, Self>,
        rep: Resource<FutureGetResult>,
    ) -> anyhow::Result<()> {
        let handle = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::future_get", "drop");
            Ok::<_, anyhow::Error>(ctx.table().delete(rep)?.handle)
        })?;
        if let Some(handle) = handle {
            handle
                .cancel_access(accessor, accessor.getter(), None)
                .await?;
        }
        Ok(())
    }

    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Result<Option<Resource<IncomingValue>>, Resource<Error>>> {
        let (mut handle, environment_id, key, key_value_service) =
            accessor.with(|mut access| {
                let ctx = access.get();
                let entry = ctx.table().get_mut(&self_)?;
                Ok::<_, anyhow::Error>((
                    entry.handle.take().ok_or_else(|| missing_handle("get"))?,
                    entry.environment_id,
                    entry.key.clone(),
                    ctx.state.key_value_service.clone(),
                ))
            })?;

        let response = 'resp: {
            if !handle.is_live() {
                match handle.replay_access(accessor, accessor.getter()).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = key_value_service
                .get(environment_id, CACHE_BUCKET.to_string(), key)
                .await
                .map_err(|err| err.to_string());
            handle
                .complete_access(accessor, accessor.getter(), HostResponseKVGet { result })
                .await?
        };

        accessor.with(|mut access| match response.result {
            Ok(Some(value)) => Ok(Ok(Some(push_incoming_value(access.get(), value)?))),
            Ok(None) => Ok(Ok(None)),
            Err(error) => Ok(Err(push_error(access.get(), error)?)),
        })
    }
}

impl<Ctx: WorkerCtx> HostFutureExistsResult for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureExistsResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn drop(
        accessor: &Accessor<U, Self>,
        rep: Resource<FutureExistsResult>,
    ) -> anyhow::Result<()> {
        let handle = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::future_exists", "drop");
            Ok::<_, anyhow::Error>(ctx.table().delete(rep)?.handle)
        })?;
        if let Some(handle) = handle {
            handle
                .cancel_access(accessor, accessor.getter(), None)
                .await?;
        }
        Ok(())
    }

    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        let (mut handle, environment_id, key, key_value_service) =
            accessor.with(|mut access| {
                let ctx = access.get();
                let entry = ctx.table().get_mut(&self_)?;
                Ok::<_, anyhow::Error>((
                    entry
                        .handle
                        .take()
                        .ok_or_else(|| missing_handle("exists"))?,
                    entry.environment_id,
                    entry.key.clone(),
                    ctx.state.key_value_service.clone(),
                ))
            })?;

        let response = 'resp: {
            if !handle.is_live() {
                match handle.replay_access(accessor, accessor.getter()).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = key_value_service
                .exists(environment_id, CACHE_BUCKET.to_string(), key)
                .await
                .map_err(|err| err.to_string());
            handle
                .complete_access(accessor, accessor.getter(), HostResponseKVDelete { result })
                .await?
        };

        accessor.with(|mut access| match response.result {
            Ok(exists) => Ok(Ok(exists)),
            Err(error) => Ok(Err(push_error(access.get(), error)?)),
        })
    }
}

impl<Ctx: WorkerCtx> HostFutureResult for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn drop(accessor: &Accessor<U, Self>, rep: Resource<FutureResult>) -> anyhow::Result<()> {
        enum Handle {
            Set(CallHandle<P3KeyvalueCacheSet, Cancellable>),
            Delete(CallHandle<P3KeyvalueCacheDelete, Cancellable>),
        }

        let handle = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::future_result", "drop");
            let entry = ctx.table().delete(rep)?;
            Ok::<_, anyhow::Error>(match entry.operation {
                CacheFutureResultOperation::Set { handle, .. } => handle.map(Handle::Set),
                CacheFutureResultOperation::Delete { handle, .. } => handle.map(Handle::Delete),
            })
        })?;

        match handle {
            Some(Handle::Set(handle)) => {
                handle
                    .cancel_access(accessor, accessor.getter(), None)
                    .await?
            }
            Some(Handle::Delete(handle)) => {
                handle
                    .cancel_access(accessor, accessor.getter(), None)
                    .await?
            }
            None => {}
        }
        Ok(())
    }

    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<FutureResult>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        enum Action {
            Set {
                handle: CallHandle<P3KeyvalueCacheSet, Cancellable>,
                environment_id: EnvironmentId,
                key: Key,
                value: Vec<u8>,
            },
            Delete {
                handle: CallHandle<P3KeyvalueCacheDelete, Cancellable>,
                environment_id: EnvironmentId,
                key: Key,
            },
        }

        let (action, key_value_service) = accessor.with(|mut access| {
            let ctx = access.get();
            let entry = ctx.table().get_mut(&self_)?;
            let action = match &mut entry.operation {
                CacheFutureResultOperation::Set {
                    handle,
                    environment_id,
                    key,
                    value,
                } => Action::Set {
                    handle: handle.take().ok_or_else(|| missing_handle("set"))?,
                    environment_id: *environment_id,
                    key: key.clone(),
                    value: value.clone(),
                },
                CacheFutureResultOperation::Delete {
                    handle,
                    environment_id,
                    key,
                } => Action::Delete {
                    handle: handle.take().ok_or_else(|| missing_handle("delete"))?,
                    environment_id: *environment_id,
                    key: key.clone(),
                },
            };
            Ok::<_, anyhow::Error>((action, ctx.state.key_value_service.clone()))
        })?;

        let response = match action {
            Action::Set {
                mut handle,
                environment_id,
                key,
                value,
            } => 'resp: {
                if !handle.is_live() {
                    match handle.replay_access(accessor, accessor.getter()).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                }

                let result = key_value_service
                    .set(environment_id, CACHE_BUCKET.to_string(), key, value)
                    .await
                    .map_err(|err| err.to_string());
                handle
                    .complete_access(accessor, accessor.getter(), HostResponseKVUnit { result })
                    .await?
            }
            Action::Delete {
                mut handle,
                environment_id,
                key,
            } => 'resp: {
                if !handle.is_live() {
                    match handle.replay_access(accessor, accessor.getter()).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                }

                let result = key_value_service
                    .delete(environment_id, CACHE_BUCKET.to_string(), key)
                    .await
                    .map_err(|err| err.to_string());
                handle
                    .complete_access(accessor, accessor.getter(), HostResponseKVUnit { result })
                    .await?
            }
        };

        accessor.with(|mut access| match response.result {
            Ok(()) => Ok(Ok(())),
            Err(error) => Ok(Err(push_error(access.get(), error)?)),
        })
    }
}

impl<Ctx: WorkerCtx> HostFutureGetOrSetResult for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureGetOrSetResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn drop(
        accessor: &Accessor<U, Self>,
        rep: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<()> {
        let handle = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::future_get_or_set", "drop");
            Ok::<_, anyhow::Error>(ctx.table().delete(rep)?.handle)
        })?;
        if let Some(handle) = handle {
            handle
                .cancel_access(accessor, accessor.getter(), None)
                .await?;
        }
        Ok(())
    }

    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Result<GetOrSetEntry, Resource<Error>>> {
        let (mut handle, environment_id, key, key_value_service) =
            accessor.with(|mut access| {
                let ctx = access.get();
                let entry = ctx.table().get_mut(&self_)?;
                Ok::<_, anyhow::Error>((
                    entry
                        .handle
                        .take()
                        .ok_or_else(|| missing_handle("get_or_set"))?,
                    entry.environment_id,
                    entry.key.clone(),
                    ctx.state.key_value_service.clone(),
                ))
            })?;

        let response = 'resp: {
            if !handle.is_live() {
                match handle.replay_access(accessor, accessor.getter()).await? {
                    CallReplayOutcome::Replayed(response) => break 'resp response,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = key_value_service
                .get(environment_id, CACHE_BUCKET.to_string(), key.clone())
                .await
                .map_err(|err| err.to_string());
            handle
                .complete_access(accessor, accessor.getter(), HostResponseKVGet { result })
                .await?
        };

        accessor.with(|mut access| match response.result {
            Ok(Some(value)) => Ok(Ok(GetOrSetEntry::Occupied(push_incoming_value(
                access.get(),
                value,
            )?))),
            Ok(None) => {
                let vacancy = access.get().table().push(CacheVacancyEntry {
                    environment_id,
                    key,
                    filled: false,
                })?;
                Ok(Ok(GetOrSetEntry::Vacant(vacancy)))
            }
            Err(error) => Ok(Err(push_error(access.get(), error)?)),
        })
    }
}

impl<Ctx: WorkerCtx> HostVacancy for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostVacancyWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn vacancy_fill(
        accessor: &Accessor<U, Self>,
        self_: Resource<Vacancy>,
        ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<OutgoingValue>> {
        let (environment_id, key) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::vacancy", "vacancy_fill");
            let entry = ctx.table().get_mut(&self_)?;
            if entry.filled {
                return Err(anyhow!("keyvalue::cache vacancy is already filled"));
            }
            entry.filled = true;
            Ok::<_, anyhow::Error>((entry.environment_id, entry.key.clone()))
        })?;

        let handle = CallHandle::<P3KeyvalueCacheVacancyFill, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestKVCacheKeyAndTtl {
                key: key.clone(),
                ttl_ms,
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        accessor.with(|mut access| {
            Ok(access
                .get()
                .table()
                .push(OutgoingValueEntry::new_cache_fill(CacheFillState {
                    handle: Some(handle),
                    environment_id,
                    key,
                }))?)
        })
    }

    async fn drop(accessor: &Accessor<U, Self>, rep: Resource<Vacancy>) -> anyhow::Result<()> {
        let entry = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::vacancy", "drop");
            Ok::<_, anyhow::Error>(ctx.table().delete(rep)?)
        })?;
        if !entry.filled {
            let mut handle = CallHandle::<P3KeyvalueCacheVacancyDrop, Cancellable>::start_access(
                accessor,
                accessor.getter(),
                HostRequestKVCacheKey {
                    key: entry.key.clone(),
                },
                DurableFunctionType::WriteRemote,
            )
            .await?;

            if !handle.is_live() {
                match handle.replay_access(accessor, accessor.getter()).await? {
                    CallReplayOutcome::Replayed(_) => return Ok(()),
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            handle
                .complete_access(
                    accessor,
                    accessor.getter(),
                    HostResponseKVUnit { result: Ok(()) },
                )
                .await?;
        }
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get(
        accessor: &Accessor<U, Self>,
        k: Key,
    ) -> anyhow::Result<Resource<FutureGetResult>> {
        let environment_id = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "get");
            ctx.owned_agent_id.environment_id()
        });
        let handle = CallHandle::<P3KeyvalueCacheGet, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestKVCacheKey { key: k.clone() },
            DurableFunctionType::ReadRemote,
        )
        .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureGetResultEntry {
                handle: Some(handle),
                environment_id,
                key: k,
            })?)
        })
    }

    async fn exists(
        accessor: &Accessor<U, Self>,
        k: Key,
    ) -> anyhow::Result<Resource<FutureExistsResult>> {
        let environment_id = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "exists");
            ctx.owned_agent_id.environment_id()
        });
        let handle = CallHandle::<P3KeyvalueCacheExists, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestKVCacheKey { key: k.clone() },
            DurableFunctionType::ReadRemote,
        )
        .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureExistsResultEntry {
                handle: Some(handle),
                environment_id,
                key: k,
            })?)
        })
    }

    async fn set(
        accessor: &Accessor<U, Self>,
        k: Key,
        v: Resource<OutgoingValue>,
        ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<FutureResult>> {
        let (environment_id, value) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "set");
            Ok::<_, anyhow::Error>((
                ctx.owned_agent_id.environment_id(),
                ctx.table().get(&v)?.body.read().unwrap().clone(),
            ))
        })?;
        let handle = CallHandle::<P3KeyvalueCacheSet, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestKVCacheKeyValueAndTtl {
                key: k.clone(),
                length: value.len(),
                ttl_ms,
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureResultEntry {
                operation: CacheFutureResultOperation::Set {
                    handle: Some(handle),
                    environment_id,
                    key: k,
                    value,
                },
            })?)
        })
    }

    async fn get_or_set(
        accessor: &Accessor<U, Self>,
        k: Key,
    ) -> anyhow::Result<Resource<FutureGetOrSetResult>> {
        let environment_id = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "get_or_set");
            ctx.owned_agent_id.environment_id()
        });
        let handle = CallHandle::<P3KeyvalueCacheGetOrSet, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestKVCacheKey { key: k.clone() },
            DurableFunctionType::ReadRemote,
        )
        .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureGetOrSetResultEntry {
                handle: Some(handle),
                environment_id,
                key: k,
            })?)
        })
    }

    async fn delete(
        accessor: &Accessor<U, Self>,
        k: Key,
    ) -> anyhow::Result<Resource<FutureResult>> {
        let environment_id = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "delete");
            ctx.owned_agent_id.environment_id()
        });
        let handle = CallHandle::<P3KeyvalueCacheDelete, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestKVCacheKey { key: k.clone() },
            DurableFunctionType::WriteRemote,
        )
        .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureResultEntry {
                operation: CacheFutureResultOperation::Delete {
                    handle: Some(handle),
                    environment_id,
                    key: k,
                },
            })?)
        })
    }
}
