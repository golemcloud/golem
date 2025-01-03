// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::durable_host::{DurableWorkerCtx, HttpRequestCloseOwner};
use crate::error::GolemError;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::WrappedFunctionType;
use tracing::warn;

pub mod outgoing_http;

/// Serializable response data structures to be stored in the oplog
pub mod serialized;

pub mod types;

pub(crate) async fn end_http_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    current_handle: u32,
) -> Result<(), GolemError> {
    if let Some(state) = ctx.state.open_http_requests.remove(&current_handle) {
        match ctx.state.open_function_table.get(&state.root_handle) {
            Some(begin_index) => {
                ctx.state
                    .end_function(&WrappedFunctionType::WriteRemoteBatched(None), *begin_index)
                    .await?;
                ctx.state.open_function_table.remove(&state.root_handle);
            }
            None => {
                warn!("No matching BeginRemoteWrite index was found when HTTP response arrived. Handle: {}; open functions: {:?}", state.root_handle, ctx.state.open_function_table);
            }
        }
    } else {
        warn!("No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}", current_handle, ctx.state.open_http_requests);
    }

    Ok(())
}

pub(crate) fn continue_http_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    current_handle: u32,
    new_handle: u32,
    new_close_owner: HttpRequestCloseOwner,
) {
    if let Some(mut state) = ctx.state.open_http_requests.remove(&current_handle) {
        state.close_owner = new_close_owner;
        ctx.state.open_http_requests.insert(new_handle, state);
    } else {
        warn!("No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}", current_handle, ctx.state.open_http_requests);
    }
}
