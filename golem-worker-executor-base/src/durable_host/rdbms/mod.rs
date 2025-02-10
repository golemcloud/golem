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

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::base_model::OplogIndex;

pub mod mysql;
pub mod postgres;
pub mod serialized;
pub mod types;

pub(crate) fn get_begin_oplog_index<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> anyhow::Result<OplogIndex> {
    let begin_oplog_idx = *ctx.state.open_function_table.get(&handle).ok_or_else(|| {
        anyhow!("No matching BeginRemoteWrite index was found for the open Rdbms request")
    })?;
    Ok(begin_oplog_idx)
}
