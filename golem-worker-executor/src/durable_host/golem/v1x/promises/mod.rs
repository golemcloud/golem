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

use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::drain_dropped_call_events_access;
use crate::durable_host::concurrent::reserve_invocation_id_before_await;
use crate::durable_host::suspendable_wait::ParkOutcome;
use crate::durable_host::suspendable_wait::ephemeral_sleep_too_long_error;
use crate::durable_host::suspendable_wait::wait_for_ready;
use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::host::HostGetPromiseResult;
use crate::preview2::golem_api_1_x::host::HostGetPromiseResultWithStore;
use crate::preview2::golem_api_1_x::host::HostWithStore;
use crate::services::promise::PromiseHandle;
use crate::services::promise::PromiseService;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::PromiseId;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestNoInput;
use golem_common::model::oplog::HostResponseGolemApiPromiseId;
use golem_common::model::oplog::HostResponseGolemApiPromiseResult;
use golem_common::model::oplog::host_functions::GolemApiCreatePromise;
use golem_common::model::oplog::host_functions::GolemApiGetPromiseResult;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use tokio::sync::OnceCell;
use wasmtime::component::Accessor;
use wasmtime::component::HasSelf;
use wasmtime::component::Resource;

impl<U: Send + 'static, Ctx: WorkerCtx> HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn create_promise(
        accessor: &Accessor<U, Self>,
    ) -> anyhow::Result<golem_api_1_x::host::PromiseId> {
        // A synchronous guest import must still let concurrent host admissions release their
        // authority boundary; holding an exclusive Store borrow here would deadlock them.
        let mut handle = DurableCallSession::<GolemApiCreatePromise, NotCancellable>::start_access_with(
            accessor,
            accessor.getter(),
            DurableFunctionType::WriteLocal,
            |context, accessor| {
                Box::pin(async move {
                    if context.is_live {
                        crate::durable_host::call_coordinator::synchronize_live_agent_authority_access(
                            accessor,
                            accessor.getter(),
                        )
                        .await?;
                    }
                    Ok(HostRequestNoInput {})
                })
            },
        )
        .await?;

        if !handle.is_live() {
            match handle.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(response) => return Ok(response.promise_id.into()),
                CallReplayOutcome::Incomplete(live) => {
                    handle = live;
                    crate::durable_host::call_coordinator::synchronize_live_agent_authority_access(
                        accessor,
                        accessor.getter(),
                    )
                    .await?;
                }
            }
        }
        let (promise_service, agent_id) = accessor.with(|mut access| {
            let ctx = access.get();
            (
                ctx.public_state.promise_service.clone(),
                ctx.agent_id().clone(),
            )
        });
        // The Start index remains the promise identity across incomplete replay.
        let promise_id = match promise_service
            .create(&agent_id, handle.start_index())
            .await
        {
            Ok(promise_id) => promise_id,
            Err(error) => return Err(handle.trap(error)),
        };
        let result = handle
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseGolemApiPromiseId { promise_id },
            )
            .await?;
        Ok(result.promise_id.into())
    }
}

impl<Ctx: WorkerCtx> HostGetPromiseResult for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, resource: Resource<GetPromiseResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::promise-result", "drop");
        let _ = self.table().delete(resource)?;

        self.state.promise_service.cleanup().await;

        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostGetPromiseResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        resource: Resource<GetPromiseResultEntry>,
    ) -> anyhow::Result<Vec<u8>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let resource_rep = resource.rep();
        let (entry, self_agent_id) = accessor.with(|mut access| {
            let ctx = access.get();
            let entry = ctx
                .table()
                .get(&Resource::<GetPromiseResultEntry>::new_borrow(resource_rep))?
                .clone();
            Ok::<_, anyhow::Error>((entry, ctx.agent_id().clone()))
        })?;

        drain_dropped_call_events_access(accessor, accessor.getter())
            .await
            .map_err(anyhow::Error::from)?;

        let mut handle = DurableCallSession::<GolemApiGetPromiseResult, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await
        .map_err(anyhow::Error::from)?;

        if !handle.is_live() {
            match handle
                .replay_access(accessor, accessor.getter())
                .await
                .map_err(anyhow::Error::from)?
            {
                CallReplayOutcome::Replayed(response) => {
                    return response.result.ok_or_else(|| {
                        anyhow::anyhow!("Promise result was not completed in the oplog")
                    });
                }
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
        }

        // Only the agent that originally created the promise is woken up when it is completed.
        if entry.promise_id.agent_id != self_agent_id {
            return Err(handle.trap(anyhow!(
                "Tried awaiting a promise not created by the current agent"
            )));
        }

        let promise_handle = match entry.get_handle().await {
            Ok(handle) => handle,
            Err(err) => {
                return Err(handle.trap(err.clone()));
            }
        };
        let (coordinated_wait, mode, max_sleep, interrupt) = accessor
            .with(|mut access| {
                let ctx = access.get();
                let interrupt = ctx.create_interrupt_signal();
                let coordinated_wait = if ctx.agent_mode() == AgentMode::Durable
                    && !ctx.state.durability_is_suppressed()
                {
                    Some(
                        accessor
                            .runtime_activity()
                            .and_then(|activity| {
                                ctx.runtime_suspension
                                    .as_ref()
                                    .and_then(|runtime| runtime.promise(activity))
                            })
                            .ok_or_else(|| anyhow!("promise runtime activity is not registered"))?,
                    )
                } else {
                    None
                };
                Ok::<_, anyhow::Error>((
                    coordinated_wait,
                    ctx.agent_mode(),
                    ctx.state.config.suspend.ephemeral_max_sleep,
                    interrupt,
                ))
            })
            .map_err(|err| handle.trap(err))?;
        let outcome = if let Some(wait) = coordinated_wait {
            match wait
                .wait(
                    {
                        let promise_handle = promise_handle.clone();
                        async move { promise_handle.await_ready().await }
                    },
                    interrupt,
                )
                .await
            {
                Ok(()) => ParkOutcome::Ready,
                Err(kind) => ParkOutcome::Interrupted(kind),
            }
        } else {
            wait_for_ready(
                mode,
                max_sleep,
                None,
                interrupt,
                promise_handle.await_ready(),
            )
            .await
        };

        match outcome {
            ParkOutcome::Ready => {}
            ParkOutcome::Interrupted(kind) => {
                // An interrupt is non-error control flow: abandon the durable call without a
                // trap context so the error classifies as `TrapType::Interrupt`.
                handle.abandon_for_trap();
                return Err(anyhow::Error::from(kind));
            }
            ParkOutcome::EphemeralTooLong {
                requested_nanos,
                max_nanos,
            } => {
                return Err(handle.trap(ephemeral_sleep_too_long_error(requested_nanos, max_nanos)));
            }
        }

        let result = match promise_handle.get().await {
            Some(result) => result,
            None => {
                return Err(handle.trap(WorkerExecutorError::runtime(format!(
                    "Promise {} was ready without a completion payload",
                    entry.promise_id
                ))));
            }
        };

        let response = handle
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseGolemApiPromiseResult {
                    result: Some(result),
                },
            )
            .await
            .map_err(anyhow::Error::from)?;

        response.result.ok_or_else(|| {
            anyhow::Error::from(WorkerExecutorError::runtime(
                "Promise result was not completed after durable persistence",
            ))
        })
    }
}

#[derive(Clone)]
pub struct GetPromiseResultEntry {
    promise_id: PromiseId,
    promise_service: Arc<dyn PromiseService>,
    handle: Arc<OnceCell<PromiseHandle>>,
}

impl GetPromiseResultEntry {
    pub fn new(promise_id: PromiseId, promise_service: Arc<dyn PromiseService>) -> Self {
        Self {
            promise_id,
            promise_service,
            handle: Arc::new(OnceCell::new()),
        }
    }

    /// Resolves the backing promise handle, memoizing only success.
    ///
    /// A failed `poll` is deliberately not cached. Polling reads the key-value storage, so a
    /// transient storage failure would otherwise poison this pollable for the rest of the
    /// resource's lifetime; every call retries until a handle is resolved.
    pub async fn get_handle(&self) -> Result<&PromiseHandle, WorkerExecutorError> {
        self.handle
            .get_or_try_init(|| self.promise_service.poll(self.promise_id.clone()))
            .await
    }

    /// Returns true if the underlying promise handle is ready, OR if constructing the
    /// handle failed (so that the pollable resolves immediately and the error is
    /// surfaced on the next `get` call, which retries the poll).
    pub async fn is_ready(&self) -> bool {
        match self.get_handle().await {
            Ok(handle) => handle.is_ready().await,
            Err(_) => true,
        }
    }
}

#[async_trait]
impl wasmtime_wasi::p2::Pollable for GetPromiseResultEntry {
    async fn ready(&mut self) {
        // A failed poll is treated as immediately ready so that the pollable
        // resolves and the subsequent `get` surfaces the error to the agent.
        if let Ok(handle) = self.get_handle().await {
            handle.await_ready().await;
        }
    }
}

#[cfg(test)]
mod tests;
