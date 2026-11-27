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

//! The parts that every live remote step of an RPC call shares: what it sends on behalf of the
//! calling agent, and how it waits for the remote answer.

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{Cancellable, DurableCallSession};
use crate::durable_host::durability::DurabilityHost;
use crate::durable_host::with_import_access;
use crate::services::rpc::Rpc;
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::caller::Caller;
use golem_common::model::invocation_context::{InvocationContextStack, SpanId};
use golem_common::model::oplog::HostPayloadPair;
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::Future;
use std::sync::Arc;
use wasmtime::component::{Accessor, HasSelf};

/// What a live remote step sends on behalf of the calling agent.
pub(super) struct RemoteCall {
    pub rpc: Arc<dyn Rpc>,
    pub created_by: AccountId,
    pub agent_id: AgentId,
    /// The invocation context of the remote step: a child of `span_id`.
    pub stack: InvocationContextStack,
    pub caller: Caller,
}

impl RemoteCall {
    pub(super) fn of<Ctx: WorkerCtx>(ctx: &mut DurableWorkerCtx<Ctx>, span_id: &SpanId) -> Self {
        Self {
            rpc: ctx.rpc(),
            created_by: ctx.created_by(),
            agent_id: *ctx.agent_id(),
            stack: ctx.clone_as_inherited_stack(span_id),
            caller: ctx.outgoing_caller(),
        }
    }

    pub(super) fn capture<U: Send + 'static, Ctx: WorkerCtx>(
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        span_id: &SpanId,
    ) -> Self {
        with_import_access(accessor, |mut access| Self::of(access.get(), span_id))
    }
}

/// Waits for a short remote step of a live call, such as the target's acceptance. An interrupt
/// of the agent traps the call and leaves its `Start` incomplete for repair.
pub(super) async fn interruptible<U, Ctx, Pair, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut DurableCallSession<Pair, Cancellable>,
    step: impl Future<Output = T>,
) -> anyhow::Result<T>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
{
    let interrupt = with_import_access(accessor, |mut access| {
        access.get().create_interrupt_signal()
    });
    tokio::select! {
        biased;
        kind = interrupt => Err(handle.trap(kind)),
        result = step => Ok(result),
    }
}

/// Waits for a long remote step, such as an invocation's result. The wait is an RPC wait, so
/// the agent can suspend while it lasts; an interrupt or a suspension ends it with its kind.
pub(super) async fn suspendable<U, Ctx, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    step: impl Future<Output = T>,
) -> Result<T, InterruptKind>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let (rpc_wait, interrupt) = with_import_access(accessor, |mut access| {
        let ctx = access.get();
        let wait = accessor.runtime_activity().and_then(|activity| {
            ctx.runtime_suspension.as_ref().and_then(|runtime| {
                runtime.rpc_wait(
                    activity,
                    ctx.state.config.suspend.rpc_suspend_after,
                    ctx.state.config.suspend.rpc_resume_after,
                )
            })
        });
        (wait, ctx.create_interrupt_signal())
    });
    match rpc_wait {
        Some(wait) => wait.wait_result(step, interrupt).await,
        None => tokio::select! {
            biased;
            kind = interrupt => Err(kind),
            result = step => Ok(result),
        },
    }
}
