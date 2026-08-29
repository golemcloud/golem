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

use super::Worker;
use crate::services::{HasAll, HasOplogService, HasWorkerService};
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::{AgentMode, ParsedAgentId, Principal};
use golem_common::model::component::{ComponentRevision, PluginPriority};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex, UpdateDescription};
use golem_common::model::worker::{ResolvedRevert, RevertWorkerTarget};
use golem_common::model::{AgentMetadata, AgentStatus, OwnedAgentId, PendingUpdateKind, Timestamp};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use tracing::{debug, info, warn};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateMode {
    Automatic,
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptDecision {
    Ignore,
    Interrupt,
    Restart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResumeDecision {
    Start,
    ForceStart,
    Reject,
    PreviousFailed,
    PreviousExited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateDecision {
    Ignore,
    Queue,
    QueueAndStart,
    QueueAndRestart,
}

fn interrupt_decision(status: &AgentStatus, recover_immediately: bool) -> InterruptDecision {
    match status {
        AgentStatus::Exited
        | AgentStatus::Idle
        | AgentStatus::Failed
        | AgentStatus::Interrupted => InterruptDecision::Ignore,
        AgentStatus::Suspended | AgentStatus::Retrying => InterruptDecision::Interrupt,
        AgentStatus::Running if recover_immediately => InterruptDecision::Restart,
        AgentStatus::Running => InterruptDecision::Interrupt,
    }
}

fn resume_decision(status: &AgentStatus, force: bool) -> ResumeDecision {
    match status {
        AgentStatus::Failed => ResumeDecision::PreviousFailed,
        AgentStatus::Exited => ResumeDecision::PreviousExited,
        AgentStatus::Suspended | AgentStatus::Interrupted | AgentStatus::Idle => {
            ResumeDecision::Start
        }
        _ if force => ResumeDecision::ForceStart,
        _ => ResumeDecision::Reject,
    }
}

fn update_decision(status: &AgentStatus, mode: UpdateMode, disable_wakeup: bool) -> UpdateDecision {
    match mode {
        UpdateMode::Automatic => match status {
            AgentStatus::Exited => UpdateDecision::Ignore,
            AgentStatus::Interrupted
            | AgentStatus::Suspended
            | AgentStatus::Retrying
            | AgentStatus::Failed => {
                if disable_wakeup {
                    UpdateDecision::Queue
                } else {
                    UpdateDecision::QueueAndStart
                }
            }
            AgentStatus::Running | AgentStatus::Idle => UpdateDecision::QueueAndRestart,
        },
        UpdateMode::Manual => {
            if disable_wakeup {
                UpdateDecision::Queue
            } else {
                UpdateDecision::QueueAndStart
            }
        }
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    async fn existing_metadata<T: HasAll<Ctx>>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<AgentMetadata, WorkerExecutorError> {
        Self::get_latest_metadata(deps, owned_agent_id)
            .await
            .ok_or_else(|| WorkerExecutorError::worker_not_found(owned_agent_id.agent_id()))
    }

    async fn get_existing_suspended<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        component_revision: Option<ComponentRevision>,
        principal: Principal,
    ) -> Result<std::sync::Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        Self::get_or_create_suspended(
            deps,
            owned_agent_id,
            None,
            Vec::new(),
            component_revision,
            None,
            &InvocationContextStack::fresh(),
            principal,
        )
        .await
    }

    pub async fn delete<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        principal: Principal,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        Self::existing_metadata(deps, owned_agent_id).await?;
        let worker = Self::get_existing_suspended(deps, owned_agent_id, None, principal).await?;

        info!("Interrupting worker before deletion");
        worker
            .set_interrupting(InterruptKind::Interrupt(Timestamp::now_utc()))
            .await;
        info!("Marking worker for deletion");
        worker.start_deleting_internal().await?;

        worker.worker_service().remove(owned_agent_id).await;
        worker.remove_from_active_agents().await;

        // Keep the worker alive until durable metadata and cache cleanup has completed.
        drop(worker);
        Ok(())
    }

    pub async fn interrupt<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        recover_immediately: bool,
        principal: Principal,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let Some(metadata) = Self::get_latest_metadata(deps, owned_agent_id).await else {
            return Ok(());
        };

        let decision = interrupt_decision(&metadata.last_known_status.status, recover_immediately);
        if decision == InterruptDecision::Ignore {
            match metadata.last_known_status.status {
                AgentStatus::Exited => warn!("Attempted interrupting worker which already exited"),
                AgentStatus::Idle => warn!("Attempted interrupting worker which is idle"),
                AgentStatus::Failed => warn!("Attempted interrupting worker which is failed"),
                AgentStatus::Interrupted => {
                    warn!("Attempted interrupting worker which is already interrupted")
                }
                _ => unreachable!(),
            }
            return Ok(());
        }

        match metadata.last_known_status.status {
            AgentStatus::Suspended => debug!("Marking suspended worker as interrupted"),
            AgentStatus::Retrying => {
                debug!("Marking worker scheduled to be retried as interrupted")
            }
            _ => {}
        }

        let worker = Self::get_existing_suspended(deps, owned_agent_id, None, principal).await?;
        let interrupt_kind = match decision {
            InterruptDecision::Interrupt => InterruptKind::Interrupt(Timestamp::now_utc()),
            InterruptDecision::Restart => InterruptKind::Restart,
            InterruptDecision::Ignore => unreachable!(),
        };
        if let Some(mut await_interruption) = worker.set_interrupting(interrupt_kind).await {
            await_interruption.recv().await.unwrap();
        }

        if decision == InterruptDecision::Interrupt {
            // Dropping the resident worker also closes live connections associated with it.
            worker.remove_from_active_agents().await;
        }
        Ok(())
    }

    pub async fn resume<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        force: bool,
        principal: Principal,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let metadata = Self::existing_metadata(deps, owned_agent_id).await?;

        match resume_decision(&metadata.last_known_status.status, force) {
            ResumeDecision::PreviousFailed => {
                let error_and_retry_count = Ctx::get_last_error_and_retry_count(
                    deps,
                    owned_agent_id,
                    metadata.agent_mode,
                    &metadata.last_known_status,
                )
                .await;
                if let Some(last_error) = error_and_retry_count {
                    return Err(WorkerExecutorError::PreviousInvocationFailed {
                        error: last_error.error,
                        stderr: last_error.stderr,
                    });
                }
                Err(WorkerExecutorError::runtime(
                    "Previous invocation failed, but failed to get error details",
                ))
            }
            ResumeDecision::PreviousExited => Err(WorkerExecutorError::PreviousInvocationExited),
            decision @ (ResumeDecision::Start | ResumeDecision::ForceStart) => {
                match decision {
                    ResumeDecision::Start => info!(
                        "Activating {:?} worker {owned_agent_id} due to explicit resume request",
                        metadata.last_known_status.status
                    ),
                    ResumeDecision::ForceStart => info!(
                        "Force activating {:?} worker {owned_agent_id} due to explicit resume request",
                        metadata.last_known_status.status
                    ),
                    _ => unreachable!(),
                }
                Self::get_or_create_running(
                    deps,
                    owned_agent_id,
                    None,
                    Vec::new(),
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                    principal,
                )
                .await?;
                Ok(())
            }
            ResumeDecision::Reject => Err(WorkerExecutorError::invalid_request(format!(
                "Worker {agent_id} is not suspended, interrupted or idle",
                agent_id = owned_agent_id.agent_id
            ))),
        }
    }

    pub async fn update<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        mode: UpdateMode,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
        principal: Principal,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let metadata = Self::existing_metadata(deps, owned_agent_id).await?;

        if metadata.last_known_status.component_revision == target_revision {
            return Err(WorkerExecutorError::invalid_request(
                "Worker is already at the target version",
            ));
        }

        let component_metadata = deps
            .component_service()
            .get_metadata(
                owned_agent_id.agent_id.component_id,
                Some(metadata.last_known_status.component_revision),
            )
            .await?;

        if let Ok(agent_id) = ParsedAgentId::parse(
            &owned_agent_id.agent_id.agent_id,
            &component_metadata.metadata,
        ) && let Some(agent_type) = component_metadata
            .metadata
            .find_agent_type_by_name_ref(&agent_id.agent_type)
            && agent_type.mode == AgentMode::Ephemeral
        {
            return Err(WorkerExecutorError::invalid_request(
                "Ephemeral workers cannot be updated",
            ));
        }

        // A worker's durable agent mode selects its oplog namespace and cannot change across
        // component revisions. Unknown revisions are still queued so the update loop records the
        // canonical FailedUpdate entry.
        if let Ok(target_component_metadata) = deps
            .component_service()
            .get_metadata(owned_agent_id.agent_id.component_id, Some(target_revision))
            .await
            && let Ok(agent_id) = ParsedAgentId::parse(
                &owned_agent_id.agent_id.agent_id,
                &target_component_metadata.metadata,
            )
            && let Some(target_agent_type) = target_component_metadata
                .metadata
                .find_agent_type_by_name_ref(&agent_id.agent_type)
        {
            let persisted_mode = metadata.agent_mode;
            if target_agent_type.mode != persisted_mode {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Cannot update worker {} from {:?} to component revision {}: the agent type \
                     '{}' has mode {:?} in the target revision but the worker was created with \
                     mode {:?}. Changing an agent type's mode across revisions is not supported.",
                    owned_agent_id,
                    persisted_mode,
                    target_revision,
                    agent_id.agent_type,
                    target_agent_type.mode,
                    persisted_mode,
                )));
            }
        }

        match mode {
            UpdateMode::Automatic => {
                if metadata
                    .last_known_status
                    .pending_updates
                    .iter()
                    .any(|update| {
                        update.kind == PendingUpdateKind::Automatic
                            && update.target_revision == target_revision
                    })
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "The same update is already in progress",
                    ));
                }
            }
            UpdateMode::Manual => {
                if metadata
                    .last_known_status
                    .pending_invocations
                    .iter()
                    .any(|invocation| {
                        invocation.manual_update_target_revision == Some(target_revision)
                    })
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "The same update is already in progress",
                    ));
                }
            }
        }

        let decision = update_decision(&metadata.last_known_status.status, mode, disable_wakeup);
        match (mode, decision) {
            (UpdateMode::Automatic, UpdateDecision::Ignore) => {
                warn!("Attempted updating worker which already exited");
            }
            (UpdateMode::Automatic, decision) => {
                let current_revision = metadata.last_known_status.component_revision;
                let component_revision = match decision {
                    UpdateDecision::Queue | UpdateDecision::QueueAndStart => Some(current_revision),
                    UpdateDecision::QueueAndRestart => None,
                    UpdateDecision::Ignore => unreachable!(),
                };
                let worker = Self::get_existing_suspended(
                    deps,
                    owned_agent_id,
                    component_revision,
                    principal,
                )
                .await?;

                debug!("Enqueuing update");
                worker
                    .enqueue_update(UpdateDescription::Automatic { target_revision })
                    .await;

                match decision {
                    UpdateDecision::Queue => {
                        debug!("Skipping worker activation due to disable_wakeup flag")
                    }
                    UpdateDecision::QueueAndStart => {
                        debug!("Resuming initialization to perform the update");
                        Self::start_if_needed(worker).await?;
                    }
                    UpdateDecision::QueueAndRestart => {
                        debug!("Enqueued update for running worker");
                        worker.set_interrupting(InterruptKind::Restart).await;
                        debug!("Interrupted running worker for update");
                    }
                    UpdateDecision::Ignore => unreachable!(),
                }
            }
            (UpdateMode::Manual, decision) => {
                let worker =
                    Self::get_existing_suspended(deps, owned_agent_id, None, principal).await?;
                worker.enqueue_manual_update(target_revision).await?;

                match decision {
                    UpdateDecision::Queue => {
                        debug!("Skipping worker activation due to disable_wakeup flag")
                    }
                    UpdateDecision::QueueAndStart => {
                        Self::start_if_needed(worker).await?;
                    }
                    UpdateDecision::Ignore | UpdateDecision::QueueAndRestart => unreachable!(),
                }
            }
        }

        Ok(())
    }

    pub async fn revert<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        target: RevertWorkerTarget,
        resolved_revert: Option<ResolvedRevert>,
        principal: Principal,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        Self::existing_metadata(deps, owned_agent_id).await?;
        let worker = Self::get_existing_suspended(deps, owned_agent_id, None, principal).await?;
        worker.revert_internal(target, resolved_revert).await
    }

    pub async fn resolve_revert_last_invocations<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        number_of_invocations: u64,
    ) -> Result<ResolvedRevert, WorkerExecutorError>
    where
        T: HasWorkerService + HasOplogService,
    {
        let agent_mode = deps
            .worker_service()
            .get_agent_mode(owned_agent_id)
            .await
            .ok_or_else(|| WorkerExecutorError::worker_not_found(owned_agent_id.agent_id()))?;

        let oplog_service = deps.oplog_service();
        let observed_oplog_index = oplog_service
            .get_last_index(owned_agent_id, agent_mode)
            .await;
        let mut current = observed_oplog_index;
        let mut found = 0;

        loop {
            let entries = oplog_service
                .read_exact(owned_agent_id, agent_mode, current, 1)
                .await;
            let entry = entries.get(&current).ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Could not read oplog entry {current} while resolving revert"
                ))
            })?;

            if matches!(entry, OplogEntry::AgentInvocationStarted { .. }) {
                found += 1;
                if found == number_of_invocations {
                    return Ok(ResolvedRevert {
                        last_oplog_index: current.previous(),
                        observed_oplog_index,
                    });
                }
            }

            if current == OplogIndex::INITIAL {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Could not find {number_of_invocations} invocations to revert"
                )));
            }
            current = current.previous();
        }
    }

    pub async fn activate_plugin<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        plugin_priority: PluginPriority,
        principal: Principal,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        Self::set_plugin_activation(deps, owned_agent_id, plugin_priority, principal, true).await
    }

    pub async fn deactivate_plugin<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        plugin_priority: PluginPriority,
        principal: Principal,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        Self::set_plugin_activation(deps, owned_agent_id, plugin_priority, principal, false).await
    }

    async fn set_plugin_activation<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        plugin_priority: PluginPriority,
        principal: Principal,
        activate: bool,
    ) -> Result<(), WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let metadata = Self::existing_metadata(deps, owned_agent_id).await?;
        let component_metadata = deps
            .component_service()
            .get_metadata(
                owned_agent_id.agent_id.component_id,
                Some(metadata.last_known_status.component_revision),
            )
            .await?;
        let agent_type =
            ParsedAgentId::parse_agent_type_name(&owned_agent_id.agent_id.agent_id).ok();
        let grant_id = agent_type
            .as_ref()
            .and_then(|agent_type| component_metadata.metadata.agent_type_plugins(agent_type))
            .and_then(|plugins| {
                plugins
                    .iter()
                    .find(|installation| installation.priority == plugin_priority)
            })
            .map(|installation| installation.environment_plugin_grant_id)
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "Plugin installation does not belong to this worker's component",
                )
            })?;

        let is_active = metadata
            .last_known_status
            .active_plugins
            .contains(&grant_id);
        if activate == is_active {
            if activate {
                warn!("Plugin is already activated");
            } else {
                warn!("Plugin is already deactivated");
            }
            return Ok(());
        }

        let worker = Self::get_existing_suspended(deps, owned_agent_id, None, principal).await?;
        if activate {
            worker.activate_plugin_internal(grant_id).await
        } else {
            worker.deactivate_plugin_internal(grant_id).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    const STATUSES: [AgentStatus; 7] = [
        AgentStatus::Running,
        AgentStatus::Idle,
        AgentStatus::Suspended,
        AgentStatus::Interrupted,
        AgentStatus::Retrying,
        AgentStatus::Failed,
        AgentStatus::Exited,
    ];

    #[test]
    fn interrupt_policy_covers_every_status() {
        let expected = [
            InterruptDecision::Interrupt,
            InterruptDecision::Ignore,
            InterruptDecision::Interrupt,
            InterruptDecision::Ignore,
            InterruptDecision::Interrupt,
            InterruptDecision::Ignore,
            InterruptDecision::Ignore,
        ];
        let expected_recovering = [
            InterruptDecision::Restart,
            InterruptDecision::Ignore,
            InterruptDecision::Interrupt,
            InterruptDecision::Ignore,
            InterruptDecision::Interrupt,
            InterruptDecision::Ignore,
            InterruptDecision::Ignore,
        ];

        for ((status, expected), expected_recovering) in
            STATUSES.iter().zip(expected).zip(expected_recovering)
        {
            assert_eq!(interrupt_decision(status, false), expected);
            assert_eq!(interrupt_decision(status, true), expected_recovering);
        }
    }

    #[test]
    fn resume_policy_covers_every_status() {
        let expected = [
            ResumeDecision::Reject,
            ResumeDecision::Start,
            ResumeDecision::Start,
            ResumeDecision::Start,
            ResumeDecision::Reject,
            ResumeDecision::PreviousFailed,
            ResumeDecision::PreviousExited,
        ];
        let expected_forced = [
            ResumeDecision::ForceStart,
            ResumeDecision::Start,
            ResumeDecision::Start,
            ResumeDecision::Start,
            ResumeDecision::ForceStart,
            ResumeDecision::PreviousFailed,
            ResumeDecision::PreviousExited,
        ];

        for ((status, expected), expected_forced) in
            STATUSES.iter().zip(expected).zip(expected_forced)
        {
            assert_eq!(resume_decision(status, false), expected);
            assert_eq!(resume_decision(status, true), expected_forced);
        }
    }

    #[test]
    fn update_policy_covers_every_status_and_wakeup_mode() {
        let automatic = [
            UpdateDecision::QueueAndRestart,
            UpdateDecision::QueueAndRestart,
            UpdateDecision::QueueAndStart,
            UpdateDecision::QueueAndStart,
            UpdateDecision::QueueAndStart,
            UpdateDecision::QueueAndStart,
            UpdateDecision::Ignore,
        ];
        let automatic_without_wakeup = [
            UpdateDecision::QueueAndRestart,
            UpdateDecision::QueueAndRestart,
            UpdateDecision::Queue,
            UpdateDecision::Queue,
            UpdateDecision::Queue,
            UpdateDecision::Queue,
            UpdateDecision::Ignore,
        ];

        for ((status, expected), expected_without_wakeup) in
            STATUSES.iter().zip(automatic).zip(automatic_without_wakeup)
        {
            assert_eq!(
                update_decision(status, UpdateMode::Automatic, false),
                expected
            );
            assert_eq!(
                update_decision(status, UpdateMode::Automatic, true),
                expected_without_wakeup
            );
            assert_eq!(
                update_decision(status, UpdateMode::Manual, false),
                UpdateDecision::QueueAndStart
            );
            assert_eq!(
                update_decision(status, UpdateMode::Manual, true),
                UpdateDecision::Queue
            );
        }
    }
}
