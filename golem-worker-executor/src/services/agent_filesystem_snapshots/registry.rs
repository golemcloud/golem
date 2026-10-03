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

use super::rules::{self, JobId, Limits, Next, Refusal, State};
use super::{JobDecision, SnapshotKind};
use crate::filesystem_snapshot::AgentSnapshots;
use golem_common::model::oplog::FilesystemSnapshotName;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

/// The state of the agents of the service, and the signal that wakes its waiters.
///
/// A transition that wakes wakes every waiter, not only the waiters of its agent. The waiters are
/// few: a start that waits for the upload of its own agent, a manual update that waits for the job
/// of its agent, a save that waits for the running save of its agent, a fork attempt that waits for
/// a delete of all snapshots of its source or for another attempt of its request, a clean-up of
/// names that waits during its whole call for a delete of all snapshots of its agent, and the idle
/// workers of the clean-up pool, at most `max_concurrent_uploads`. After a restart of the executor
/// there are none but the workers, because the jobs live in the process. Each woken waiter takes
/// the lock once and checks its own agent in O(1), so a transition costs O(waiters) lock-and-check
/// steps. A signal for each agent would add state and a lifecycle to save those steps.
pub(super) struct Registry {
    state: Mutex<State>,
    changed: watch::Sender<()>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Registry {
    /// A registry without agents, whose clean-ups have `limits`.
    pub(super) fn new(limits: Limits) -> Self {
        Self {
            state: Mutex::new(State::with_limits(limits)),
            changed: watch::Sender::new(()),
        }
    }

    /// Runs `rule`, a transition of [`rules`], on the state: the state moves out of the lock into
    /// the rule, and the next state that the rule gives moves back. Then it wakes the waiters when
    /// the transition says so. The rules have no path that panics in a release build, and the
    /// executor builds with `panic = "abort"`, so the lock is never poisoned with the state moved
    /// out; a rule that panicked would leave `State::default()` behind.
    pub(super) fn apply<T>(&self, rule: impl FnOnce(State) -> Next<T>) -> T {
        let (answer, wakes) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let next = rule(std::mem::take(&mut *state));
            let wakes = next.wakes();
            let (next, answer) = next.into_parts();
            crate::metrics::filesystem_snapshots::set_cleanups_pending(
                rules::agents_with_cleanups(&next),
            );
            *state = next;
            (answer, wakes)
        };
        if wakes {
            self.changed.send_modify(|()| {});
        }
        answer
    }

    /// A receiver that sees each transition that wakes, from now on. Take it before the
    /// transition whose answer decides the wait, so no wake-up is lost between the two.
    pub(super) fn subscribe(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    /// Waits until `found` gives a value for the state. Gives `None` when the registry is gone.
    /// `found` only reads: no transition runs inside it.
    pub(super) async fn until<T>(&self, found: impl Fn(&State) -> Option<T>) -> Option<T> {
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
    pub(super) fn read<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        read(&self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Waits until the job `id` of `agent` is gone, or an admission replaces it. Gives at once when
    /// the registry is gone.
    pub(super) async fn until_job_gone_or_replaceable(&self, agent: &AgentSnapshots, id: JobId) {
        self.until(|state| rules::update_may_ask_again(state, agent, id).then_some(()))
            .await;
    }

    /// Waits until no job runs for `agent`. Gives at once when the registry is gone.
    #[cfg(test)]
    pub(super) async fn until_agent_free(&self, agent: &AgentSnapshots) {
        self.until(|state| rules::is_free(state, agent).then_some(()))
            .await;
    }

    /// Waits until no save of `agent` runs. Gives at once when the registry is gone. It only
    /// reads: the flag of a save is taken by the transition that begins the save call.
    pub(super) async fn until_save_may_start(&self, agent: &AgentSnapshots) {
        self.until(|state| (!rules::save_running(state, agent)).then_some(()))
            .await;
    }

    /// Waits until a delete of all snapshots of `agent` is pending or runs. Never completes when
    /// the registry is gone.
    pub(super) async fn until_all_requested(&self, agent: &AgentSnapshots) {
        if self
            .until(|state| rules::all_requested(state, agent).then_some(()))
            .await
            .is_none()
        {
            std::future::pending::<()>().await;
        }
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
    /// Admits a job of `kind` with `name` for `agent`. `stop` stops the job, and `room` tells
    /// whether the volume has room for a capture. This is the only place that makes the stop of
    /// the deletes of a job: a child of `stop`, which the state of the job holds too. It is
    /// cancelled at once while a revert of the agent holds the deletes of its jobs. Gives the
    /// ticket and the stop of the job that the admission replaced, which the caller cancels.
    pub(super) fn admit(
        registry: &Arc<Registry>,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
        kind: SnapshotKind,
        stop: CancellationToken,
        room: bool,
    ) -> Result<(Self, Option<CancellationToken>), Refusal> {
        let retention_stop = stop.child_token();
        let admitted = registry.apply(|state| {
            rules::admit(
                state,
                agent,
                name,
                kind,
                stop.clone(),
                retention_stop.clone(),
                room,
            )
        })?;
        if admitted.under_revert {
            retention_stop.cancel();
        }
        Ok((
            Self {
                registry: Arc::clone(registry),
                agent: agent.clone(),
                id: admitted.id,
                stop,
                retention_stop,
            },
            admitted.replaced,
        ))
    }

    /// The reports of the runs of the upload of the job, which its limiter makes.
    pub(super) fn runs(&self) -> JobRuns {
        JobRuns {
            registry: Arc::clone(&self.registry),
            agent: self.agent.clone(),
            id: self.id,
        }
    }

    /// Whether an admission replaced the job: the job left the state while its ticket lives.
    pub(super) fn replaced(&self) -> bool {
        self.registry
            .read(|state| rules::has_ended(state, &self.agent, self.id))
    }

    /// The stop of the job, as a token that the limiter of its upload holds.
    pub(super) fn stop_token(&self) -> CancellationToken {
        self.stop.clone()
    }

    /// Records the decision of the job. The first decision stays.
    pub(super) fn decide(&self, decision: JobDecision) {
        self.registry
            .apply(|state| rules::decide(state, &self.agent, self.id, decision));
    }

    /// Completes when the job is stopped: by a delete of all snapshots of the agent, by a delete
    /// of its name, or by the shutdown.
    pub(super) fn until_stopped(&self) -> WaitForCancellationFuture<'_> {
        self.stop.cancelled()
    }

    /// Whether a stop of the job was asked for.
    pub(super) fn stop_requested(&self) -> bool {
        self.stop.is_cancelled()
    }

    /// Completes when the deletes of the job after its save are stopped. The stop of the job
    /// stops them too, and so does a manual update of the agent that finds the job running.
    pub(super) fn deletes_stopped(&self) -> impl Future<Output = ()> + Send + 'static {
        self.retention_stop.clone().cancelled_owned()
    }

    /// The agent of the job.
    pub(super) fn agent(&self) -> &AgentSnapshots {
        &self.agent
    }
}

impl Drop for JobTicket {
    fn drop(&mut self) {
        self.registry
            .apply(|state| rules::end(state, &self.agent, self.id));
    }
}

/// The reports of the runs of the upload of one job.
pub(super) struct JobRuns {
    registry: Arc<Registry>,
    agent: AgentSnapshots,
    id: JobId,
}

impl JobRuns {
    /// A run got a slot. Gives false when the job is no longer live, and then the run does not
    /// start.
    pub(super) fn granted(&self) -> bool {
        self.registry
            .apply(|state| rules::run_granted(state, &self.agent, self.id))
    }

    /// The upload waits for its next run after a failed run.
    pub(super) fn failed(&self) {
        self.registry
            .apply(|state| rules::run_failed(state, &self.agent, self.id));
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
        let id = registry.apply(|state| rules::start_wait(state, agent, name))?;
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
        self.registry
            .apply(|state| rules::unwatch(state, &self.agent, self.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_filesystem_snapshots::ConfirmOutcome;
    use futures::FutureExt as _;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentId, OwnedAgentId};
    use test_r::test;

    #[test]
    fn a_dropped_wait_releases_the_decision_of_the_ended_job() {
        let registry = Arc::new(Registry::default());
        let agent = AgentSnapshots::agent(
            &OwnedAgentId::new(
                EnvironmentId::new(),
                &AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "waiting".to_string(),
                },
            ),
            golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        );
        let name = FilesystemSnapshotName::periodic();
        let (job, _) = JobTicket::admit(
            &registry,
            &agent,
            &name,
            SnapshotKind::Periodic,
            CancellationToken::new(),
            true,
        )
        .expect("admitted");
        assert!(job.runs().granted());
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

    #[test]
    fn the_stop_of_the_deletes_of_a_job_is_a_child_of_its_stop_and_the_state_holds_it() {
        let registry = Arc::new(Registry::default());
        let agent = AgentSnapshots::agent(
            &OwnedAgentId::new(
                EnvironmentId::new(),
                &AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "retention-stop".to_string(),
                },
            ),
            golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        );
        let name = FilesystemSnapshotName::periodic();
        let stop = CancellationToken::new();
        let (job, _) = JobTicket::admit(
            &registry,
            &agent,
            &name,
            SnapshotKind::Periodic,
            stop.clone(),
            true,
        )
        .expect("admitted");
        let refused = JobTicket::admit(
            &registry,
            &agent,
            &name,
            SnapshotKind::Periodic,
            CancellationToken::new(),
            true,
        )
        .err()
        .and_then(|refusal| refusal.running);

        stop.cancel();

        assert!(job.deletes_stopped().now_or_never().is_some());
        assert!(refused.is_some_and(|running| running.retention_stop.is_cancelled()));
    }
}
