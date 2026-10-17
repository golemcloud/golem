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
use golem_common::model::card::KvVerb;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::host_functions::{
    P3KeyvalueCacheDelete, P3KeyvalueCacheExists, P3KeyvalueCacheGet, P3KeyvalueCacheGetOrSet,
    P3KeyvalueCacheSet, P3KeyvalueCacheVacancyDrop, P3KeyvalueCacheVacancyFill,
};
use golem_common::model::oplog::{
    DeniablePair, DurableFunctionType, HostRequestKVCacheKey, HostRequestKVCacheKeyAndTtl,
    HostRequestKVCacheKeyValueAndTtl, HostResponseKVDelete, HostResponseKVGet, HostResponseKVUnit,
};
use wasmtime::component::{Accessor, HasSelf, Resource};

use crate::durable_host::authorization::targets::kv_target;
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallReplayOutcome, Cancellable, DurableCallSession,
    authorize_live_permissions_at_serialized_access, reserve_invocation_id_before_await,
};
use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::keyvalue::types::{
    CACHE_BUCKET, CacheFillState, IncomingValueEntry, OutgoingValueEntry,
};
use crate::durable_host::keyvalue::{denial, environment_owner};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, LiveAuthorizationPermit};
use crate::preview2::wasi::keyvalue::cache::{
    Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry,
    Host, HostFutureExistsResult, HostFutureExistsResultWithStore, HostFutureGetOrSetResult,
    HostFutureGetOrSetResultWithStore, HostFutureGetResult, HostFutureGetResultWithStore,
    HostFutureResult, HostFutureResultWithStore, HostVacancy, HostVacancyWithStore, HostWithStore,
    IncomingValue, Key, OutgoingValue, Vacancy,
};
use crate::workerctx::WorkerCtx;

pub struct CacheFutureGetResultEntry {
    handle: Option<DurableCallSession<P3KeyvalueCacheGet, Cancellable>>,
    environment_id: EnvironmentId,
    key: Key,
    denied: Option<HostResponseKVGet>,
}

pub struct CacheFutureExistsResultEntry {
    handle: Option<DurableCallSession<P3KeyvalueCacheExists, Cancellable>>,
    environment_id: EnvironmentId,
    key: Key,
    denied: Option<HostResponseKVDelete>,
}

pub struct CacheFutureResultEntry {
    operation: CacheFutureResultOperation,
}

pub struct CacheFutureGetOrSetResultEntry {
    handle: Option<DurableCallSession<P3KeyvalueCacheGetOrSet, Cancellable>>,
    environment_id: EnvironmentId,
    key: Key,
    permit: Option<LiveAuthorizationPermit>,
    denied: Option<HostResponseKVGet>,
}

pub struct CacheVacancyEntry {
    environment_id: EnvironmentId,
    key: Key,
    filled: bool,
    permit: Option<LiveAuthorizationPermit>,
}

enum CacheFutureResultOperation {
    Set {
        handle: Option<DurableCallSession<P3KeyvalueCacheSet, Cancellable>>,
        environment_id: EnvironmentId,
        key: Key,
        value: Vec<u8>,
        denied: Option<HostResponseKVUnit>,
    },
    Delete {
        handle: Option<DurableCallSession<P3KeyvalueCacheDelete, Cancellable>>,
        environment_id: EnvironmentId,
        key: Key,
        denied: Option<HostResponseKVUnit>,
    },
}

fn missing_handle(function: &str) -> anyhow::Error {
    anyhow!("keyvalue::cache::{function} future result already consumed")
}

async fn authorize_cache_key<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    verb: KvVerb,
    key: &str,
) -> anyhow::Result<Option<String>> {
    let target = accessor
        .with(|mut access| kv_target(environment_owner(access.get()), verb, CACHE_BUCKET, key));
    Ok(match target {
        Ok(target) => {
            authorize_live_permissions_at_serialized_access(accessor, accessor.getter(), &[target])
                .await?
                .err()
                .map(denial)
        }
        Err(error) => Some(denial(error)),
    })
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
        let (mut handle, environment_id, key, denied, key_value_service) =
            accessor.with(|mut access| {
                let ctx = access.get();
                let entry = ctx.table().get_mut(&self_)?;
                Ok::<_, anyhow::Error>((
                    entry.handle.take().ok_or_else(|| missing_handle("get"))?,
                    entry.environment_id,
                    entry.key.clone(),
                    entry.denied.take(),
                    ctx.state.key_value_service.clone(),
                ))
            })?;

        let response = 'resp: {
            if handle.is_live() {
                if let Some(response) = denied {
                    break 'resp handle
                        .complete_access(accessor, accessor.getter(), response)
                        .await?;
                }
            } else {
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
        let (mut handle, environment_id, key, denied, key_value_service) =
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
                    entry.denied.take(),
                    ctx.state.key_value_service.clone(),
                ))
            })?;

        let response = 'resp: {
            if handle.is_live() {
                if let Some(response) = denied {
                    break 'resp handle
                        .complete_access(accessor, accessor.getter(), response)
                        .await?;
                }
            } else {
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
            Set(DurableCallSession<P3KeyvalueCacheSet, Cancellable>),
            Delete(DurableCallSession<P3KeyvalueCacheDelete, Cancellable>),
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
                handle: DurableCallSession<P3KeyvalueCacheSet, Cancellable>,
                environment_id: EnvironmentId,
                key: Key,
                value: Vec<u8>,
                denied: Option<HostResponseKVUnit>,
            },
            Delete {
                handle: DurableCallSession<P3KeyvalueCacheDelete, Cancellable>,
                environment_id: EnvironmentId,
                key: Key,
                denied: Option<HostResponseKVUnit>,
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
                    denied,
                } => Action::Set {
                    handle: handle.take().ok_or_else(|| missing_handle("set"))?,
                    environment_id: *environment_id,
                    key: key.clone(),
                    value: std::mem::take(value),
                    denied: denied.take(),
                },
                CacheFutureResultOperation::Delete {
                    handle,
                    environment_id,
                    key,
                    denied,
                } => Action::Delete {
                    handle: handle.take().ok_or_else(|| missing_handle("delete"))?,
                    environment_id: *environment_id,
                    key: key.clone(),
                    denied: denied.take(),
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
                denied,
            } => 'resp: {
                if handle.is_live() {
                    if let Some(response) = denied {
                        break 'resp handle
                            .complete_access(accessor, accessor.getter(), response)
                            .await?;
                    }
                } else {
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
                denied,
            } => 'resp: {
                if handle.is_live() {
                    if let Some(response) = denied {
                        break 'resp handle
                            .complete_access(accessor, accessor.getter(), response)
                            .await?;
                    }
                } else {
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
        let (mut handle, environment_id, key, permit, denied, key_value_service) =
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
                    entry.permit,
                    entry.denied.take(),
                    ctx.state.key_value_service.clone(),
                ))
            })?;

        let response = 'resp: {
            if handle.is_live() {
                if let Some(response) = denied {
                    break 'resp handle
                        .complete_access(accessor, accessor.getter(), response)
                        .await?;
                }
            } else {
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
                    permit,
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
        let (environment_id, key, permit, is_live) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::vacancy", "vacancy_fill");
            let entry = ctx.table().get(&self_)?;
            if entry.filled {
                return Err(anyhow!("keyvalue::cache vacancy is already filled"));
            }
            Ok::<_, anyhow::Error>((
                entry.environment_id,
                entry.key.clone(),
                entry.permit,
                ctx.state.is_live(),
            ))
        })?;
        if is_live {
            accessor.with(|mut access| {
                access.get().table().get_mut(&self_)?.filled = true;
                Ok::<_, anyhow::Error>(())
            })?;
        }
        let handle = DurableCallSession::<P3KeyvalueCacheVacancyFill, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestKVCacheKeyAndTtl {
                key: key.clone(),
                ttl_ms,
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;
        if !is_live {
            accessor.with(|mut access| {
                access.get().table().get_mut(&self_)?.filled = true;
                Ok::<_, anyhow::Error>(())
            })?;
        }

        accessor.with(|mut access| {
            Ok(access
                .get()
                .table()
                .push(OutgoingValueEntry::new_cache_fill(CacheFillState {
                    handle: Some(handle),
                    environment_id,
                    key,
                    _permit: permit,
                }))?)
        })
    }

    async fn drop(accessor: &Accessor<U, Self>, rep: Resource<Vacancy>) -> anyhow::Result<()> {
        let (filled, key) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache::vacancy", "drop");
            let entry = ctx.table().get(&rep)?;
            Ok::<_, anyhow::Error>((entry.filled, entry.key.clone()))
        })?;
        if !filled {
            let mut handle =
                DurableCallSession::<P3KeyvalueCacheVacancyDrop, Cancellable>::start_access(
                    accessor,
                    accessor.getter(),
                    HostRequestKVCacheKey { key },
                    DurableFunctionType::WriteRemote,
                )
                .await?;

            accessor.with(|mut access| {
                access.get().table().delete(rep)?;
                Ok::<_, anyhow::Error>(())
            })?;

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
        } else {
            accessor.with(|mut access| {
                access.get().table().delete(rep)?;
                Ok::<_, anyhow::Error>(())
            })?;
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
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let (environment_id, is_live) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "get");
            (ctx.owned_agent_id.environment_id(), ctx.state.is_live())
        });
        let denial = if is_live {
            authorize_cache_key(accessor, KvVerb::Read, &k).await?
        } else {
            None
        };
        let request = HostRequestKVCacheKey { key: k.clone() };
        let denied = denial
            .as_deref()
            .map(|denial| P3KeyvalueCacheGet::denied(&request, denial));
        let handle =
            DurableCallSession::<P3KeyvalueCacheGet, Cancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::ReadRemote,
                AccessClaimOptions {
                    denial,
                    ..Default::default()
                },
                request,
            )
            .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureGetResultEntry {
                handle: Some(handle),
                environment_id,
                key: k,
                denied,
            })?)
        })
    }

    async fn exists(
        accessor: &Accessor<U, Self>,
        k: Key,
    ) -> anyhow::Result<Resource<FutureExistsResult>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let (environment_id, is_live) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "exists");
            (ctx.owned_agent_id.environment_id(), ctx.state.is_live())
        });
        let denial = if is_live {
            authorize_cache_key(accessor, KvVerb::Read, &k).await?
        } else {
            None
        };
        let request = HostRequestKVCacheKey { key: k.clone() };
        let denied = denial
            .as_deref()
            .map(|denial| P3KeyvalueCacheExists::denied(&request, denial));
        let handle =
            DurableCallSession::<P3KeyvalueCacheExists, Cancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::ReadRemote,
                AccessClaimOptions {
                    denial,
                    ..Default::default()
                },
                request,
            )
            .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureExistsResultEntry {
                handle: Some(handle),
                environment_id,
                key: k,
                denied,
            })?)
        })
    }

    async fn set(
        accessor: &Accessor<U, Self>,
        k: Key,
        v: Resource<OutgoingValue>,
        ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<FutureResult>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let (environment_id, is_live) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "set");
            (ctx.owned_agent_id.environment_id(), ctx.state.is_live())
        });
        let denial = if is_live {
            authorize_cache_key(accessor, KvVerb::Write, &k).await?
        } else {
            None
        };
        // A denied call records only the length. A replayed call keeps the bytes, because its
        // `Start` may have no `End`, and the repair writes the value the guest passed.
        let (value, length) = accessor.with(|mut access| {
            let body = access.get().table().get(&v)?.body.read().unwrap();
            let length = body.len();
            let value = if denial.is_some() {
                Vec::new()
            } else {
                body.clone()
            };
            Ok::<_, anyhow::Error>((value, length))
        })?;
        let request = HostRequestKVCacheKeyValueAndTtl {
            key: k.clone(),
            length,
            ttl_ms,
        };
        let denied = denial
            .as_deref()
            .map(|denial| P3KeyvalueCacheSet::denied(&request, denial));
        let handle =
            DurableCallSession::<P3KeyvalueCacheSet, Cancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::WriteRemote,
                AccessClaimOptions {
                    denial,
                    ..Default::default()
                },
                request,
            )
            .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureResultEntry {
                operation: CacheFutureResultOperation::Set {
                    handle: Some(handle),
                    environment_id,
                    key: k,
                    value,
                    denied,
                },
            })?)
        })
    }

    async fn get_or_set(
        accessor: &Accessor<U, Self>,
        k: Key,
    ) -> anyhow::Result<Resource<FutureGetOrSetResult>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let (environment_id, is_live) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "get_or_set");
            (ctx.owned_agent_id.environment_id(), ctx.state.is_live())
        });
        let (permit, denial) = if is_live {
            let owner = accessor.with(|mut access| environment_owner(access.get()));
            let targets = [
                kv_target(owner.clone(), KvVerb::Read, CACHE_BUCKET, &k),
                kv_target(owner, KvVerb::Write, CACHE_BUCKET, &k),
            ]
            .into_iter()
            .collect::<Result<Vec<_>, _>>();
            match targets {
                Ok(targets) => match authorize_live_permissions_at_serialized_access(
                    accessor,
                    accessor.getter(),
                    &targets,
                )
                .await?
                {
                    Ok(permit) => (Some(permit), None),
                    Err(error) => (None, Some(denial(error))),
                },
                Err(error) => (None, Some(denial(error))),
            }
        } else {
            (None, None)
        };
        let request = HostRequestKVCacheKey { key: k.clone() };
        let denied = denial
            .as_deref()
            .map(|denial| P3KeyvalueCacheGetOrSet::denied(&request, denial));
        let handle =
            DurableCallSession::<P3KeyvalueCacheGetOrSet, Cancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::ReadRemote,
                AccessClaimOptions {
                    denial,
                    ..Default::default()
                },
                request,
            )
            .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureGetOrSetResultEntry {
                handle: Some(handle),
                environment_id,
                key: k,
                permit,
                denied,
            })?)
        })
    }

    async fn delete(
        accessor: &Accessor<U, Self>,
        k: Key,
    ) -> anyhow::Result<Resource<FutureResult>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let (environment_id, is_live) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::cache", "delete");
            (ctx.owned_agent_id.environment_id(), ctx.state.is_live())
        });
        let denial = if is_live {
            authorize_cache_key(accessor, KvVerb::Delete, &k).await?
        } else {
            None
        };
        let request = HostRequestKVCacheKey { key: k.clone() };
        let denied = denial
            .as_deref()
            .map(|denial| P3KeyvalueCacheDelete::denied(&request, denial));
        let handle =
            DurableCallSession::<P3KeyvalueCacheDelete, Cancellable>::start_access_with_options(
                accessor,
                accessor.getter(),
                DurableFunctionType::WriteRemote,
                AccessClaimOptions {
                    denial,
                    ..Default::default()
                },
                request,
            )
            .await?;
        accessor.with(|mut access| {
            Ok(access.get().table().push(CacheFutureResultEntry {
                operation: CacheFutureResultOperation::Delete {
                    handle: Some(handle),
                    environment_id,
                    key: k,
                    denied,
                },
            })?)
        })
    }
}
