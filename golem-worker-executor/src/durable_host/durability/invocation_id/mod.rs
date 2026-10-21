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

use crate::durable_host::DurableWorkerCtx;
use crate::worker::suspension::GuestImportInvocationId;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{DurableInvocationId, OplogIndex};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
use wasmtime::component::RuntimeActivityId;

/// Takes the next ordinal under `anchor` and gives the invocation id of a guest call.
///
/// The ordinals under one anchor are taken in guest call order, which is the same in the live run
/// and in every replay, so a call gets the same id each time.
fn next_invocation_id(
    ordinals: &mut HashMap<OplogIndex, u64>,
    anchor: OplogIndex,
) -> Result<DurableInvocationId, WorkerExecutorError> {
    let next = ordinals.entry(anchor).or_default();
    let ordinal = *next;
    *next = ordinal
        .checked_add(1)
        .ok_or_else(|| WorkerExecutorError::runtime("durable invocation ordinal overflow"))?;
    Ok(DurableInvocationId::new(anchor, ordinal))
}

/// Reserves the invocation id of a guest call under `anchor`: the `Start` of the parent custom
/// invocation, or [`DurableWorkerCtx::invocation_anchor`] for a call without one. Every `Start`
/// the call records carries the id, and replay claims those `Start`s by it.
///
/// `activity` is the concurrent guest import that makes the call. All durable calls of one import
/// share the id it reserved first. That first reservation must happen before the guest makes
/// another import call: concurrent imports reach their durable calls in an order that depends on
/// host scheduling, and only the guest's call order is the same in every run.
pub(crate) fn reserve_invocation_id<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    activity: Option<RuntimeActivityId>,
    anchor: OplogIndex,
) -> Result<DurableInvocationId, WorkerExecutorError> {
    let runtime = activity.zip(ctx.runtime_suspension.clone());
    if let Some((activity, runtime)) = &runtime {
        match runtime.guest_import_invocation_id(*activity) {
            GuestImportInvocationId::Reserved(invocation_id) => {
                return Ok(invocation_id);
            }
            GuestImportInvocationId::Reservable => {}
            GuestImportInvocationId::Late => {
                return Err(WorkerExecutorError::runtime(
                    "a guest import reserved its durable invocation id after the guest made another import call",
                ));
            }
        }
    }
    let invocation_id = next_invocation_id(&mut ctx.state.invocation_ordinals, anchor)?;
    if let Some((activity, runtime)) = runtime {
        runtime.record_guest_import_invocation_id(activity, invocation_id);
    }
    Ok(invocation_id)
}

#[cfg(test)]
mod tests;
