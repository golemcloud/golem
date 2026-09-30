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

//! The state of the agents, and the tickets that report to it.
//!
//! The registry holds one [`State`] value and changes it only through the transitions of
//! [`rules`], each with its own answer. Each ticket makes its transition in its constructor, and
//! its `Drop` only reports the end of what it holds.

use super::JobDecision;
use super::rules::{self, JobId, Refusal, State, Transition};
use crate::filesystem_snapshot::AgentSnapshots;
use golem_common::model::oplog::FilesystemSnapshotName;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The state of the agents of the service, and the signal that wakes its waiters.
#[derive(Default)]
pub(super) struct Registry {
    state: Mutex<State>,
    changed: watch::Sender<()>,
}

impl Registry {
    /// Applies the transition `f`, which is the transition `transition` of [`rules`], and wakes
    /// the waiters when [`rules::wakes`] says so. No lock is held across an await, so the state
    /// of a poisoned lock are used as they are.
    fn apply<T>(&self, transition: Transition, f: impl FnOnce(&mut State) -> T) -> T {
        let answer = f(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        if rules::wakes(transition) {
            self.changed.send_modify(|()| {});
        }
        answer
    }

    /// Waits until `found` gives a value for the state. Gives `None` when the registry is gone.
    async fn until<T>(&self, found: impl Fn(&State) -> Option<T>) -> Option<T> {
        let mut value = None;
        self.changed
            .subscribe()
            .wait_for(|()| {
                value = found(&self.state.lock().unwrap_or_else(PoisonError::into_inner));
                value.is_some()
            })
            .await
            .ok()?;
        value
    }

    /// Reads the state now.
    #[cfg(test)]
    pub(super) fn read<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        read(&self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Waits until the job `id` of `agent` is gone. Gives at once when the registry is gone.
    pub(super) async fn until_job_gone(&self, agent: &AgentSnapshots, id: JobId) {
        self.until(|state| rules::has_ended(state, agent, id).then_some(()))
            .await;
    }

    /// Waits until no job runs for `agent`. Gives at once when the registry is gone.
    pub(super) async fn until_agent_free(&self, agent: &AgentSnapshots) {
        self.until(|state| rules::is_free(state, agent).then_some(()))
            .await;
    }
}

/// The admitted job of an agent. Dropping it ends the job.
pub(super) struct JobTicket {
    registry: Arc<Registry>,
    agent: AgentSnapshots,
    id: JobId,
    stop: CancellationToken,
    retention_stop: CancellationToken,
}

impl JobTicket {
    /// Admits a job with `name` for `agent`. `stop` stops the job, and `room` tells whether the
    /// volume has room for a capture. The stop of the deletes of the job is a child of `stop`,
    /// and the state of the job holds the same token.
    pub(super) fn admit(
        registry: &Arc<Registry>,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
        stop: CancellationToken,
        room: bool,
    ) -> Result<Self, Refusal> {
        let retention_stop = stop.child_token();
        let id = registry.apply(Transition::Admit, |state| {
            rules::admit(
                state,
                agent,
                name,
                stop.clone(),
                retention_stop.clone(),
                room,
            )
        })?;
        Ok(Self {
            registry: Arc::clone(registry),
            agent: agent.clone(),
            id,
            stop,
            retention_stop,
        })
    }

    /// The job has started saving: it got a slot of the uploads.
    pub(super) fn saving(&self) {
        self.registry.apply(Transition::Saving, |state| {
            rules::saving(state, &self.agent, self.id)
        });
    }

    /// Records the decision of the job. The first decision stays.
    pub(super) fn decide(&self, decision: JobDecision) {
        self.registry.apply(Transition::Decide, |state| {
            rules::decide(state, &self.agent, self.id, decision)
        });
    }

    /// The stop of the job. A delete of all snapshots of the agent and the shutdown cancel it.
    pub(super) fn stop(&self) -> &CancellationToken {
        &self.stop
    }

    /// The stop of the deletes of the job after its save. The stop of the job cancels it too, and
    /// so does a manual update of the agent that finds the job running.
    pub(super) fn retention_stop(&self) -> &CancellationToken {
        &self.retention_stop
    }

    pub(super) fn agent(&self) -> &AgentSnapshots {
        &self.agent
    }
}

impl Drop for JobTicket {
    fn drop(&mut self) {
        self.registry.apply(Transition::End, |state| {
            rules::end(state, &self.agent, self.id)
        });
    }
}

/// A queued or running delete of all snapshots of an agent. Dropping it ends the delete.
pub(super) struct DeleteAllTicket {
    registry: Arc<Registry>,
    agent: AgentSnapshots,
}

impl DeleteAllTicket {
    /// Marks a delete of all snapshots of `agent`, and gives the stop of the job of the agent, when
    /// one runs.
    pub(super) fn delete_all_snapshots(
        registry: &Arc<Registry>,
        agent: &AgentSnapshots,
    ) -> (Self, Option<CancellationToken>) {
        let stop = registry.apply(Transition::DeleteAllSnapshots, |state| {
            rules::delete_all_snapshots(state, agent)
        });
        (
            Self {
                registry: Arc::clone(registry),
                agent: agent.clone(),
            },
            stop,
        )
    }

    /// Waits until no job runs for the agent. A job cannot start while the delete is marked.
    pub(super) async fn until_agent_free(&self) {
        self.registry.until_agent_free(&self.agent).await;
    }
}

impl Drop for DeleteAllTicket {
    fn drop(&mut self) {
        self.registry
            .apply(Transition::AllSnapshotsDeleted, |state| {
                rules::all_snapshots_deleted(state, &self.agent)
            });
    }
}

/// A start that waits for the decision of a job. Dropping it ends the wait.
pub(super) struct WaitTicket {
    registry: Arc<Registry>,
    agent: AgentSnapshots,
    id: JobId,
}

impl WaitTicket {
    /// Registers a wait for the job of `agent` with `name`, when the start waits for it. Gives
    /// the decision that the job has now otherwise.
    pub(super) fn start_wait(
        registry: &Arc<Registry>,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
    ) -> Result<Self, Option<JobDecision>> {
        let id = registry.apply(Transition::StartWait, |state| {
            rules::start_wait(state, agent, name)
        })?;
        Ok(Self {
            registry: Arc::clone(registry),
            agent: agent.clone(),
            id,
        })
    }

    /// Waits for the decision of the job. A job that ended without one, or a registry that is
    /// gone, gives `Stopped`.
    pub(super) async fn decided(&self) -> JobDecision {
        self.registry
            .until(|state| rules::decision_of(state, &self.agent, self.id))
            .await
            .unwrap_or(JobDecision::Stopped)
    }
}

impl Drop for WaitTicket {
    fn drop(&mut self) {
        self.registry.apply(Transition::Unwatch, |state| {
            rules::unwatch(state, &self.agent, self.id)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_filesystem_snapshots::ConfirmOutcome;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentId, OwnedAgentId};
    use test_r::test;

    #[test]
    fn a_dropped_wait_releases_the_decision_of_the_ended_job() {
        let registry = Arc::new(Registry::default());
        let agent = AgentSnapshots::agent(&OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "waiting".to_string(),
            },
        ));
        let name = FilesystemSnapshotName::periodic();
        let job = JobTicket::admit(&registry, &agent, &name, CancellationToken::new(), true)
            .expect("admitted");
        job.saving();
        let wait = WaitTicket::start_wait(&registry, &agent, &name).expect("waits");
        let id = wait.id;
        job.decide(JobDecision::Confirmed(ConfirmOutcome::Confirmed));
        drop(job);
        let decision =
            |registry: &Registry| registry.read(|state| rules::decision_of(state, &agent, id));

        let while_waiting = decision(&registry);
        drop(wait);
        let after_the_wait = decision(&registry);

        assert_eq!(
            (while_waiting, after_the_wait),
            (
                Some(JobDecision::Confirmed(ConfirmOutcome::Confirmed)),
                Some(JobDecision::Stopped)
            )
        );
    }
}
