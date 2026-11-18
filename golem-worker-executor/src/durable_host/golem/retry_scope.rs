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

//! Retry policy scopes: a named retry policy that applies to the task that opened it and to the
//! subtasks it creates while the scope is open.
//!
//! A scope lives in the task's context slot ([`GuestTaskContext`]), which a subtask copies when it
//! is created, and a store task captures with its attribution. Policy resolution overlays the
//! scopes of the task the running host code belongs to on the agent-wide policies
//! (`retry_policy_mutations`). Opening and dropping a scope records nothing: replay runs the guest
//! again and opens the same scopes, and every guest-visible read of the policies is a recorded
//! host call.

use crate::durable_host::durability::{GuestTaskContext, RetryPolicyScope};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem_api_1_x::retry::NamedRetryPolicy as WitNamedRetryPolicy;
use crate::preview2::golem_api_1_x::retry_scope::{
    Host, HostRetryPolicyScope, HostRetryPolicyScopeWithStore,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::retry_policy::NamedRetryPolicy;
use tracing::debug;
use wasmtime::AsContextMut;
use wasmtime::component::{Access, HasSelf, Resource};

/// The guest's handle to an open retry policy scope.
#[derive(Debug)]
pub struct RetryPolicyScopeEntry {
    scope_id: u64,
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> HostRetryPolicyScope for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostRetryPolicyScopeWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    fn new(
        mut access: Access<'_, U, Self>,
        policy: WitNamedRetryPolicy,
    ) -> anyhow::Result<Resource<RetryPolicyScopeEntry>> {
        DurabilityHost::observe_function_call(
            access.get(),
            "golem::api::retry_scope",
            "retry_policy_scope::new",
        );
        let policy: NamedRetryPolicy = policy.try_into().map_err(anyhow::Error::msg)?;
        let scope_id = {
            let state = &mut access.get().state;
            let scope_id = state.next_retry_policy_scope_id;
            state.next_retry_policy_scope_id += 1;
            scope_id
        };
        let mut retry_scopes = current_retry_scopes(&mut access)?;
        retry_scopes.push(RetryPolicyScope { scope_id, policy });
        install(&mut access, retry_scopes)?;
        Ok(access
            .get()
            .table()
            .push(RetryPolicyScopeEntry { scope_id })?)
    }

    fn drop(
        mut access: Access<'_, U, Self>,
        resource: Resource<RetryPolicyScopeEntry>,
    ) -> anyhow::Result<()> {
        let RetryPolicyScopeEntry { scope_id } = access.get().table().delete(resource)?;
        let mut retry_scopes = current_retry_scopes(&mut access)?;
        let Some(position) = retry_scopes
            .iter()
            .position(|scope| scope.scope_id == scope_id)
        else {
            debug!(
                scope_id,
                "A retry policy scope was dropped by a task that does not have it open"
            );
            return Ok(());
        };
        if position + 1 != retry_scopes.len() {
            debug!(
                scope_id,
                "A retry policy scope was dropped out of nesting order"
            );
        }
        retry_scopes.remove(position);
        install(&mut access, retry_scopes)
    }
}

/// Installs the calling task's retry policy scopes, and records them on the task's invocation for
/// the retry decision of a guest trap. A task whose invocation was replaced records nothing.
fn install<U: Send + 'static, Ctx: WorkerCtx>(
    access: &mut Access<'_, U, HasSelf<DurableWorkerCtx<Ctx>>>,
    retry_scopes: Vec<RetryPolicyScope>,
) -> anyhow::Result<()> {
    if let Some(invocation) = access.get().state.own_invocation_state_mut() {
        invocation.trap_retry_scopes.clone_from(&retry_scopes);
    }
    GuestTaskContext::install_retry_scopes(access.as_context_mut(), retry_scopes)
}

fn current_retry_scopes<U: Send + 'static, Ctx: WorkerCtx>(
    access: &mut Access<'_, U, HasSelf<DurableWorkerCtx<Ctx>>>,
) -> anyhow::Result<Vec<RetryPolicyScope>> {
    Ok(access
        .as_context_mut()
        .guest_task_context::<GuestTaskContext>()?
        .map(|context| context.retry_scopes.clone())
        .unwrap_or_default())
}
