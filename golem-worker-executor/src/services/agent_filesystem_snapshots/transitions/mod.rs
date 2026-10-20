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

//! The state of the service and its transitions, as pure functions over plain values: the jobs,
//! the clean-ups, the store calls, the forks and the reverts of each agent, and the queries over
//! that state. The decisions that do not read or change the state are in [`super::decisions`].
//! Nothing here waits, reads a clock, draws a random number or calls the store.

use super::decisions::ForkOutcome;
use super::{JobDecision, SnapshotKind, SnapshotSkip};
use crate::filesystem_snapshot::{AgentSnapshots, SnapshotName};
use golem_common::model::AgentId;
use golem_common::model::oplog::FilesystemSnapshotName;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::{NonZeroU32, NonZeroUsize};
use tokio_util::sync::CancellationToken;

/// The number of a job. It is unique for the life of the process.
pub(super) type JobId = u64;

/// The largest number of agents with pending clean-up work.
pub(super) const MAX_PENDING_CLEANUPS: usize = 65_536;

/// The largest number of pending snapshot names over all agents.
pub(super) const MAX_PENDING_NAMES: usize = 3 << 18;

/// The bounds of the pending clean-up work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Limits {
    /// The largest number of agents with pending work.
    pub(super) pending_cleanups: usize,
    /// The largest number of pending names over all agents.
    pub(super) pending_names: usize,
    /// The largest number of pending names of one agent.
    pub(super) names_per_cleanup: usize,
}

impl Limits {
    /// The limits of a service whose agents each have at most `names_per_cleanup` pending names.
    pub(super) fn new(names_per_cleanup: NonZeroUsize) -> Self {
        Self {
            pending_cleanups: MAX_PENDING_CLEANUPS,
            pending_names: MAX_PENDING_NAMES,
            names_per_cleanup: names_per_cleanup.get(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            pending_cleanups: MAX_PENDING_CLEANUPS,
            pending_names: MAX_PENDING_NAMES,
            names_per_cleanup: MAX_PENDING_NAMES,
        }
    }
}

/// The fork attempt of one request: the target agent and the hash of the request.
pub(super) type Flight = (AgentId, [u8; 32]);

/// The jobs of the agents, the clean-ups, the store calls that have not ended, the fork attempts,
/// and the decisions of ended jobs that a start waits for.
#[derive(Debug, Default)]
pub(super) struct State {
    jobs: HashMap<AgentSnapshots, Job>,
    /// The last ended job of each agent that a start still waits for.
    ended: HashMap<AgentSnapshots, Ended>,
    /// The number of the last admitted job.
    last_job: JobId,
    /// The clean-up work of each agent that has some.
    cleanups: HashMap<AgentSnapshots, Cleanup>,
    /// The work of each agent that can write its snapshots and has not ended: store calls, also
    /// those whose caller stopped waiting, and the source holds of forks.
    busy: HashMap<AgentSnapshots, NonZeroU32>,
    /// The save mark of each agent: the job whose save call may start a run now.
    save_running: HashMap<AgentSnapshots, JobId>,
    /// The agents that a worker of the pool takes next, oldest first. An agent can be here when it
    /// is no longer ready, and `take_ready` skips it.
    ready: VecDeque<AgentSnapshots>,
    /// The agents with pending names, in the order in which the names became pending, each with
    /// the stamp of its pending names. An entry whose stamp is not the stamp of the pending names
    /// of its agent is stale.
    names_order: VecDeque<(AgentSnapshots, u64)>,
    /// The stamp of the next pending names.
    next_order: u64,
    /// The number of agents with pending work.
    pending_entries: usize,
    /// The number of agents with pending names.
    pending_names_entries: usize,
    /// The number of pending names over all agents.
    pending_names_total: usize,
    /// The fork attempts that run on this executor.
    flights: HashMap<Flight, ForkPhase>,
    /// The number of reverts of each agent that hold the deletes of its jobs.
    reverts: HashMap<AgentSnapshots, NonZeroU32>,
    limits: Limits,
}

impl State {
    /// An empty state with `limits`.
    pub(super) fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }
}

/// The job of one agent, from its admission to its end.
#[derive(Debug)]
struct Job {
    id: JobId,
    name: FilesystemSnapshotName,
    kind: SnapshotKind,
    phase: JobPhase,
    /// Where the store call of the upload of the job is.
    run_phase: RunPhase,
    /// Stops the job. It is a child of the shutdown token.
    stop: CancellationToken,
    /// Stops the deletes of the job after its save. It is a child of `stop`, and the ticket of
    /// the job holds the same token.
    retention_stop: CancellationToken,
    /// The number of starts that wait for the decision of the job.
    waiters: u32,
}

/// The decision of an ended job, while starts wait for it.
#[derive(Debug)]
struct Ended {
    id: JobId,
    decision: JobDecision,
    waiters: NonZeroU32,
}

/// Where a job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JobPhase {
    /// The job has its admission and waits for its capture or for its first slot of the
    /// uploads.
    Admitted,
    /// The job has started saving: it got its first slot of the uploads. It stays here until it
    /// decides, also while it waits between two runs without a slot.
    Saving,
    /// The job knows how it ended its work on the snapshot.
    Decided(JobDecision),
}

/// Where the store call of the upload of a job is, as the store reports it through the limiter of
/// the job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunPhase {
    /// No run has had a slot yet, or a run holds one. The job cannot be replaced.
    NotWaiting,
    /// A run failed, and the store call waits for its next run. No write of the call can still
    /// land, and the call runs nothing now.
    WaitingAfterFailure,
    /// The store call waits until its writes that can still land have landed or can no longer
    /// land. It holds no slot, and it starts no run and no write until a new grant.
    WaitingForLateWrites,
}

/// Clean-up work of one agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Work {
    /// Deletes these snapshots.
    Names(HashSet<SnapshotName>),
    /// Deletes every snapshot.
    All,
}

/// The kind of the work that a worker of the pool runs for an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunningWork {
    Names,
    All,
}

/// The clean-up of one agent: the work that waits, and the work that a worker runs now.
#[derive(Debug, Default)]
struct Cleanup {
    pending: Option<Work>,
    running: Option<RunningWork>,
    /// Whether the agent is in `ready` since it was last pushed there.
    queued: bool,
    /// The stamp of the pending names in `names_order`.
    order: Option<u64>,
}

/// What a store call does, for the count of the work of its agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CallKind {
    /// A save of the upload of the job `job`. At most one save call of an agent holds the save
    /// mark, and only that call can start a run. A save call loses the mark when an admission
    /// replaces its job; it then starts no run and no write, and ends after the waits and the
    /// check of its writes that can still land.
    Save { job: JobId },
    /// A copy into the agent `to`. It counts on both agents.
    Copy { to: AgentSnapshots },
    /// Any other call that writes.
    Other,
}

/// Where a fork attempt is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForkPhase {
    /// The attempt copies, or has not reached its publication.
    Copying,
    /// The attempt publishes its target.
    Publishing,
}

/// What the end of a fork attempt does to the snapshots of its stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ForkEnd {
    /// Nothing can publish the stage of the attempt: it never reached its publication, or the
    /// live target has another instance id. The delete of all snapshots of the stage is
    /// requested. `overflow` tells what the bound of the clean-ups did to that request.
    StageDeleted { overflow: Option<Overflow> },
    /// The attempt did not finish its publication. Its stage can be published, so its snapshots
    /// stay.
    StageLeaked,
    /// The attempt published its stage.
    Done,
}

/// What a request of clean-up work gave.
#[derive(Clone, Debug, Default)]
pub(super) struct Requested {
    /// The stop of the job of the agent, when the request stops it.
    pub(super) stop: Option<CancellationToken>,
    /// What the bounds of the clean-ups did to the request, when they did something.
    pub(super) overflow: Option<Overflow>,
}

/// What the bounds of the clean-ups did to a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Overflow {
    /// The bounds refused some names of the request, or dropped the delete of all snapshots.
    Refused,
    /// The delete of all snapshots evicted the pending names of this agent. These names are lost.
    Evicted(AgentSnapshots),
}

/// A transition of [`State`]. The registry wakes its waiters after a transition when
/// [`wakes`] says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transition {
    Admit,
    Replace,
    RunGranted,
    RunFailed,
    RunWaitingForLateWrites,
    Decide,
    End,
    StartWait,
    Unwatch,
    RequestNames,
    RequestAll,
    TakeReady,
    CleanupEnded,
    BeginCall,
    StoreCallEnded,
    RevertBegan,
    RevertEnded,
    ForkBegan,
    ForkPublishing,
    ForkEnded,
}

/// Whether the waiters of the registry must see `transition`. A waiter waits for a decision, for
/// the end of a job, for the end of a save, for clean-up work, or for the end of a fork attempt.
fn wakes(transition: Transition) -> bool {
    match transition {
        Transition::Decide
        | Transition::Replace
        | Transition::RunFailed
        | Transition::RunWaitingForLateWrites
        | Transition::End
        | Transition::RequestNames
        | Transition::RequestAll
        | Transition::CleanupEnded
        | Transition::StoreCallEnded
        | Transition::ForkEnded => true,
        Transition::Admit
        | Transition::RunGranted
        | Transition::StartWait
        | Transition::Unwatch
        | Transition::TakeReady
        | Transition::BeginCall
        | Transition::RevertBegan
        | Transition::RevertEnded
        | Transition::ForkBegan
        | Transition::ForkPublishing => false,
    }
}

/// The next state that a transition gives, with its answer. Each rule names its own transition
/// here, so the registry cannot pair a rule with the wake of another.
#[must_use]
pub(super) struct Next<T> {
    state: State,
    answer: T,
    transition: Transition,
}

impl<T> Next<T> {
    fn of(transition: Transition, state: State, answer: T) -> Self {
        Self {
            state,
            answer,
            transition,
        }
    }

    /// Whether the waiters of the registry must see the transition.
    pub(super) fn wakes(&self) -> bool {
        wakes(self.transition)
    }

    /// The next state and the answer.
    pub(super) fn into_parts(self) -> (State, T) {
        (self.state, self.answer)
    }
}

/// Why an admission gives no job.
#[derive(Clone, Debug)]
pub(super) struct Refusal {
    pub(super) skip: SnapshotSkip,
    /// The job that runs for the agent, when one runs.
    pub(super) running: Option<RunningJob>,
}

/// The job that runs for an agent, as a refused admission sees it.
#[derive(Clone, Debug)]
pub(super) struct RunningJob {
    pub(super) id: JobId,
    /// The stop of the deletes of the job after its save, read under the same lock as `id`.
    pub(super) retention_stop: CancellationToken,
}

/// A job that an admission gave.
#[derive(Debug)]
pub(super) struct Admitted {
    pub(super) id: JobId,
    /// The stop of the job that the admission replaced. The caller cancels it after the
    /// transition.
    pub(super) replaced: Option<CancellationToken>,
    /// Whether a revert of the agent holds the deletes of its jobs. The caller then cancels the
    /// stop of the deletes of the new job after the transition.
    pub(super) under_revert: bool,
}

/// Admits a job of `kind` with `name` for `agent`, in the order room, delete of all snapshots,
/// running job. `stop` stops the job, `retention_stop` stops its deletes after its save, and `room`
/// tells whether the volume has room for a capture. A periodic job that has not decided, whose
/// store call still holds the save mark, and that waits for its next run after a failed run or
/// waits for its late writes, is replaced: the old job leaves the state, its save call loses the
/// save mark, its decision is `Replaced` for the starts that wait for it, and the answer gives its
/// stop, which the caller cancels. Any other running job refuses the admission with
/// `UploadInFlight`. Gives the next state and the job, or the refusal.
pub(super) fn admit(
    mut state: State,
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
    kind: SnapshotKind,
    stop: CancellationToken,
    retention_stop: CancellationToken,
    room: bool,
) -> Next<Result<Admitted, Refusal>> {
    let running = state.jobs.get(agent).map(|job| RunningJob {
        id: job.id,
        retention_stop: job.retention_stop.clone(),
    });
    let replaceable = state
        .jobs
        .get(agent)
        .is_some_and(|job| replaceable(&state, agent, job));
    let skip = if !room {
        Some(SnapshotSkip::VolumeUnderPressure)
    } else if all_requested(&state, agent) {
        Some(SnapshotSkip::DeletingAllSnapshots)
    } else if running.is_some() && !replaceable {
        Some(SnapshotSkip::UploadInFlight)
    } else {
        None
    };
    if let Some(skip) = skip {
        return Next::of(Transition::Admit, state, Err(Refusal { skip, running }));
    }
    let replaced = state.jobs.remove(agent).map(|old| {
        if state.save_running.get(agent) == Some(&old.id) {
            state.save_running.remove(agent);
        }
        ended_with(&mut state, agent, old, JobDecision::Replaced)
    });
    state.last_job += 1;
    let id = state.last_job;
    state.jobs.insert(
        agent.clone(),
        Job {
            id,
            name: name.clone(),
            kind,
            phase: JobPhase::Admitted,
            run_phase: RunPhase::NotWaiting,
            stop,
            retention_stop,
            waiters: 0,
        },
    );
    // The replaced job left the state while its ticket lives, so the agent can be ready now.
    let transition = if replaced.is_some() {
        push_if_ready(&mut state, agent);
        Transition::Replace
    } else {
        Transition::Admit
    };
    let under_revert = state.reverts.contains_key(agent);
    Next::of(
        transition,
        state,
        Ok(Admitted {
            id,
            replaced,
            under_revert,
        }),
    )
}

/// A revert of `agent` begins to hold the deletes of its jobs. Gives the stop of the deletes of
/// the job that runs for the agent, which the caller cancels; each job that is admitted while the
/// hold lives gets its deletes stopped too.
pub(super) fn revert_began(
    mut state: State,
    agent: &AgentSnapshots,
) -> Next<Option<CancellationToken>> {
    state.reverts = with_one_more(state.reverts, agent);
    let retention_stop = state.jobs.get(agent).map(|job| job.retention_stop.clone());
    Next::of(Transition::RevertBegan, state, retention_stop)
}

/// A revert of `agent` ends its hold. A hold that is not there changes nothing.
pub(super) fn revert_ended(mut state: State, agent: &AgentSnapshots) -> Next<()> {
    if state.reverts.contains_key(agent) {
        state.reverts = with_one_less(state.reverts, agent);
    }
    Next::of(Transition::RevertEnded, state, ())
}

/// Whether an admission replaces `job`, the job of `agent`: a periodic job that has not decided,
/// whose store call still holds the save mark, and that waits for its next run after a failed run
/// or waits for its late writes. A job whose call has returned is never replaced.
fn replaceable(state: &State, agent: &AgentSnapshots, job: &Job) -> bool {
    job.kind == SnapshotKind::Periodic
        && matches!(
            job.run_phase,
            RunPhase::WaitingAfterFailure | RunPhase::WaitingForLateWrites
        )
        && !matches!(job.phase, JobPhase::Decided(_))
        && state.save_running.get(agent) == Some(&job.id)
}

/// Keeps the decision of a job that left the state while starts wait for it, and gives its stop.
fn ended_with(
    state: &mut State,
    agent: &AgentSnapshots,
    job: Job,
    decision: JobDecision,
) -> CancellationToken {
    if let Some(waiters) = NonZeroU32::new(job.waiters) {
        state.ended.insert(
            agent.clone(),
            Ended {
                id: job.id,
                decision,
                waiters,
            },
        );
    }
    job.stop
}

/// A run of the job `id` got a slot of the uploads: the job is saving, and it waits after no
/// failure. Gives false when the job is no longer live, and then the slot goes back and the run
/// does not start.
pub(super) fn run_granted(mut state: State, agent: &AgentSnapshots, id: JobId) -> Next<bool> {
    let granted = match live(&mut state, agent, id) {
        Some(job) => {
            job.run_phase = RunPhase::NotWaiting;
            if job.phase == JobPhase::Admitted {
                job.phase = JobPhase::Saving;
            }
            true
        }
        None => false,
    };
    Next::of(Transition::RunGranted, state, granted)
}

/// A run of the job `id` failed, and its store call waits for its next run. A report of a job that
/// is no longer live changes nothing.
pub(super) fn run_failed(mut state: State, agent: &AgentSnapshots, id: JobId) -> Next<()> {
    if let Some(job) = live(&mut state, agent, id) {
        job.run_phase = RunPhase::WaitingAfterFailure;
    }
    Next::of(Transition::RunFailed, state, ())
}

/// The store call of the job `id` waits until its writes that can still land have landed or can
/// no longer land. A report of a job that is no longer live changes nothing.
pub(super) fn run_waiting_for_late_writes(
    mut state: State,
    agent: &AgentSnapshots,
    id: JobId,
) -> Next<()> {
    if let Some(job) = live(&mut state, agent, id) {
        job.run_phase = RunPhase::WaitingForLateWrites;
    }
    Next::of(Transition::RunWaitingForLateWrites, state, ())
}

/// The stop of the deletes of the job of `agent` after its save, when a job runs.
#[cfg(test)]
pub(super) fn job_retention_stop(
    state: &State,
    agent: &AgentSnapshots,
) -> Option<CancellationToken> {
    state.jobs.get(agent).map(|job| job.retention_stop.clone())
}

/// Whether the job of `agent` waits for its next run after a failed run.
#[cfg(test)]
pub(super) fn job_waits_after_failure(state: &State, agent: &AgentSnapshots) -> bool {
    state
        .jobs
        .get(agent)
        .is_some_and(|job| job.run_phase == RunPhase::WaitingAfterFailure)
}

/// Whether an admission replaces the job `id` of `agent`, as [`admit`] decides.
pub(super) fn is_replaceable(state: &State, agent: &AgentSnapshots, id: JobId) -> bool {
    state
        .jobs
        .get(agent)
        .is_some_and(|job| job.id == id && replaceable(state, agent, job))
}

/// The job `id` decided. The first decision stays.
pub(super) fn decide(
    mut state: State,
    agent: &AgentSnapshots,
    id: JobId,
    decision: JobDecision,
) -> Next<()> {
    if let Some(job) = live(&mut state, agent, id)
        && !matches!(job.phase, JobPhase::Decided(_))
    {
        job.phase = JobPhase::Decided(decision);
    }
    Next::of(Transition::Decide, state, ())
}

/// The live job `id` of `agent` in a state that a rule owns.
fn live<'a>(state: &'a mut State, agent: &AgentSnapshots, id: JobId) -> Option<&'a mut Job> {
    state.jobs.get_mut(agent).filter(|job| job.id == id)
}

/// Frees the agent of the job `id`, and keeps its decision while starts wait for it.
pub(super) fn end(mut state: State, agent: &AgentSnapshots, id: JobId) -> Next<()> {
    if state.jobs.get(agent).is_some_and(|job| job.id == id)
        && let Some(job) = state.jobs.remove(agent)
    {
        let decision = match job.phase {
            JobPhase::Decided(decision) => decision,
            JobPhase::Admitted | JobPhase::Saving => JobDecision::Stopped,
        };
        let _stop = ended_with(&mut state, agent, job, decision);
    }
    push_if_ready(&mut state, agent);
    Next::of(Transition::End, state, ())
}

/// A start waits only for a job of the agent with the name that has started saving and has not
/// decided. It then registers as a waiter in the same transition and gets the job. A
/// start that does not wait gets the decision of the job with the name, when known. Gives the
/// next state and the answer.
pub(super) fn start_wait(
    mut state: State,
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
) -> Next<Result<JobId, Option<JobDecision>>> {
    let answer = match state.jobs.get_mut(agent).filter(|job| &job.name == name) {
        Some(job) => match job.phase {
            JobPhase::Saving => {
                job.waiters += 1;
                Ok(job.id)
            }
            JobPhase::Decided(decision) => Err(Some(decision)),
            JobPhase::Admitted => Err(None),
        },
        None => Err(None),
    };
    Next::of(Transition::StartWait, state, answer)
}

/// Takes one waiter from the job `id`, live or ended.
pub(super) fn unwatch(mut state: State, agent: &AgentSnapshots, id: JobId) -> Next<()> {
    if let Some(job) = live(&mut state, agent, id) {
        job.waiters = job.waiters.saturating_sub(1);
        return Next::of(Transition::Unwatch, state, ());
    }
    let left = state
        .ended
        .get(agent)
        .filter(|ended| ended.id == id)
        .map(|ended| NonZeroU32::new(ended.waiters.get() - 1));
    match left {
        Some(Some(waiters)) => {
            if let Some(ended) = state.ended.get_mut(agent) {
                ended.waiters = waiters;
            }
        }
        Some(None) => {
            state.ended.remove(agent);
        }
        None => {}
    }
    Next::of(Transition::Unwatch, state, ())
}

/// The decision of the job `id` of `agent`, or `None` while it runs undecided. A job that ended
/// without a kept decision counts as stopped.
pub(super) fn decision_of(state: &State, agent: &AgentSnapshots, id: JobId) -> Option<JobDecision> {
    match state.jobs.get(agent).filter(|job| job.id == id) {
        Some(job) => match job.phase {
            JobPhase::Decided(decision) => Some(decision),
            JobPhase::Admitted | JobPhase::Saving => None,
        },
        None => Some(
            state
                .ended
                .get(agent)
                .filter(|ended| ended.id == id)
                .map_or(JobDecision::Stopped, |ended| ended.decision),
        ),
    }
}

/// Whether the job `id` of `agent` ended.
pub(super) fn has_ended(state: &State, agent: &AgentSnapshots, id: JobId) -> bool {
    state.jobs.get(agent).is_none_or(|job| job.id != id)
}

/// The number of agents with pending or running clean-up work.
pub(super) fn agents_with_cleanups(state: &State) -> usize {
    state.cleanups.len()
}

/// Whether no job runs for `agent`.
#[cfg(test)]
pub(super) fn is_free(state: &State, agent: &AgentSnapshots) -> bool {
    !state.jobs.contains_key(agent)
}

/// Requests the delete of the snapshots `names` of `agent`. The request merges into the pending
/// names of the agent, and is ignored while a delete of all snapshots of the agent is pending or
/// runs. Only the names of a revert come here. The pending names keep their place: the names of
/// the request are added in their order while they fit under the limit of one agent and the limit
/// of all agents, and the names that do not fit are refused. A request for an agent without
/// pending work, also an agent whose clean-up runs, is refused when the limit of the agents with
/// pending work is reached. An empty request changes nothing. The answer tells
/// whether names were refused, and holds the stop of the job of the agent when `names` holds the
/// name of the job, also when the bounds refuse the names.
pub(super) fn request_names(
    mut state: State,
    agent: &AgentSnapshots,
    names: &[SnapshotName],
) -> Next<Requested> {
    let stop = state
        .jobs
        .get(agent)
        .filter(|job| names.iter().any(|name| name.as_str() == job.name.as_str()))
        .map(|job| job.stop.clone());
    let overflow = add_names(&mut state, agent, names).then_some(Overflow::Refused);
    push_if_ready(&mut state, agent);
    Next::of(
        Transition::RequestNames,
        state,
        Requested { stop, overflow },
    )
}

/// Adds `names` to the pending names of `agent` within the limits, and tells whether some names
/// were refused.
fn add_names(state: &mut State, agent: &AgentSnapshots, names: &[SnapshotName]) -> bool {
    if names.is_empty() || all_requested(state, agent) {
        return false;
    }
    if !has_pending(state, agent) && state.pending_entries >= state.limits.pending_cleanups {
        return true;
    }
    let limits = state.limits;
    let total = state.pending_names_total;
    let cleanup = state.cleanups.entry(agent.clone()).or_default();
    let created = cleanup.pending.is_none();
    let Work::Names(pending) = cleanup
        .pending
        .get_or_insert_with(|| Work::Names(HashSet::new()))
    else {
        return false;
    };
    let room = limits
        .pending_names
        .saturating_sub(total)
        .min(limits.names_per_cleanup.saturating_sub(pending.len()));
    let (added, overflow) = names
        .iter()
        .fold((0usize, false), |(added, overflow), name| {
            if pending.contains(name) {
                (added, overflow)
            } else if added < room {
                pending.insert(name.clone());
                (added + 1, overflow)
            } else {
                (added, true)
            }
        });
    if created && added == 0 {
        cleanup.pending = None;
        if cleanup.running.is_none() {
            state.cleanups.remove(agent);
        }
        return overflow;
    }
    state.pending_names_total += added;
    if created {
        state.pending_entries += 1;
        state.pending_names_entries += 1;
        push_order(state, agent);
    }
    overflow
}

/// Requests the delete of all snapshots of `agent`. It replaces the pending names of the agent,
/// and carries no names, so the limit of the names of one agent does not apply to it. When the
/// limit of the agents with pending work is reached and the agent has no pending work, also when
/// its clean-up runs, it evicts the oldest pending names of another agent, which count as a leaked
/// clean-up, and it is dropped only when no pending names are left to evict. The answer names the
/// evicted agent, and holds the stop of the job of the agent, also when the bound drops the
/// request.
pub(super) fn request_all(mut state: State, agent: &AgentSnapshots) -> Next<Requested> {
    let stop = state.jobs.get(agent).map(|job| job.stop.clone());
    let overflow = add_all(&mut state, agent);
    push_if_ready(&mut state, agent);
    Next::of(Transition::RequestAll, state, Requested { stop, overflow })
}

/// Makes the delete of all snapshots of `agent` pending, and tells what the bound of the agents
/// with pending work did to it: nothing, dropped it, or evicted the pending names of another agent.
fn add_all(state: &mut State, agent: &AgentSnapshots) -> Option<Overflow> {
    let pending_names = match state
        .cleanups
        .get(agent)
        .and_then(|cleanup| cleanup.pending.as_ref())
    {
        Some(Work::All) => return None,
        Some(Work::Names(names)) => Some(names.len()),
        None => None,
    };
    let overflow = match pending_names {
        Some(names) => {
            state.pending_names_total = state.pending_names_total.saturating_sub(names);
            state.pending_names_entries = state.pending_names_entries.saturating_sub(1);
            None
        }
        None if state.pending_entries >= state.limits.pending_cleanups => {
            let Some(evicted) = evict_oldest_names(state) else {
                return Some(Overflow::Refused);
            };
            state.pending_entries += 1;
            Some(Overflow::Evicted(evicted))
        }
        None => {
            state.pending_entries += 1;
            None
        }
    };
    let cleanup = state.cleanups.entry(agent.clone()).or_default();
    cleanup.pending = Some(Work::All);
    cleanup.order = None;
    overflow
}

/// Whether `agent` has pending clean-up work.
fn has_pending(state: &State, agent: &AgentSnapshots) -> bool {
    state
        .cleanups
        .get(agent)
        .is_some_and(|cleanup| cleanup.pending.is_some())
}

/// Drops the oldest pending names of an agent, and gives that agent, when there was one. The work
/// that the agent runs stays.
fn evict_oldest_names(state: &mut State) -> Option<AgentSnapshots> {
    let cleanups = &state.cleanups;
    let (agent, _) = std::iter::from_fn(|| state.names_order.pop_front())
        .find(|(agent, stamp)| holds_names_of(cleanups, agent, *stamp))?;
    let cleanup = state.cleanups.get_mut(&agent)?;
    let names = match cleanup.pending.take() {
        Some(Work::Names(names)) => names.len(),
        other => {
            cleanup.pending = other;
            return None;
        }
    };
    cleanup.order = None;
    if cleanup.running.is_none() {
        state.cleanups.remove(&agent);
    }
    state.pending_names_total = state.pending_names_total.saturating_sub(names);
    state.pending_names_entries = state.pending_names_entries.saturating_sub(1);
    state.pending_entries = state.pending_entries.saturating_sub(1);
    Some(agent)
}

/// Whether the order entry of `agent` with `stamp` stands for the pending names of the agent.
fn holds_names_of(
    cleanups: &HashMap<AgentSnapshots, Cleanup>,
    agent: &AgentSnapshots,
    stamp: u64,
) -> bool {
    cleanups.get(agent).is_some_and(|cleanup| {
        cleanup.order == Some(stamp) && matches!(cleanup.pending, Some(Work::Names(_)))
    })
}

/// Puts the new pending names of `agent` at the back of the order. Stale entries at the front go
/// first, and the order is compacted when it holds more than twice the entries that it needs, so
/// each push costs O(1) amortized.
fn push_order(state: &mut State, agent: &AgentSnapshots) {
    let cleanups = &state.cleanups;
    let stale = state
        .names_order
        .iter()
        .take_while(|(agent, stamp)| !holds_names_of(cleanups, agent, *stamp))
        .count();
    state.names_order.drain(..stale);
    let stamp = state.next_order;
    state.next_order += 1;
    if let Some(cleanup) = state.cleanups.get_mut(agent) {
        cleanup.order = Some(stamp);
    }
    state.names_order.push_back((agent.clone(), stamp));
    if state.names_order.len() > 2 * state.pending_names_entries + 16 {
        let cleanups = &state.cleanups;
        state
            .names_order
            .retain(|(agent, stamp)| holds_names_of(cleanups, agent, *stamp));
    }
}

/// Whether a worker of the pool can run the pending work of `agent` now: no work of the agent
/// runs, no store call or fork hold of the agent is open, and no job of the agent blocks the
/// work. A job blocks a delete of all snapshots, and a delete of names that holds its name.
pub(super) fn is_ready(state: &State, agent: &AgentSnapshots) -> bool {
    ready_in(&state.cleanups, &state.busy, &state.jobs, agent)
}

fn ready_in(
    cleanups: &HashMap<AgentSnapshots, Cleanup>,
    busy: &HashMap<AgentSnapshots, NonZeroU32>,
    jobs: &HashMap<AgentSnapshots, Job>,
    agent: &AgentSnapshots,
) -> bool {
    cleanups.get(agent).is_some_and(|cleanup| {
        cleanup.running.is_none()
            && !busy.contains_key(agent)
            && match &cleanup.pending {
                Some(Work::All) => !jobs.contains_key(agent),
                Some(Work::Names(names)) => jobs
                    .get(agent)
                    .is_none_or(|job| !names.contains(job.name.as_str())),
                None => false,
            }
    })
}

/// Puts `agent` at the back of the ready queue when it is ready and not queued.
fn push_if_ready(state: &mut State, agent: &AgentSnapshots) {
    if !is_ready(state, agent) {
        return;
    }
    if let Some(cleanup) = state.cleanups.get_mut(agent)
        && !cleanup.queued
    {
        cleanup.queued = true;
        state.ready.push_back(agent.clone());
    }
}

/// Takes the first ready agent from the ready queue, and moves its pending work to the running
/// work. A queued agent that is not ready is skipped, and the next transition that makes it ready
/// queues it again. Gives `None` only when the queue is empty.
pub(super) fn take_ready(mut state: State) -> Next<Option<(AgentSnapshots, Work)>> {
    let (ready, cleanups, busy, jobs) = (
        &mut state.ready,
        &mut state.cleanups,
        &state.busy,
        &state.jobs,
    );
    let found = std::iter::from_fn(|| ready.pop_front()).find(|agent| {
        let is_ready = ready_in(cleanups, busy, jobs, agent);
        if let Some(cleanup) = cleanups.get_mut(agent) {
            cleanup.queued = false;
        }
        is_ready
    });
    let taken = found.and_then(|agent| {
        let cleanup = state.cleanups.get_mut(&agent)?;
        let work = cleanup.pending.take()?;
        cleanup.order = None;
        cleanup.running = Some(match work {
            Work::Names(_) => RunningWork::Names,
            Work::All => RunningWork::All,
        });
        state.pending_entries = state.pending_entries.saturating_sub(1);
        if let Work::Names(names) = &work {
            state.pending_names_total = state.pending_names_total.saturating_sub(names.len());
            state.pending_names_entries = state.pending_names_entries.saturating_sub(1);
        }
        Some((agent, work))
    });
    Next::of(Transition::TakeReady, state, taken)
}

/// The running clean-up work of `agent` ended. An agent without pending work leaves the
/// clean-ups. The end of a delete of all snapshots also drops the ended decision of the agent.
pub(super) fn cleanup_ended(mut state: State, agent: &AgentSnapshots) -> Next<()> {
    if let Some(cleanup) = state.cleanups.get_mut(agent) {
        if cleanup.running.take() == Some(RunningWork::All) {
            state.ended.remove(agent);
        }
        if cleanup.pending.is_none() {
            state.cleanups.remove(agent);
        }
    }
    push_if_ready(&mut state, agent);
    Next::of(Transition::CleanupEnded, state, ())
}

/// Whether a delete of all snapshots of `agent` is pending or runs.
pub(super) fn all_requested(state: &State, agent: &AgentSnapshots) -> bool {
    state.cleanups.get(agent).is_some_and(|cleanup| {
        matches!(cleanup.pending, Some(Work::All)) || cleanup.running == Some(RunningWork::All)
    })
}

/// Whether a save call of `agent` holds the save mark now.
pub(super) fn save_running(state: &State, agent: &AgentSnapshots) -> bool {
    state.save_running.contains_key(agent)
}

/// The number of open store calls and fork holds of `agent`.
#[cfg(test)]
pub(super) fn busy(state: &State, agent: &AgentSnapshots) -> u32 {
    state.busy.get(agent).map_or(0, |count| count.get())
}

/// The clean-up work that a shutdown loses: the pending work of each agent and the running work
/// of each agent, which the shutdown drops.
pub(super) fn cleanups_lost_at_shutdown(state: &State) -> usize {
    state.pending_entries
        + state
            .cleanups
            .values()
            .filter(|cleanup| cleanup.running.is_some())
            .count()
}

/// Begins a store call of `kind` for `agent`. A save is refused while another save call of the
/// agent holds the save mark; a save that begins takes the mark. The call counts on `agent`, and a
/// copy also on the agent that it copies into.
pub(super) fn begin_call(mut state: State, agent: &AgentSnapshots, kind: &CallKind) -> Next<bool> {
    let begun = match kind {
        CallKind::Save { job } => {
            if state.save_running.contains_key(agent) {
                false
            } else {
                state.save_running.insert(agent.clone(), *job);
                true
            }
        }
        CallKind::Copy { .. } | CallKind::Other => true,
    };
    if begun {
        state.busy = with_one_more(state.busy, agent);
        if let CallKind::Copy { to } = kind {
            state.busy = with_one_more(state.busy, to);
        }
    }
    Next::of(Transition::BeginCall, state, begun)
}

/// A store call of `kind` for `agent` ended. Its counts end, and a save gives the save mark back
/// when it still holds it.
pub(super) fn store_call_ended(
    mut state: State,
    agent: &AgentSnapshots,
    kind: &CallKind,
) -> Next<()> {
    state.busy = with_one_less(state.busy, agent);
    match kind {
        CallKind::Save { job } => {
            if state.save_running.get(agent) == Some(job) {
                state.save_running.remove(agent);
            }
        }
        CallKind::Copy { to } => {
            state.busy = with_one_less(state.busy, to);
            push_if_ready(&mut state, to);
        }
        CallKind::Other => {}
    }
    push_if_ready(&mut state, agent);
    Next::of(Transition::StoreCallEnded, state, ())
}

/// The fork attempt `flight` holds the snapshots of its source `from` before it reads the oplog of
/// the source. It is refused while a delete of all snapshots of the source is pending or runs, and
/// while another attempt of the same flight runs.
pub(super) fn fork_began(mut state: State, from: &AgentSnapshots, flight: &Flight) -> Next<bool> {
    let began = !all_requested(&state, from) && !state.flights.contains_key(flight);
    if began {
        state.busy = with_one_more(state.busy, from);
        state.flights.insert(flight.clone(), ForkPhase::Copying);
    }
    Next::of(Transition::ForkBegan, state, began)
}

/// The fork attempt `flight` starts its publication.
pub(super) fn fork_publishing(mut state: State, flight: &Flight) -> Next<()> {
    if let Some(phase) = state.flights.get_mut(flight) {
        *phase = ForkPhase::Publishing;
    }
    Next::of(Transition::ForkPublishing, state, ())
}

/// The fork attempt `flight` of the source `from` ended, with the snapshots of its stage in
/// `stage` once its copy began, and with the `outcome` of its publication or of a reconciliation
/// once it knows one. Its hold of the source ends. When nothing can publish `stage`, because the
/// attempt never reached its publication or another stage is live, the delete of all snapshots of
/// `stage` is requested in the same transition. An attempt whose publication began and whose
/// outcome is not known leaves `stage`.
pub(super) fn fork_ended(
    mut state: State,
    from: &AgentSnapshots,
    flight: &Flight,
    stage: Option<&AgentSnapshots>,
    outcome: Option<ForkOutcome>,
) -> Next<ForkEnd> {
    state.busy = with_one_less(state.busy, from);
    let phase = state.flights.remove(flight);
    let end = match (phase, outcome, stage) {
        (None, _, _) | (_, Some(ForkOutcome::Published), _) | (_, _, None) => ForkEnd::Done,
        (Some(ForkPhase::Copying), _, Some(stage)) | (_, Some(ForkOutcome::Lost), Some(stage)) => {
            let overflow = add_all(&mut state, stage);
            push_if_ready(&mut state, stage);
            ForkEnd::StageDeleted { overflow }
        }
        (Some(ForkPhase::Publishing), Some(ForkOutcome::Unknown) | None, Some(_)) => {
            ForkEnd::StageLeaked
        }
    };
    push_if_ready(&mut state, from);
    Next::of(Transition::ForkEnded, state, end)
}

/// `busy` with one more count for `agent`.
fn with_one_more(
    mut busy: HashMap<AgentSnapshots, NonZeroU32>,
    agent: &AgentSnapshots,
) -> HashMap<AgentSnapshots, NonZeroU32> {
    busy.entry(agent.clone())
        .and_modify(|count| *count = count.saturating_add(1))
        .or_insert(NonZeroU32::MIN);
    busy
}

/// `busy` with one count less for `agent`. The last count removes the agent. A missing count
/// changes nothing.
fn with_one_less(
    mut busy: HashMap<AgentSnapshots, NonZeroU32>,
    agent: &AgentSnapshots,
) -> HashMap<AgentSnapshots, NonZeroU32> {
    let left = busy
        .get(agent)
        .map(|count| NonZeroU32::new(count.get().saturating_sub(1)));
    debug_assert!(
        left.is_some(),
        "a count of the work of an agent ended twice"
    );
    match left {
        Some(Some(left)) => {
            busy.insert(agent.clone(), left);
        }
        Some(None) => {
            busy.remove(agent);
        }
        None => {}
    }
    busy
}

/// Whether a manual update that waits for the job `id` of `agent` asks for an admission again: the
/// job is gone, or an admission can replace it.
pub(super) fn update_may_ask_again(state: &State, agent: &AgentSnapshots, id: JobId) -> bool {
    has_ended(state, agent, id) || is_replaceable(state, agent, id)
}

#[cfg(test)]
mod tests;
