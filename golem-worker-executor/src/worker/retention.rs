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

use super::*;
use golem_common::model::agent::InvocationEnd;
use golem_common::model::{ScheduleId, ScheduledAction};

pub(super) fn deadline(
    invocation_id: Option<InvocationId>,
    status: &AgentStatusRecord,
    lifetime: Duration,
) -> Option<jiff::Timestamp> {
    let key = invocation_id?;
    let gave_up = matches!(status.status, AgentStatus::Failed | AgentStatus::Exited);
    if !gave_up && (status.running.is_some() || status.settling_invocation.is_some()) {
        return None;
    }
    let finished_at = match status.invocation_results.position(&key) {
        Some(position) if position.end == InvocationEnd::Retrying && !gave_up => return None,
        Some(position) if position.end == InvocationEnd::Retrying => status.terminal_failure_at?,
        Some(position) => position.changed_at,
        None if gave_up => status.terminal_failure_at?,
        None => return None,
    };
    let deadline = i64::try_from(finished_at.to_millis())
        .ok()
        .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
        .and_then(|finished_at| {
            jiff::SignedDuration::try_from(lifetime)
                .ok()
                .and_then(|lifetime| finished_at.checked_add(lifetime).ok())
        });
    if deadline.is_none() {
        tracing::warn!(
            invocation_id = %key,
            finished_at_millis = finished_at.to_millis(),
            lifetime = ?lifetime,
            "Skipping retention for an unrepresentable invocation deadline"
        );
    }
    deadline
}

pub(super) async fn schedule<T: HasConfig + HasSchedulerService + HasShutdownToken>(
    deps: &T,
    owned_agent_id: &OwnedAgentId,
    fingerprint: AgentFingerprint,
    invocation_id: Option<InvocationId>,
    status: &AgentStatusRecord,
) {
    if let Some(at) = deadline(
        invocation_id,
        status,
        deps.config().standalone_tool_invocation_retention,
    ) {
        let scheduler = deps.scheduler_service();
        let shutdown = deps.shutdown_token();
        // The terminal is already committed. A later load repairs an interrupted registration.
        tokio::select! {
            _ = shutdown.cancelled() => {},
            _ = scheduler.schedule_with_id(
                ScheduleId::for_finished_invocation(owned_agent_id),
                at,
                ScheduledAction::DeleteFinishedInvocation {
                    owned_agent_id: owned_agent_id.clone(),
                    fingerprint,
                },
            ) => {},
        }
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(crate) fn standalone_invocation_id(&self) -> Option<InvocationId> {
        (self.initial_worker_metadata.owner_kind == OwnerKind::StandaloneTool)
            .then(|| {
                self.initial_worker_metadata
                    .standalone_tool
                    .as_ref()
                    .map(|tool| tool.invocation_id)
            })
            .flatten()
    }

    pub(crate) async fn finished_invocation_deadline(
        &self,
    ) -> Result<Option<jiff::Timestamp>, WorkerExecutorError> {
        let status = self.try_get_last_known_status().await?;
        Ok(deadline(
            self.standalone_invocation_id(),
            &status,
            self.config().standalone_tool_invocation_retention,
        ))
    }

    pub(super) async fn schedule_finished_invocation_retention(&self) {
        let status = self.get_last_known_status().await;
        schedule(
            &self.deps,
            &self.owned_agent_id,
            self.initial_worker_metadata.fingerprint,
            self.standalone_invocation_id(),
            &status,
        )
        .await;
    }
}
