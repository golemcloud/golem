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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx, HttpRequestCloseOwner};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use golem_common::model::oplog::DurableFunctionType;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tracing::{debug, warn};

pub mod outgoing_http;
pub mod types;

pub(crate) async fn end_http_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    current_handle: u32,
) -> Result<(), WorkerExecutorError> {
    debug!(
        worker_id = %ctx.owned_agent_id.agent_id,
        handle = current_handle,
        is_live = ctx.state.is_live(),
        open_requests = ?ctx.state.open_http_requests.keys().collect::<Vec<_>>(),
        "end_http_request called"
    );
    if let Some(state) = ctx.state.open_http_requests.remove(&current_handle) {
        debug!(
            worker_id = %ctx.owned_agent_id.agent_id,
            handle = current_handle,
            begin_index = %state.begin_index,
            close_owner = ?state.close_owner,
            "end_http_request: ending durable function"
        );
        ctx.end_durable_function(
            &DurableFunctionType::WriteRemoteBatched(None),
            state.begin_index,
            false,
        )
        .await?;

        ctx.finish_span(&state.span_id).await?;
    } else {
        warn!(
            "No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}",
            current_handle, ctx.state.open_http_requests
        );
    }

    Ok(())
}

pub(crate) fn continue_http_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    current_handle: u32,
    new_handle: u32,
    new_close_owner: HttpRequestCloseOwner,
) {
    debug!(
        worker_id = %ctx.owned_agent_id.agent_id,
        current_handle,
        new_handle,
        new_close_owner = ?new_close_owner,
        is_live = ctx.state.is_live(),
        open_requests = ?ctx.state.open_http_requests.keys().collect::<Vec<_>>(),
        "continue_http_request called"
    );
    if let Some(mut state) = ctx.state.open_http_requests.remove(&current_handle) {
        let old_owner = state.close_owner.clone();
        state.close_owner = new_close_owner.clone();
        debug!(
            worker_id = %ctx.owned_agent_id.agent_id,
            current_handle,
            new_handle,
            old_owner = ?old_owner,
            new_owner = ?new_close_owner,
            begin_index = %state.begin_index,
            "continue_http_request: transferred ownership"
        );
        ctx.state.open_http_requests.insert(new_handle, state);
    } else {
        warn!(
            "No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}",
            current_handle, ctx.state.open_http_requests
        );
    }
}
