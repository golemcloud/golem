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

//! The host calls that a never-live agent may execute.
//!
//! A never-live agent replays its history for a pending automatic update and stops. It never
//! goes live, so a durable call that has no recorded result ends the replay before it executes.
//! A call executes now only in a snapshot hook, which runs with durability suppressed: it records
//! nothing, and nothing is replayed for it. A load hook runs read-only, and a never-live agent
//! runs no save hook. There, a typed durable call is admitted only when its function type is
//! `ReadLocal`, and every other call is refused before the read-only check, so the refusal is
//! not replayable and not a read-only violation. A `Custom` function is refused, because the
//! guest supplies its name and function type. A call that returns its recorded result is always
//! admitted, so recorded replay does not change.

use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_service_base::error::worker_executor::WorkerExecutorError;

/// The refusal of the host call or native operation at `boundary` for a never-live owner: the
/// typed error of that call.
pub(crate) fn refusal(boundary: &str) -> WorkerExecutorError {
    tracing::warn!(boundary, "a never-live owner refused a peer effect");
    WorkerExecutorError::NeverLiveEffectRefused {
        boundary: boundary.to_string(),
    }
}

/// Whether an owner that is `marked` never-live admits a durable call of `host_function` with
/// `function_type`. A call that only returns its recorded result, `executes_now` false, is
/// admitted; a call that executes now is admitted only when it is `ReadLocal` and not `Custom`.
/// An unmarked owner admits every call.
pub(crate) fn admits(
    marked: bool,
    executes_now: bool,
    function_type: &DurableFunctionType,
    host_function: &HostFunctionName,
) -> bool {
    !marked
        || !executes_now
        || (matches!(function_type, DurableFunctionType::ReadLocal)
            && !matches!(host_function, HostFunctionName::Custom(_)))
}

#[cfg(test)]
mod tests;
