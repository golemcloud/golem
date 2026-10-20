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

use super::{RetirementReason, Worker};
use crate::services::{HasAll, HasOplogService, HasWorkerService};
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::{AgentMode, ParsedAgentId, Principal};
use golem_common::model::agent::{ResolvedRevert, RevertWorkerTarget};
use golem_common::model::component::{ComponentRevision, PluginPriority};
use golem_common::model::oplog::{OplogEntry, OplogErrorKind, OplogIndex};
use golem_common::model::{AgentStatus, OwnedAgentId, PendingUpdateKind, Timestamp};
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
        AgentStatus::Running | AgentStatus::Suspended | AgentStatus::Retrying
            if recover_immediately =>
        {
            InterruptDecision::Restart
        }
        AgentStatus::Running | AgentStatus::Suspended | AgentStatus::Retrying => {
            InterruptDecision::Interrupt
        }
    }
}

fn resume_decision(
    status: &AgentStatus,
    last_error_kind: Option<OplogErrorKind>,
    force: bool,
) -> ResumeDecision {
    match status {
        AgentStatus::Failed if force && last_error_kind == Some(OplogErrorKind::Recovery) => {
            ResumeDecision::ForceStart
        }
        AgentStatus::Failed => ResumeDecision::PreviousFailed,
        AgentStatus::Exited => ResumeDecision::PreviousExited,
        AgentStatus::Suspended
        | AgentStatus::Interrupted
        | AgentStatus::Idle
        | AgentStatus::Retrying => ResumeDecision::Start,
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

pub(super) fn has_duplicate_pending_update(
    status: &golem_common::model::AgentStatusRecord,
    target_revision: ComponentRevision,
) -> bool {
    status.pending_updates.iter().any(|update| {
        matches!(
            update.kind,
            PendingUpdateKind::Automatic | PendingUpdateKind::SnapshotAssistedAutomatic(_)
        ) && update.target_revision == target_revision
    })
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(super) async fn get_existing_suspended<T>(
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        principal: Principal,
    ) -> Result<std::sync::Arc<Self>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        deps.active_agents()
            .get_existing(deps, owned_agent_id, principal)
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
        let mut worker =
            Self::get_existing_suspended(deps, owned_agent_id, principal.clone()).await?;
        let fingerprint = worker.get_initial_worker_metadata().fingerprint;
        loop {
            if let Some(outcome) = worker.start_deletion().await? {
                return outcome.handle().wait().await;
            }
            worker = match Self::get_existing_suspended(deps, owned_agent_id, principal.clone())
                .await
            {
                Ok(worker) if worker.get_initial_worker_metadata().fingerprint == fingerprint => {
                    worker
                }
                Ok(_) | Err(WorkerExecutorError::AgentNotFound { .. }) => return Ok(()),
                Err(error) => return Err(error),
            };
        }
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
        let worker = match Self::get_existing_suspended(deps, owned_agent_id, principal).await {
            Ok(worker) => worker,
            Err(WorkerExecutorError::AgentNotFound { .. }) => return Ok(()),
            Err(error) => return Err(error),
        };
        let metadata = worker.get_latest_worker_metadata().await;

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

        if decision == InterruptDecision::Interrupt {
            match metadata.last_known_status.status {
                AgentStatus::Suspended => debug!("Marking suspended worker as interrupted"),
                AgentStatus::Retrying => {
                    debug!("Marking worker scheduled to be retried as interrupted")
                }
                _ => {}
            }
        }

        let interrupt_kind = match decision {
            InterruptDecision::Interrupt => InterruptKind::Interrupt(Timestamp::now_utc()),
            InterruptDecision::Restart => InterruptKind::Restart,
            InterruptDecision::Ignore => unreachable!(),
        };
        if decision == InterruptDecision::Interrupt {
            worker
                .interrupt_and_retire(interrupt_kind, RetirementReason::Requested)
                .await?;
        } else if let Some(mut await_interruption) = worker.set_interrupting(interrupt_kind).await?
        {
            await_interruption.recv().await.unwrap();
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
        let worker = Self::get_existing_suspended(deps, owned_agent_id, principal).await?;
        worker.ensure_component_agent_ingress("resume")?;
        let metadata = worker.get_latest_worker_metadata().await;

        match resume_decision(
            &metadata.last_known_status.status,
            metadata.last_known_status.last_error_kind,
            force,
        ) {
            ResumeDecision::PreviousFailed => {
                let error_and_retry_count = Ctx::get_last_error_and_retry_count(
                    deps,
                    owned_agent_id,
                    metadata.agent_mode,
                    &metadata.last_known_status,
                )
                .await;
                if let Some(last_error) = error_and_retry_count {
                    return if metadata.last_known_status.last_error_kind
                        == Some(golem_common::model::oplog::OplogErrorKind::Recovery)
                    {
                        Err(WorkerExecutorError::failed_to_resume_worker(
                            owned_agent_id.agent_id.clone(),
                            WorkerExecutorError::runtime(
                                last_error.error.to_string(&last_error.stderr),
                            ),
                        ))
                    } else {
                        Err(WorkerExecutorError::PreviousInvocationFailed {
                            error: last_error.error,
                            stderr: last_error.stderr,
                        })
                    };
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
                Self::start_if_needed(worker).await?;
                Ok(())
            }
            ResumeDecision::Reject => Err(WorkerExecutorError::invalid_request(format!(
                "Worker {agent_id} is not suspended, interrupted, idle, or retrying",
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
    ) -> Result<OplogIndex, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker = Self::get_existing_suspended(deps, owned_agent_id, principal).await?;
        worker.ensure_component_agent_ingress("update")?;
        let metadata = worker.get_latest_worker_metadata().await;
        if worker.deletion_owns_retirement().await {
            return Err(WorkerExecutorError::invalid_request(
                "Worker is being deleted",
            ));
        }

        if metadata.last_known_status.component_revision == target_revision {
            return Err(WorkerExecutorError::invalid_request(
                "Worker is already at the target version",
            ));
        }

        if metadata.agent_mode == AgentMode::Ephemeral {
            return Err(WorkerExecutorError::invalid_request(
                "Ephemeral workers cannot be updated",
            ));
        }

        // A worker's durable agent mode selects its oplog namespace and cannot change across
        // component revisions. Unknown revisions are still queued so the update loop records the
        // canonical FailedUpdate entry. Automatic updates defer every target lookup until their
        // replay strategy has been durably selected at the queue head and validated.
        if mode != UpdateMode::Automatic
            && let Ok(target_component_metadata) = deps
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
                if has_duplicate_pending_update(&metadata.last_known_status, target_revision) {
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
            (UpdateMode::Automatic, UpdateDecision::Ignore) => Err(
                WorkerExecutorError::invalid_request("Cannot update an exited worker"),
            ),
            (UpdateMode::Automatic, decision) => {
                debug!("Enqueuing update");
                let update_attempt_index = worker.enqueue_automatic_update(target_revision).await?;

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
                        worker.set_interrupting(InterruptKind::Restart).await?;
                        debug!("Interrupted running worker for update");
                    }
                    UpdateDecision::Ignore => unreachable!(),
                }
                Ok(update_attempt_index)
            }
            (UpdateMode::Manual, decision) => {
                let update_attempt_index = worker.enqueue_manual_update(target_revision).await?;

                match decision {
                    UpdateDecision::Queue => {
                        debug!("Skipping worker activation due to disable_wakeup flag")
                    }
                    UpdateDecision::QueueAndStart => {
                        Self::start_if_needed(worker).await?;
                    }
                    UpdateDecision::Ignore | UpdateDecision::QueueAndRestart => unreachable!(),
                }
                Ok(update_attempt_index)
            }
        }
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
        let worker = Self::get_existing_suspended(deps, owned_agent_id, principal).await?;
        worker.ensure_component_agent_ingress("revert")?;
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
        let identity = deps
            .worker_service()
            .resolve_agent_identity(owned_agent_id)
            .await?
            .ok_or_else(|| WorkerExecutorError::worker_not_found(owned_agent_id.agent_id()))?;
        let agent_mode = identity.agent_mode;

        let oplog_service = deps.oplog_service();
        let observed_oplog_index = oplog_service
            .get_last_index(owned_agent_id, agent_mode)
            .await?;
        let mut current = observed_oplog_index;
        let mut found = 0;

        loop {
            let entries = oplog_service
                .read_exact(owned_agent_id, agent_mode, current, 1)
                .await?;
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
        let worker = Self::get_existing_suspended(deps, owned_agent_id, principal).await?;
        worker.ensure_component_agent_ingress("plugin change")?;
        let metadata = worker.get_latest_worker_metadata().await;
        if worker.deletion_owns_retirement().await {
            return Err(WorkerExecutorError::invalid_request(
                "Worker is being deleted",
            ));
        }
        let component_metadata = deps
            .component_service()
            .get_metadata(
                owned_agent_id.agent_id.component_id,
                Some(metadata.last_known_status.component_revision),
            )
            .await?;
        let agent_type =
            ParsedAgentId::parse_agent_type_name(&owned_agent_id.agent_id.agent_id).ok();
        let grant_id = component_metadata
            .metadata
            .owner_plugins(metadata.owner_kind, agent_type.as_ref())
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
    use golem_common::model::{
        AgentStatusRecord, AssistedSelection, PendingUpdateRef, UsableAutomaticSnapshot,
    };
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
    fn automatic_update_duplicates_are_kind_scoped() {
        let target_revision = ComponentRevision::new(3).unwrap();
        let mut status = AgentStatusRecord::default();
        status.pending_updates.push_back(PendingUpdateRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(5),
            admission_index: OplogIndex::from_u64(5),
            target_revision,
            kind: PendingUpdateKind::Automatic,
        });

        assert!(has_duplicate_pending_update(&status, target_revision));

        status.pending_updates.push_back(PendingUpdateRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(6),
            admission_index: OplogIndex::from_u64(6),
            target_revision,
            kind: PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot: UsableAutomaticSnapshot {
                    index: OplogIndex::from_u64(4),
                    component_revision: ComponentRevision::new(2).unwrap(),
                    filesystem_snapshot: None,
                },
            })),
        });
        assert!(has_duplicate_pending_update(&status, target_revision));
    }

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
            InterruptDecision::Restart,
            InterruptDecision::Ignore,
            InterruptDecision::Restart,
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
            ResumeDecision::Start,
            ResumeDecision::PreviousFailed,
            ResumeDecision::PreviousExited,
        ];
        let expected_forced = [
            ResumeDecision::ForceStart,
            ResumeDecision::Start,
            ResumeDecision::Start,
            ResumeDecision::Start,
            ResumeDecision::Start,
            ResumeDecision::PreviousFailed,
            ResumeDecision::PreviousExited,
        ];

        for ((status, expected), expected_forced) in
            STATUSES.iter().zip(expected).zip(expected_forced)
        {
            assert_eq!(resume_decision(status, None, false), expected);
            assert_eq!(resume_decision(status, None, true), expected_forced);
        }

        assert_eq!(
            resume_decision(&AgentStatus::Failed, Some(OplogErrorKind::Recovery), true),
            ResumeDecision::ForceStart
        );
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

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Enqueue attempting an update.
    ///
    /// The update itself is not performed by the invocation queue's processing loop,
    /// it is going to affect how the worker is recovered next time.
    pub async fn enqueue_automatic_update(
        &self,
        target_revision: ComponentRevision,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        // Admission identity and execution strategy are separate. The first PendingUpdate only
        // admits the request; the invocation loop durably selects full or snapshot-assisted replay
        // when this request reaches the queue head.
        let instance_guard = self.lock_non_stopping_worker().await;
        instance_guard.ensure_not_deleting()?;
        let status = self.state_actor.status().await;
        if lifecycle::has_duplicate_pending_update(&status, target_revision) {
            return Err(WorkerExecutorError::invalid_request(
                "The same update is already in progress",
            ));
        }

        self.bump_read_only_cache_epoch();
        let entry =
            OplogEntry::pending_update(UpdateDescription::Automatic { target_revision }, None);
        let oplog_index = self
            .add_and_commit_oplog_internal(
                &instance_guard,
                entry,
                Some(WorkerCommand::WorkAvailable),
            )
            .await?;
        drop(instance_guard);
        Ok(oplog_index)
    }
    pub(in crate::worker) async fn enqueue_update_for_attempt(
        &self,
        update_description: UpdateDescription,
        update_attempt_index: Option<OplogIndex>,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        // Bump + commit under the same worker lifecycle lock.
        let instance_guard = self.lock_non_stopping_worker().await;
        instance_guard.ensure_not_deleting()?;
        self.bump_read_only_cache_epoch();
        let entry = OplogEntry::pending_update(update_description.clone(), update_attempt_index);
        let oplog_index = self
            .add_and_commit_oplog_internal(
                &instance_guard,
                entry,
                Some(WorkerCommand::WorkAvailable),
            )
            .await?;
        drop(instance_guard);
        Ok(oplog_index)
    }
    /// Enqueues a manual update.
    ///
    /// This enqueues a special function invocation that saves the component's state and
    /// triggers a restart immediately.
    pub async fn enqueue_manual_update(
        &self,
        target_revision: ComponentRevision,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        self.ensure_component_agent_ingress("manual update")?;
        self.enqueue_worker_invocation(AgentInvocation::ManualUpdate { target_revision })
            .await
    }
    /// Reads the `PendingUpdate` oplog entry referenced by `pending` and reconstructs the full
    /// update description, including any snapshot payload reference. The status record only keeps
    /// a lightweight reference, so callers that apply the update hydrate it on demand.
    pub(in crate::worker) async fn hydrate_pending_update(
        &self,
        pending: &PendingUpdateRef,
    ) -> Result<HydratedUpdate, WorkerExecutorError> {
        let entry = self.oplog.read(pending.oplog_index).await?;
        match entry {
            OplogEntry::PendingUpdate { description, .. } => Ok(HydratedUpdate {
                reference: pending.clone(),
                description,
            }),
            other => Err(WorkerExecutorError::unknown(format!(
                "Expected a PendingUpdate oplog entry at index {}, but found {other:?}",
                pending.oplog_index
            ))),
        }
    }
    pub(crate) async fn persist_successful_update(
        self: &Arc<Self>,
        linear_memory: &LinearMemoryTracker,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<EnvironmentPluginGrantId>,
        snapshot_assisted_details: Option<SnapshotAssistedUpdateDetails>,
    ) -> Result<(), WorkerExecutorError> {
        let wallet = self.owner_execution.wallet().await?;
        let boundary = Box::pin(wallet.lock_boundary());
        self.persist_successful_update_with_boundary(
            linear_memory,
            target_revision,
            new_component_size,
            new_active_plugins,
            snapshot_assisted_details,
            wallet,
            boundary,
        )
        .await
    }
    pub(crate) async fn persist_successful_update_with_boundary(
        self: &Arc<Self>,
        linear_memory: &LinearMemoryTracker,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<EnvironmentPluginGrantId>,
        snapshot_assisted_details: Option<SnapshotAssistedUpdateDetails>,
        wallet: Arc<owner_wallet::OwnerWallet>,
        boundary: futures::future::BoxFuture<
            'static,
            Result<Arc<owner_wallet::CardEventBoundary>, WorkerExecutorError>,
        >,
    ) -> Result<(), WorkerExecutorError> {
        let done = {
            let mut growth = self.memory_growth.lock().unwrap();
            let entry = OplogEntry::successful_update(
                target_revision,
                new_component_size,
                Some(linear_memory.current_bytes()),
                new_active_plugins,
                snapshot_assisted_details,
            );
            *growth = Arc::new(PendingMemoryGrowth::default());
            self.state_actor
                .queue_ordered_oplog_entry(self.clone(), entry, wallet, boundary)
        };
        // A refusal is returned: an update the oplog does not record has not been applied.
        done.await.unwrap_or_else(|_| {
            panic!(
                "Worker state actor for {} dropped an ordered oplog entry",
                self.owned_agent_id
            )
        })
    }
    pub(in crate::worker) async fn activate_plugin_internal(
        &self,
        plugin_grant_id: EnvironmentPluginGrantId,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot activate plugin on a deleting worker",
            ));
        };

        // Plugin activation does not affect invocation results: do not bump
        // the read-only cache epoch.
        self.add_and_commit_oplog_internal(
            &instance_guard,
            OplogEntry::activate_plugin(plugin_grant_id),
            Some(WorkerCommand::WorkAvailable),
        )
        .await?;

        drop(instance_guard);
        Ok(())
    }
    pub(in crate::worker) async fn deactivate_plugin_internal(
        &self,
        plugin_grant_id: EnvironmentPluginGrantId,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot deactivate plugin on a deleting worker",
            ));
        };

        // Plugin deactivation does not affect invocation results: do not bump
        // the read-only cache epoch.
        self.add_and_commit_oplog_internal(
            &instance_guard,
            OplogEntry::deactivate_plugin(plugin_grant_id),
            Some(WorkerCommand::WorkAvailable),
        )
        .await?;

        drop(instance_guard);
        Ok(())
    }
    /// Reverts the worker to a previous state, selected by either the last oplog index to keep
    /// or the number of invocations to drop.
    ///
    /// The revert operations is implemented by inserting a special oplog entry that
    /// extends the worker's deleted oplog regions, skipping entries from the end of the oplog.
    pub(in crate::worker) async fn revert_internal(
        self: &Arc<Self>,
        target: RevertWorkerTarget,
        resolved_revert: Option<ResolvedRevert>,
    ) -> Result<(), WorkerExecutorError> {
        self.ensure_component_agent_ingress("revert")?;
        match target {
            RevertWorkerTarget::RevertToOplogIndex(target) => {
                if resolved_revert.is_some() {
                    return Err(WorkerExecutorError::invalid_request(
                        "Resolved revert must only be supplied for a count-based revert",
                    ));
                }
                self.revert_to_last_oplog_index(target.last_oplog_index, None)
                    .await
            }
            RevertWorkerTarget::RevertLastInvocations(_) => {
                let resolved_revert = resolved_revert.ok_or_else(|| {
                    WorkerExecutorError::invalid_request(
                        "Count-based revert requires a resolved cutoff",
                    )
                })?;
                self.revert_to_last_oplog_index(
                    resolved_revert.last_oplog_index,
                    Some(resolved_revert.observed_oplog_index),
                )
                .await
            }
        }
    }
    pub async fn cancel_invocation(
        &self,
        idempotency_key: IdempotencyKey,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot cancel invocation on a deleting worker",
            ));
        };

        self.add_and_commit_oplog_internal(
            &instance_guard,
            OplogEntry::cancel_pending_invocation(idempotency_key),
            Some(WorkerCommand::WorkAvailable),
        )
        .await?;

        drop(instance_guard);
        Ok(())
    }
    pub(in crate::worker) async fn revert_to_last_oplog_index(
        self: &Arc<Self>,
        last_oplog_index: OplogIndex,
        expected_oplog_index: Option<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        if last_oplog_index == OplogIndex::NONE {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot revert a worker before the create oplog index".to_string(),
            ));
        }

        let worker = self.clone();
        tokio::spawn(async move {
            // The hold stops the deletes of the upload jobs of the agent until the revert ends, so
            // retention removes no name that the revert can make a baseline again.
            let revert_hold = worker.agent_filesystem_snapshots().begin_revert(
                &crate::filesystem_snapshot::AgentSnapshots::agent(
                    &worker.owned_agent_id,
                    worker.initial_worker_metadata.fingerprint,
                ),
            );
            let mut cleanup = worker.owner_cleanup.lock().await;
            if worker.deletion_owns_retirement().await {
                return Err(WorkerExecutorError::invalid_request(
                    "Cannot revert a deleting worker",
                ));
            }
            if *cleanup != OwnerCleanupState::PreRemoval
                || !worker.is_current_cached_owner().await
                || worker.owner_retirement_requested.is_cancelled()
            {
                return Err(WorkerExecutorError::runtime("Worker owner is retiring"));
            }
            worker.owner_retirement_requested.cancel();
            worker.set_interrupting(InterruptKind::Restart).await?;
            worker.durable_stream_producer.fence();
            let forwarding = downcast_oplog::<ForwardingOplog>(&worker.oplog);
            let retirement = worker.retire_durable_stream_producer();
            worker
                .stop_internal(
                    false,
                    None,
                    UnloadRequest::ordinary(UnloadReason::ExplicitStop),
                    FinalWorkerState::Unloaded {
                        startup_failure: None,
                    },
                    PendingLiveInvocationDisposition::Fail,
                )
                .await;
            match &*worker.lock_non_stopping_worker().await {
                WorkerInstance::Unloaded { .. } => {}
                WorkerInstance::CleanupFailed(error) => return Err(error.clone()),
                _ => {
                    return Err(WorkerExecutorError::runtime(
                        "Cannot revert an active or deleting worker",
                    ));
                }
            }
            worker.durable_stream_attachment_reconciler.stop().await;
            retirement.await?;
            worker.state_actor.drain_lifecycle().await?;
            if let Some(forwarding) = &forwarding {
                forwarding.drain_forwarding().await;
            }
            worker.durable_stream_commit()(None).await;

            let instance_guard = worker.lock_non_stopping_worker().await;
            let result = worker
                .append_revert(last_oplog_index, expected_oplog_index)
                .await;
            drop(instance_guard);

            // Even rejected cuts retire this shell. A subsequent request reconstructs its
            // producer and status, while the open oplog generation remains reusable.
            worker.status_flusher.begin_delete().await;
            worker.status_checkpointer.begin_delete().await;
            worker.remove_from_active_agents().await;
            *cleanup = OwnerCleanupState::Retired;
            result.map(|reverted| revert_hold.delete_snapshots(reverted))
        })
        .await
        .map_err(|error| WorkerExecutorError::runtime(format!("Worker revert failed: {error}")))?
    }
    /// Called only after owner writers have drained, with the stopped instance locked. Gives the
    /// filesystem snapshot names that the committed revert made unused.
    pub(in crate::worker) async fn append_revert(
        &self,
        last_oplog_index: OplogIndex,
        expected_oplog_index: Option<OplogIndex>,
    ) -> Result<Box<[FilesystemSnapshotName]>, WorkerExecutorError> {
        use crate::durable_host::durable_stream::StreamStoreError;
        use crate::services::oplog::DurableStreamOplogRecord;

        let region_end = self.oplog.current_oplog_index().await;
        if let Some(expected_oplog_index) = expected_oplog_index
            && region_end != expected_oplog_index
        {
            return Err(WorkerExecutorError::invalid_request(format!(
                "Stale count-based revert resolution: expected oplog index {expected_oplog_index}, found {region_end}"
            )));
        }
        if last_oplog_index >= region_end {
            return Err(WorkerExecutorError::invalid_request(
                "Revert must retain an earlier oplog prefix",
            ));
        }
        let region_start = last_oplog_index.next();
        let last_known_status = self.get_last_known_status().await;
        let dropped_region = OplogRegion {
            start: region_start,
            end: region_end,
        };
        let entries = self
            .oplog
            .read_exact(OplogIndex::INITIAL, region_end.as_u64())
            .await?;

        {
            cut_point::validate_snapshot_update_boundaries(
                &entries,
                last_oplog_index,
                &last_known_status.deleted_regions,
            )
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "Cannot revert worker to oplog index {last_oplog_index}: {error}"
                ))
            })?;

            let validation_regions = revert_validation_regions(&entries, &dropped_region);

            if validation_regions.is_in_deleted_region(last_oplog_index) {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Attempted to revert to a deleted region in oplog to index {last_oplog_index}"
                )));
            }

            if let Some(spanning) = cut_point::find_construct_spanning_cut_point(
                |idx| {
                    std::future::ready(Ok::<_, std::convert::Infallible>(
                        entries
                            .get(&idx)
                            .expect("read_exact must return every requested oplog entry")
                            .clone(),
                    ))
                },
                last_oplog_index,
                region_end,
                &validation_regions,
            )
            .await
            .unwrap_or_else(|never| match never {})
            {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Cannot revert worker to oplog index {last_oplog_index}: the cut point is inside {spanning}"
                )));
            }

            let reverted = filesystem_snapshots::reverted_snapshot_names(
                &entries,
                &dropped_region,
                &last_known_status.deleted_regions,
            );
            let mut prospective_entries = entries;
            prospective_entries.insert(
                region_end.next(),
                OplogEntry::revert(dropped_region.clone()),
            );
            let retained_status = AgentStatusRecord {
                invocation_results: self.config().invocation_results.membership(),
                ..Default::default()
            };
            let retained_status = update_status_with_new_entries(
                self.agent_mode(),
                retained_status,
                prospective_entries,
                &self.config().retry,
            )
            .map_err(WorkerExecutorError::runtime)?
            .ok_or_else(|| {
                WorkerExecutorError::runtime(format!(
                    "Failed to reconstruct worker status at oplog index {last_oplog_index}"
                ))
            })?;

            let restored_component = self.preflight_revert_restoration(&retained_status).await?;

            let validated_oplog_index = self.oplog.current_oplog_index().await;
            if validated_oplog_index != region_end {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Oplog changed while validating revert: expected oplog index {region_end}, found {validated_oplog_index}"
                )));
            }

            // Revert changes observable state, invalidate cached results.
            self.bump_read_only_cache_epoch();

            if last_known_status.has_durable_stream_history {
                let identity = (
                    &self.owned_agent_id,
                    self.initial_worker_metadata.fingerprint,
                );
                let cut = DurableStreamStore::prepare_fork_cut(
                    self.oplog.as_ref(),
                    identity,
                    identity,
                    region_end,
                    last_oplog_index,
                    None,
                    [0; 32],
                    true,
                )
                .await
                .map_err(|error| match error {
                    StreamStoreError::InvalidOffset(_) => {
                        WorkerExecutorError::invalid_request(error.to_string())
                    }
                    _ => WorkerExecutorError::runtime(error.to_string()),
                })?;
                let mut entries = vec![
                    DurableStreamOplogRecord::InlineEntry(OplogEntry::revert(
                        dropped_region.clone(),
                    )),
                    DurableStreamOplogRecord::Session(
                        None,
                        Box::new(StreamSessionRecord::ForkCut(cut)),
                    ),
                ];
                for pending in &last_known_status.pending_invocations {
                    if pending.oplog_index > last_oplog_index
                        && let Some(key) = &pending.idempotency_key
                        && self
                            .worker_service()
                            .lookup_durable_stream_session(
                                &self.owned_agent_id,
                                self.agent_mode(),
                                self.initial_worker_metadata.fingerprint,
                                &last_known_status,
                                key,
                            )
                            .await
                            .map_err(|error| {
                                error.into_worker_executor_error(WorkerExecutorError::runtime)
                            })?
                            .is_some_and(|session| session.prepared.is_some())
                    {
                        entries.push(DurableStreamOplogRecord::InlineEntry(
                            OplogEntry::cancel_pending_invocation(key.clone()),
                        ));
                    }
                }
                self.oplog
                    .add_durable_stream_batch(Box::new(move |_| entries))
                    .await
                    .map_err(|error| self.retire_if_fenced(error))?;
            } else {
                // Through the fence-aware helper: a revert is a write like any other, and if the
                // shard has a new owner this must surface rather than be discarded.
                self.add_to_oplog(OplogEntry::revert(dropped_region.clone()))
                    .await?;
            }
            // Below the commit threshold the add only buffers, so this commit is where a new
            // owner of the shard is found. A refused commit gives the error, and the caller then
            // deletes no filesystem snapshot of the dropped region.
            self.commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
            self.hydrated_invocation_results.write().await.clear();
            self.current_component.store(Arc::new(restored_component));
            Ok(reverted)
        }
    }
    pub(in crate::worker) async fn preflight_revert_restoration(
        &self,
        status: &AgentStatusRecord,
    ) -> Result<golem_service_base::model::component::Component, WorkerExecutorError> {
        let checked_head = revert_checked_head(status);
        let active_revision = checked_head
            .map(|update| update.target_revision)
            .unwrap_or(status.component_revision);
        let (_, active_component) = self
            .component_service()
            .get(
                &self.engine(),
                self.owned_agent_id.component_id(),
                active_revision,
            )
            .await?;

        let selection = self.select_start(status).await?;
        let replay_revision = selection.replay_revision_without_unavailable;
        let replay_component = if active_component.revision == replay_revision {
            active_component.clone()
        } else {
            self.component_service()
                .get_metadata(self.owned_agent_id.component_id(), Some(replay_revision))
                .await?
        };

        if let Some(snapshot) = &status.authoritative_snapshot {
            self.preflight_snapshot_update_payload(snapshot.index)
                .await?;
        }
        // The record of a pending baseline, when the start uses it: its update is the checked
        // head, which the start does not fail before it restores the record.
        let pending_snapshot_index = match &selection.baseline {
            snapshot_selection::SelectedBaseline::ManualPending { head, .. } => {
                (checked_head == Some(head.as_ref())).then_some(head.oplog_index)
            }
            snapshot_selection::SelectedBaseline::AssistedPending { snapshot, head } => {
                (checked_head == Some(head.as_ref())).then_some(snapshot.index)
            }
            snapshot_selection::SelectedBaseline::Periodic(_)
            | snapshot_selection::SelectedBaseline::AssistedPromoted { .. }
            | snapshot_selection::SelectedBaseline::ManualPromoted { .. }
            | snapshot_selection::SelectedBaseline::InitialFiles => None,
        };
        if let Some(snapshot_index) = pending_snapshot_index
            && Some(snapshot_index)
                != status
                    .authoritative_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.index)
        {
            self.preflight_snapshot_update_payload(snapshot_index)
                .await?;
        }

        let initial_files = self
            .parsed_agent_id
            .as_ref()
            .and_then(|agent_id| {
                replay_component
                    .metadata
                    .agent_type_provision_configs()
                    .get(&agent_id.agent_type)
            })
            .map(|config| config.files.clone())
            .unwrap_or_default();
        prepare_initial_files(
            self.file_loader(),
            self.owned_agent_id.environment_id,
            &initial_files,
        )
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;

        Ok(active_component)
    }
    pub(in crate::worker) async fn preflight_snapshot_update_payload(
        &self,
        snapshot_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        match self.oplog.read(snapshot_index).await? {
            OplogEntry::PendingUpdate {
                description: UpdateDescription::SnapshotBased { payload, .. },
                ..
            } => {
                self.oplog
                    .download_payload(payload)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
            }
            OplogEntry::Snapshot { data, .. } => {
                self.oplog
                    .download_payload(data)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
            }
            _ => {
                return Err(WorkerExecutorError::runtime(format!(
                    "Expected snapshot payload at oplog index {snapshot_index}"
                )));
            }
        }
        Ok(())
    }
}

/// The pending update whose target and record a revert checks before it writes: the head that a
/// start of `status` instantiates, when its strategy is chosen. The start chooses the strategy of
/// an automatic update without a strategy entry, and handles each failure of its target or of
/// the record it selects, so a revert does not check them.
pub(in crate::worker) fn revert_checked_head(
    status: &AgentStatusRecord,
) -> Option<&PendingUpdateRef> {
    snapshot_selection::active_head(status).filter(|update| !is_unselected_automatic(update))
}

pub(in crate::worker) mod deletion;
pub(in crate::worker) mod retirement;
pub(in crate::worker) mod stop;
