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

use super::WorkerInstance;
use crate::model::TrapType;
use crate::services::golem_config::GolemConfig;
use crate::services::resource_limits::MonthlyResourceExhaustion;
use crate::workerctx::WorkerCtx;
use golem_common::model::AgentStatus;
use golem_common::model::OplogIndex;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use uuid::Uuid;

#[cfg(test)]
mod tests;

/// Capture ownership before stopping consumes the Running metadata. An unchanged counter
/// alone does not identify a pre-owner attempt as the previous generation's owner.
pub(super) fn started_marker_generation(
    lifecycle: &WorkerInstance,
    expected_attempt: Uuid,
    generation: u64,
) -> Option<u64> {
    matches!(lifecycle.deletion_runtime(), WorkerInstance::Running(running)
        if running.owner_generation_started && running.start_attempt == expected_attempt)
    .then_some(generation)
}

/// FIFO-folded Suspended status already contains the marker for an established owner.
/// A pre-owner attempt cannot reuse a prior generation's status.
pub(super) fn unloaded_lifecycle_marker(
    kind: InterruptKind,
    status: AgentStatus,
    generation: u64,
    established_generation: Option<u64>,
) -> Option<OplogEntry> {
    match kind {
        InterruptKind::Suspend(_) => {
            if status == AgentStatus::Suspended && established_generation == Some(generation) {
                return None;
            }
            Some(OplogEntry::suspend())
        }
        InterruptKind::Interrupt(_) => Some(OplogEntry::interrupted()),
        InterruptKind::Restart | InterruptKind::Jump | InterruptKind::ShardLost => None,
    }
}

pub(super) fn monthly_failure<Ctx: WorkerCtx>(
    trap: &TrapType,
    mode: AgentMode,
    accepted: &Option<(InterruptKind, MonthlyResourceExhaustion)>,
    config: &GolemConfig,
) -> Option<TrapType> {
    match trap {
        TrapType::Interrupt(kind) => {
            monthly_interrupt_error(*kind, mode, accepted, config).map(|error| {
                TrapType::from_worker_executor_error::<Ctx>(
                    error,
                    OplogIndex::INITIAL,
                    false,
                    false,
                    mode,
                )
            })
        }
        _ => None,
    }
}

pub(super) fn monthly_interrupt_error(
    kind: InterruptKind,
    mode: AgentMode,
    accepted: &Option<(InterruptKind, MonthlyResourceExhaustion)>,
    config: &GolemConfig,
) -> Option<WorkerExecutorError> {
    if mode != AgentMode::Ephemeral {
        return None;
    }
    accepted.as_ref().and_then(|(selected, exhaustion)| {
        (*selected == kind).then(|| {
            super::invocation_loop::monthly_resource_exhausted_invocation_error(config, *exhaustion)
        })
    })
}
