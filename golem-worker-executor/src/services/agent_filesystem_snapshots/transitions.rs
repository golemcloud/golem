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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ForkEnd {
    /// Nothing can publish the stage of the attempt: it never reached its publication, or the
    /// live target has another instance id. The delete of all snapshots of the stage is
    /// requested. `overflow` tells whether the bound of the clean-ups dropped that request or
    /// evicted other work.
    StageDeleted { overflow: bool },
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
    /// Whether the bound of the clean-ups dropped some of the request, or evicted other work for
    /// it.
    pub(super) overflow: bool,
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
/// runs. The pending names keep their place: the names of the request are added in their order
/// while they fit under the limit of one agent and the limit of all agents, and the names that do
/// not fit are refused. A request for an agent without pending work is refused when the limit of
/// the agents with pending work is reached. An empty request changes nothing. The answer tells
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
    let overflow = add_names(&mut state, agent, names);
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
    let exists = state.cleanups.contains_key(agent);
    if !exists && state.pending_entries >= state.limits.pending_cleanups {
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

/// Requests the delete of all snapshots of `agent`. It replaces the pending names of the agent.
/// When the limit of the agents with pending work is reached, it evicts the oldest pending names
/// of another agent, and it is dropped only when no pending names are left to evict. The answer
/// holds the stop of the job of the agent, also when the bound drops the request.
pub(super) fn request_all(mut state: State, agent: &AgentSnapshots) -> Next<Requested> {
    let stop = state.jobs.get(agent).map(|job| job.stop.clone());
    let overflow = add_all(&mut state, agent);
    push_if_ready(&mut state, agent);
    Next::of(Transition::RequestAll, state, Requested { stop, overflow })
}

/// Makes the delete of all snapshots of `agent` pending, and tells whether the bound dropped it or
/// evicted other work.
fn add_all(state: &mut State, agent: &AgentSnapshots) -> bool {
    let pending_names = match state.cleanups.get(agent).map(|cleanup| &cleanup.pending) {
        Some(Some(Work::All)) => return false,
        Some(Some(Work::Names(names))) => Some(names.len()),
        Some(None) => None,
        None => {
            let evicted = state.pending_entries >= state.limits.pending_cleanups;
            if evicted && !evict_oldest_names(state) {
                return true;
            }
            state.pending_entries += 1;
            state.cleanups.insert(
                agent.clone(),
                Cleanup {
                    pending: Some(Work::All),
                    ..Cleanup::default()
                },
            );
            return evicted;
        }
    };
    match pending_names {
        Some(names) => {
            state.pending_names_total = state.pending_names_total.saturating_sub(names);
            state.pending_names_entries = state.pending_names_entries.saturating_sub(1);
        }
        None => state.pending_entries += 1,
    }
    if let Some(cleanup) = state.cleanups.get_mut(agent) {
        cleanup.pending = Some(Work::All);
        cleanup.order = None;
    }
    false
}

/// Drops the oldest pending names of an agent, and tells whether there were any. The work that
/// the agent runs stays.
fn evict_oldest_names(state: &mut State) -> bool {
    let cleanups = &state.cleanups;
    let Some((agent, _)) = std::iter::from_fn(|| state.names_order.pop_front())
        .find(|(agent, stamp)| holds_names_of(cleanups, agent, *stamp))
    else {
        return false;
    };
    let Some(cleanup) = state.cleanups.get_mut(&agent) else {
        return false;
    };
    let names = match cleanup.pending.take() {
        Some(Work::Names(names)) => names.len(),
        other => {
            cleanup.pending = other;
            return false;
        }
    };
    cleanup.order = None;
    if cleanup.running.is_none() {
        state.cleanups.remove(&agent);
    }
    state.pending_names_total = state.pending_names_total.saturating_sub(names);
    state.pending_names_entries = state.pending_names_entries.saturating_sub(1);
    state.pending_entries = state.pending_entries.saturating_sub(1);
    true
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
mod tests {
    use super::super::ConfirmOutcome;
    use super::*;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentId, OwnedAgentId};
    use std::collections::HashMap;
    use test_r::test;

    fn agent_snapshots(name: &str) -> AgentSnapshots {
        AgentSnapshots::agent(
            &OwnedAgentId::new(
                EnvironmentId::new(),
                &AgentId {
                    component_id: ComponentId::new(),
                    agent_id: name.to_string(),
                },
            ),
            golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        )
    }

    /// Runs the rule `rule` on the state in `state`, puts its next state back, and gives its
    /// answer, as the registry does.
    fn step<T>(state: &mut State, rule: impl FnOnce(State) -> Next<T>) -> T {
        let (next, answer) = rule(std::mem::take(state)).into_parts();
        *state = next;
        answer
    }

    fn saving(state: &mut State, agent: &AgentSnapshots, id: JobId) {
        step(state, |state| super::run_granted(state, agent, id));
    }

    fn granted(state: &mut State, agent: &AgentSnapshots, id: JobId) -> bool {
        step(state, |state| super::run_granted(state, agent, id))
    }

    /// The save call of the job `id` holds the save mark, as the call that runs the job's save
    /// does, and its run failed: the call waits for its next run.
    fn failed_run(state: &mut State, agent: &AgentSnapshots, id: JobId) {
        if !state.save_running.contains_key(agent) {
            begin(state, agent, &CallKind::Save { job: id });
        }
        step(state, |state| super::run_failed(state, agent, id))
    }

    fn decide(state: &mut State, agent: &AgentSnapshots, id: JobId, decision: JobDecision) {
        step(state, |state| super::decide(state, agent, id, decision))
    }

    fn delete_all_snapshots(
        state: &mut State,
        agent: &AgentSnapshots,
    ) -> Option<CancellationToken> {
        step(state, |state| super::request_all(state, agent)).stop
    }

    fn request_all(state: &mut State, agent: &AgentSnapshots) -> Requested {
        step(state, |state| super::request_all(state, agent))
    }

    fn request_names(
        state: &mut State,
        agent: &AgentSnapshots,
        names: &[SnapshotName],
    ) -> Requested {
        step(state, |state| super::request_names(state, agent, names))
    }

    fn take(state: &mut State) -> Option<(AgentSnapshots, Work)> {
        step(state, take_ready)
    }

    fn ended_cleanup(state: &mut State, agent: &AgentSnapshots) {
        step(state, |state| cleanup_ended(state, agent))
    }

    fn begin(state: &mut State, agent: &AgentSnapshots, kind: &CallKind) -> bool {
        step(state, |state| begin_call(state, agent, kind))
    }

    fn ended_call(state: &mut State, agent: &AgentSnapshots, kind: &CallKind) {
        step(state, |state| store_call_ended(state, agent, kind))
    }

    /// Takes each ready agent and ends its work at once, until none is ready.
    fn drain(state: &mut State) {
        if let Some((agent, _)) = take(state) {
            ended_cleanup(state, &agent);
            drain(state);
        }
    }

    fn snapshot_names(texts: &[&str]) -> Vec<SnapshotName> {
        texts
            .iter()
            .map(|text| SnapshotName::new(text).unwrap())
            .collect()
    }

    fn work_names(work: &Work) -> std::collections::BTreeSet<String> {
        match work {
            Work::Names(names) => names.iter().map(|name| name.as_str().to_string()).collect(),
            Work::All => std::collections::BTreeSet::from(["*".to_string()]),
        }
    }

    fn unwatch(state: &mut State, agent: &AgentSnapshots, id: JobId) {
        step(state, |state| super::unwatch(state, agent, id))
    }

    fn admit(
        state: &mut State,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
        stop: CancellationToken,
        retention_stop: CancellationToken,
        room: bool,
    ) -> Result<JobId, Refusal> {
        admit_kind(
            state,
            agent,
            name,
            SnapshotKind::Periodic,
            stop,
            retention_stop,
            room,
        )
        .map(|admitted| admitted.id)
    }

    fn admit_kind(
        state: &mut State,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
        kind: SnapshotKind,
        stop: CancellationToken,
        retention_stop: CancellationToken,
        room: bool,
    ) -> Result<Admitted, Refusal> {
        step(state, |state| {
            super::admit(state, agent, name, kind, stop, retention_stop, room)
        })
    }

    fn try_admit(
        state: &mut State,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
        room: bool,
    ) -> Result<JobId, Refusal> {
        admit(
            state,
            agent,
            name,
            CancellationToken::new(),
            CancellationToken::new(),
            room,
        )
    }

    fn admitted(state: &mut State, agent: &AgentSnapshots, name: &FilesystemSnapshotName) -> JobId {
        try_admit(state, agent, name, true).expect("admitted")
    }

    fn refusal(result: Result<JobId, Refusal>) -> Option<(SnapshotSkip, Option<JobId>)> {
        result
            .err()
            .map(|refusal| (refusal.skip, refusal.running.map(|running| running.id)))
    }

    fn end_job(state: &mut State, agent: &AgentSnapshots, id: JobId) {
        step(state, |state| end(state, agent, id))
    }

    fn wait(
        state: &mut State,
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
    ) -> Result<JobId, Option<JobDecision>> {
        step(state, |state| start_wait(state, agent, name))
    }

    #[test]
    fn a_clean_up_entry_has_its_measured_size() {
        // The map entry of a clean-up: the key, which shares the namespace of the agent, and the
        // entry. The pending names of the entry live on the heap, counted by `pending_names`.
        assert_eq!(
            (
                std::mem::size_of::<Cleanup>(),
                std::mem::size_of::<(AgentSnapshots, Cleanup)>(),
                std::mem::size_of::<SnapshotName>(),
            ),
            (80, 88, 16)
        );
    }

    #[test]
    fn a_shutdown_loses_the_pending_and_the_running_clean_ups() {
        let mut state = State::default();
        let (pending, running, both) = (
            agent_snapshots("pending"),
            agent_snapshots("running"),
            agent_snapshots("both"),
        );
        let name = || SnapshotName::new(FilesystemSnapshotName::periodic().as_str()).unwrap();
        request_names(&mut state, &running, &[name()]);
        request_names(&mut state, &both, &[name()]);
        let taken = [take(&mut state), take(&mut state)].map(|taken| taken.is_some());
        request_names(&mut state, &both, &[name()]);
        request_names(&mut state, &pending, &[name()]);

        assert_eq!(
            (taken, cleanups_lost_at_shutdown(&state)),
            ([true, true], 4)
        );
    }

    #[test]
    fn a_shutdown_loses_one_pending_and_two_running_clean_ups() {
        let mut state = State::default();
        let (pending, first, second) = (
            agent_snapshots("pending"),
            agent_snapshots("first-running"),
            agent_snapshots("second-running"),
        );
        let name = || SnapshotName::new(FilesystemSnapshotName::periodic().as_str()).unwrap();
        request_names(&mut state, &first, &[name()]);
        request_names(&mut state, &second, &[name()]);
        let taken = [take(&mut state), take(&mut state)].map(|taken| taken.is_some());
        request_names(&mut state, &pending, &[name()]);

        assert_eq!(
            (taken, cleanups_lost_at_shutdown(&state)),
            ([true, true], 3)
        );
    }

    #[test]
    fn a_replacing_admission_queues_the_agent_that_the_replaced_job_held() {
        let mut state = State::default();
        let agent = agent_snapshots("agent");
        let (first, second) = (
            FilesystemSnapshotName::periodic(),
            FilesystemSnapshotName::periodic(),
        );
        let old = admitted(&mut state, &agent, &first);
        granted(&mut state, &agent, old);
        failed_run(&mut state, &agent, old);
        request_names(
            &mut state,
            &agent,
            &[SnapshotName::new(first.as_str()).unwrap()],
        );
        let held = take(&mut state).is_none();

        admitted(&mut state, &agent, &second);
        // The save call of the replaced job runs on as a tail until it returns.
        let while_the_tail_runs = take(&mut state).is_none();
        ended_call(&mut state, &agent, &CallKind::Save { job: old });
        let taken = take(&mut state).map(|(taken, _)| taken);

        assert_eq!(
            (held, while_the_tail_runs, taken),
            (true, true, Some(agent))
        );
    }

    #[test]
    fn an_admission_checks_room_then_a_delete_of_all_snapshots_then_a_running_job() {
        let mut state = State::default();
        let agent = agent_snapshots("order");
        let other = agent_snapshots("other");
        let name = FilesystemSnapshotName::periodic();

        delete_all_snapshots(&mut state, &agent);
        let deleting_without_room = refusal(try_admit(&mut state, &agent, &name, false));
        let deleting = refusal(try_admit(&mut state, &agent, &name, true));
        let first = admitted(&mut state, &other, &name);
        let running = refusal(try_admit(&mut state, &other, &name, true));
        let running_without_room = refusal(try_admit(&mut state, &other, &name, false));

        assert_eq!(
            deleting_without_room,
            Some((SnapshotSkip::VolumeUnderPressure, None))
        );
        assert_eq!(deleting, Some((SnapshotSkip::DeletingAllSnapshots, None)));
        assert_eq!(running, Some((SnapshotSkip::UploadInFlight, Some(first))));
        assert_eq!(
            running_without_room,
            Some((SnapshotSkip::VolumeUnderPressure, Some(first)))
        );
    }

    #[test]
    fn a_job_number_is_never_given_twice_and_an_end_frees_only_its_own_job() {
        let mut state = State::default();
        let agent = agent_snapshots("numbers");
        let name = FilesystemSnapshotName::periodic();

        let first = admitted(&mut state, &agent, &name);
        end_job(&mut state, &agent, first);
        let second = admitted(&mut state, &agent, &name);
        end_job(&mut state, &agent, first);
        let still_running = !is_free(&state, &agent);
        end_job(&mut state, &agent, second);

        assert!(second > first);
        assert!(still_running);
        assert!(is_free(&state, &agent));
        assert!(has_ended(&state, &agent, second));
    }

    #[test]
    fn the_phase_only_moves_forward_and_the_first_decision_stays() {
        let mut state = State::default();
        let agent = agent_snapshots("phase");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut state, &agent, &name);

        let admitted_decision = decision_of(&state, &agent, id);
        saving(&mut state, &agent, id);
        let saving_decision = decision_of(&state, &agent, id);
        decide(&mut state, &agent, id, JobDecision::SaveFailed);
        decide(
            &mut state,
            &agent,
            id,
            JobDecision::Confirmed(ConfirmOutcome::Confirmed),
        );
        saving(&mut state, &agent, id);
        let decided = decision_of(&state, &agent, id);
        let phase = state.jobs.get(&agent).map(|job| job.phase);

        assert_eq!(
            (admitted_decision, saving_decision, decided, phase),
            (
                None,
                None,
                Some(JobDecision::SaveFailed),
                Some(JobPhase::Decided(JobDecision::SaveFailed))
            )
        );
    }

    #[test]
    fn a_start_waits_only_for_a_saving_undecided_job_with_its_name() {
        let mut state = State::default();
        let agent = agent_snapshots("start-wait");
        let name = FilesystemSnapshotName::periodic();
        let other = FilesystemSnapshotName::periodic();

        let no_job = wait(&mut state, &agent, &name);
        let id = admitted(&mut state, &agent, &name);
        let while_admitted = wait(&mut state, &agent, &name);
        saving(&mut state, &agent, id);
        let other_name = wait(&mut state, &agent, &other);
        let while_saving = wait(&mut state, &agent, &name);
        let waiters = state.jobs.get(&agent).map(|job| job.waiters);
        decide(
            &mut state,
            &agent,
            id,
            JobDecision::Confirmed(ConfirmOutcome::Deferred),
        );
        let decided = wait(&mut state, &agent, &name);

        assert_eq!(no_job, Err(None));
        assert_eq!(while_admitted, Err(None));
        assert_eq!(other_name, Err(None));
        assert_eq!(while_saving, Ok(id));
        assert_eq!(waiters, Some(1));
        assert_eq!(
            decided,
            Err(Some(JobDecision::Confirmed(ConfirmOutcome::Deferred)))
        );
    }

    #[test]
    fn an_ended_job_keeps_its_decision_only_while_a_start_waits_for_it() {
        let mut state = State::default();
        let agent = agent_snapshots("ended");
        let name = FilesystemSnapshotName::periodic();

        let unwatched = admitted(&mut state, &agent, &name);
        saving(&mut state, &agent, unwatched);
        decide(
            &mut state,
            &agent,
            unwatched,
            JobDecision::Confirmed(ConfirmOutcome::Superseded),
        );
        end_job(&mut state, &agent, unwatched);
        let without_waiter = (state.ended.len(), decision_of(&state, &agent, unwatched));

        let watched = admitted(&mut state, &agent, &name);
        saving(&mut state, &agent, watched);
        assert!(wait(&mut state, &agent, &name).is_ok());
        assert!(wait(&mut state, &agent, &name).is_ok());
        decide(
            &mut state,
            &agent,
            watched,
            JobDecision::Confirmed(ConfirmOutcome::Superseded),
        );
        end_job(&mut state, &agent, watched);
        let kept = decision_of(&state, &agent, watched);
        unwatch(&mut state, &agent, unwatched);
        unwatch(&mut state, &agent, watched);
        let after_one = state.ended.len();
        unwatch(&mut state, &agent, watched);

        assert_eq!(without_waiter, (0, Some(JobDecision::Stopped)));
        assert_eq!(
            kept,
            Some(JobDecision::Confirmed(ConfirmOutcome::Superseded))
        );
        assert_eq!(after_one, 1);
        assert!(state.ended.is_empty());
    }

    #[test]
    fn an_undecided_job_that_ends_with_a_waiter_keeps_stopped() {
        let mut state = State::default();
        let agent = agent_snapshots("stopped");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut state, &agent, &name);
        saving(&mut state, &agent, id);
        assert!(wait(&mut state, &agent, &name).is_ok());

        end_job(&mut state, &agent, id);

        assert_eq!(
            state.ended.get(&agent).map(|ended| ended.decision),
            Some(JobDecision::Stopped)
        );
    }

    #[test]
    fn an_unwatch_of_a_live_job_takes_one_waiter_and_a_stale_one_does_nothing() {
        let mut state = State::default();
        let agent = agent_snapshots("unwatch");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut state, &agent, &name);
        saving(&mut state, &agent, id);
        assert!(wait(&mut state, &agent, &name).is_ok());
        assert!(wait(&mut state, &agent, &name).is_ok());

        unwatch(&mut state, &agent, id + 1);
        let after_stale = state.jobs.get(&agent).map(|job| job.waiters);
        unwatch(&mut state, &agent, id);

        assert_eq!(after_stale, Some(2));
        assert_eq!(state.jobs.get(&agent).map(|job| job.waiters), Some(1));
    }

    #[test]
    fn a_delete_of_all_snapshots_refuses_admissions_until_its_work_ends_and_drops_the_ended_decision()
     {
        let mut state = State::default();
        let agent = agent_snapshots("deletes");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut state, &agent, &name);
        saving(&mut state, &agent, id);
        assert!(wait(&mut state, &agent, &name).is_ok());
        let stop = state.jobs.get(&agent).map(|job| job.stop.clone());

        let first = delete_all_snapshots(&mut state, &agent);
        let while_the_job_runs = take(&mut state);
        end_job(&mut state, &agent, id);
        let second = delete_all_snapshots(&mut state, &agent);
        let taken = take(&mut state);
        let while_running = (
            refusal(try_admit(&mut state, &agent, &name, true)),
            state.ended.len(),
        );
        ended_cleanup(&mut state, &agent);
        let after = try_admit(&mut state, &agent, &name, true).is_ok();

        assert!(first.is_some_and(|token| Some(&token) == stop.as_ref()));
        assert!(second.is_none());
        assert!(while_the_job_runs.is_none());
        assert_eq!(taken.map(|(_, work)| work), Some(Work::All));
        assert_eq!(
            while_running,
            (Some((SnapshotSkip::DeletingAllSnapshots, None)), 1)
        );
        assert!(after);
        assert!(state.ended.is_empty());
        assert!(state.cleanups.is_empty());
    }

    #[test]
    fn only_transitions_that_end_or_add_work_wake_the_waiters() {
        let agent = agent_snapshots("wakes");
        let name = FilesystemSnapshotName::periodic();
        let names = snapshot_names(&["p-1"]);
        let flight = (
            AgentId {
                component_id: ComponentId::new(),
                agent_id: "target".to_string(),
            },
            [0; 32],
        );
        let fresh = State::default;

        assert_eq!(
            [
                super::admit(
                    fresh(),
                    &agent,
                    &name,
                    SnapshotKind::Periodic,
                    CancellationToken::new(),
                    CancellationToken::new(),
                    true
                )
                .wakes(),
                super::run_granted(fresh(), &agent, 1).wakes(),
                super::run_failed(fresh(), &agent, 1).wakes(),
                super::run_waiting_for_late_writes(fresh(), &agent, 1).wakes(),
                replacing(&agent, &name).wakes(),
                super::decide(fresh(), &agent, 1, JobDecision::Stopped).wakes(),
                end(fresh(), &agent, 1).wakes(),
                start_wait(fresh(), &agent, &name).wakes(),
                super::unwatch(fresh(), &agent, 1).wakes(),
                super::request_names(fresh(), &agent, &names).wakes(),
                super::request_all(fresh(), &agent).wakes(),
                take_ready(fresh()).wakes(),
                cleanup_ended(fresh(), &agent).wakes(),
                begin_call(fresh(), &agent, &CallKind::Other).wakes(),
                store_call_ended(
                    begin_call(fresh(), &agent, &CallKind::Other).into_parts().0,
                    &agent,
                    &CallKind::Other
                )
                .wakes(),
                fork_began(fresh(), &agent, &flight).wakes(),
                fork_publishing(fresh(), &flight).wakes(),
                fork_ended(
                    fork_began(fresh(), &agent, &flight).into_parts().0,
                    &agent,
                    &flight,
                    Some(&agent),
                    Some(ForkOutcome::Published)
                )
                .wakes(),
                super::revert_began(fresh(), &agent).wakes(),
                super::revert_ended(super::revert_began(fresh(), &agent).into_parts().0, &agent)
                    .wakes(),
            ],
            [
                false, false, true, true, true, true, true, false, false, true, true, false, true,
                false, true, false, false, true, false, false
            ]
        );
    }

    /// Admits a periodic job for `agent` and gives whether the deletes of the new job must stop.
    fn admitted_under_revert(state: &mut State, agent: &AgentSnapshots) -> Option<bool> {
        admit_kind(
            state,
            agent,
            &FilesystemSnapshotName::periodic(),
            SnapshotKind::Periodic,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .ok()
        .map(|admitted| admitted.under_revert)
    }

    #[test]
    fn a_revert_hold_stops_the_deletes_of_the_running_job_and_of_each_job_admitted_while_it_lives()
    {
        let mut state = State::default();
        let agent = agent_snapshots("revert-hold");
        let other = agent_snapshots("revert-hold-other");
        let retention_stop = CancellationToken::new();
        let running = admit(
            &mut state,
            &agent,
            &FilesystemSnapshotName::periodic(),
            CancellationToken::new(),
            retention_stop.clone(),
            true,
        )
        .unwrap();
        let before = step(&mut state, |state| revert_began(state, &other));

        let held = step(&mut state, |state| revert_began(state, &agent));
        end_job(&mut state, &agent, running);
        let during = admitted_under_revert(&mut state, &agent);
        let other_during = admitted_under_revert(&mut state, &other);
        step(&mut state, |state| revert_ended(state, &agent));
        let id = state.jobs.get(&agent).map(|job| job.id).unwrap();
        end_job(&mut state, &agent, id);
        let after = admitted_under_revert(&mut state, &agent);

        assert_eq!(
            (
                before.is_none(),
                held.is_some_and(|stop| stop == retention_stop),
                during,
                other_during,
                after
            ),
            (true, true, Some(true), Some(true), Some(false))
        );
    }

    #[test]
    fn two_reverts_hold_until_both_end_and_an_end_without_a_hold_changes_nothing() {
        let mut state = State::default();
        let agent = agent_snapshots("two-reverts");
        step(&mut state, |state| revert_ended(state, &agent));
        step(&mut state, |state| revert_began(state, &agent));
        step(&mut state, |state| revert_began(state, &agent));

        step(&mut state, |state| revert_ended(state, &agent));
        let after_one = state.reverts.contains_key(&agent);
        step(&mut state, |state| revert_ended(state, &agent));
        let after_two = state.reverts.contains_key(&agent);
        step(&mut state, |state| revert_ended(state, &agent));

        assert_eq!(
            (after_one, after_two, state.reverts.is_empty()),
            (true, false, true)
        );
    }

    #[test]
    fn a_burst_of_deletes_of_one_agent_merges_into_one_pending_clean_up() {
        let mut state = State::default();
        let agent = agent_snapshots("burst");

        request_names(&mut state, &agent, &snapshot_names(&["p-1", "p-2"]));
        request_names(&mut state, &agent, &snapshot_names(&["p-2", "p-3"]));
        request_names(&mut state, &agent, &[]);
        let entries = (state.pending_entries, state.pending_names_total);
        let taken = take(&mut state);

        assert_eq!(entries, (1, 3));
        assert_eq!(
            taken.map(|(_, work)| work_names(&work)),
            Some(
                ["p-1", "p-2", "p-3"]
                    .map(String::from)
                    .into_iter()
                    .collect()
            )
        );
        assert!(take(&mut state).is_none());
        assert_eq!((state.pending_entries, state.pending_names_total), (0, 0));
    }

    #[test]
    fn an_empty_delete_of_names_makes_no_entry() {
        let mut state = State::default();
        let agent = agent_snapshots("empty");

        let requested = request_names(&mut state, &agent, &[]);

        assert!(requested.stop.is_none() && !requested.overflow);
        assert!(state.cleanups.is_empty() && state.ready.is_empty());
    }

    #[test]
    fn a_delete_of_all_snapshots_replaces_the_pending_names_of_its_agent() {
        let mut state = State::default();
        let agent = agent_snapshots("replaced");

        request_names(&mut state, &agent, &snapshot_names(&["p-1", "p-2"]));
        request_all(&mut state, &agent);
        let ignored = request_names(&mut state, &agent, &snapshot_names(&["p-3"]));
        let counts = (
            state.pending_entries,
            state.pending_names_entries,
            state.pending_names_total,
        );

        assert!(!ignored.overflow);
        assert_eq!(counts, (1, 0, 0));
        assert_eq!(take(&mut state).map(|(_, work)| work), Some(Work::All));
    }

    #[test]
    fn names_for_an_agent_whose_delete_of_all_runs_and_an_empty_request_at_the_limit_add_nothing() {
        let mut state = limited(1, 10, 10);
        let (running, full, empty) = (
            agent_snapshots("running-all"),
            agent_snapshots("fills-the-limit"),
            agent_snapshots("empty-request"),
        );
        request_all(&mut state, &running);
        let taken = take(&mut state).map(|(agent, work)| (agent == running, work));
        request_all(&mut state, &full);

        let names = request_names(&mut state, &running, &snapshot_names(&["p-1"]));
        let nothing = request_names(&mut state, &empty, &[]);

        assert_eq!(taken, Some((true, Work::All)));
        assert_eq!(
            (
                names.overflow,
                nothing.overflow,
                state
                    .cleanups
                    .get(&running)
                    .map(|cleanup| cleanup.pending.clone()),
                state.cleanups.contains_key(&empty),
                (
                    state.pending_entries,
                    state.pending_names_entries,
                    state.pending_names_total
                ),
            ),
            (false, false, Some(None), false, (1, 0, 0))
        );
    }

    #[test]
    fn a_delete_of_all_for_an_agent_whose_clean_up_runs_counts_one_pending_entry() {
        let mut state = State::default();
        let agent = agent_snapshots("all-while-running");
        request_names(&mut state, &agent, &snapshot_names(&["p-1"]));
        let taken = take(&mut state).map(|(_, work)| work_names(&work));

        let requested = request_all(&mut state, &agent);

        assert_eq!(
            taken,
            Some(std::collections::BTreeSet::from(["p-1".to_string()]))
        );
        assert_eq!((requested.overflow, state.pending_entries), (false, 1));
    }

    #[test]
    fn a_delete_of_names_stops_only_the_job_whose_name_it_holds() {
        let mut state = State::default();
        let agent = agent_snapshots("stops");
        let name = FilesystemSnapshotName::periodic();
        let own = SnapshotName::new(name.as_str()).unwrap();
        let id = admitted(&mut state, &agent, &name);

        let other = request_names(&mut state, &agent, &snapshot_names(&["p-other"]));
        let while_other_job = take(&mut state).map(|(_, work)| work_names(&work));
        ended_cleanup(&mut state, &agent);
        let own_name = request_names(&mut state, &agent, std::slice::from_ref(&own));
        let while_own_job = take(&mut state);
        end_job(&mut state, &agent, id);
        let after_the_job = take(&mut state).map(|(_, work)| work_names(&work));

        assert!(other.stop.is_none());
        assert_eq!(
            while_other_job,
            Some(std::collections::BTreeSet::from(["p-other".to_string()]))
        );
        assert!(own_name.stop.is_some());
        assert!(while_own_job.is_none());
        assert_eq!(
            after_the_job,
            Some(std::collections::BTreeSet::from([own.as_str().to_string()]))
        );
    }

    fn limited(pending_cleanups: usize, pending_names: usize, names_per_cleanup: usize) -> State {
        State::with_limits(Limits {
            pending_cleanups,
            pending_names,
            names_per_cleanup,
        })
    }

    #[test]
    fn a_delete_of_all_snapshots_past_the_limit_evicts_pending_names() {
        let mut state = limited(2, 100, 10);
        let (first, second, third, fourth) = (
            agent_snapshots("first"),
            agent_snapshots("second"),
            agent_snapshots("third"),
            agent_snapshots("fourth"),
        );
        request_names(&mut state, &first, &snapshot_names(&["p-1", "p-2"]));
        request_names(&mut state, &second, &snapshot_names(&["p-3"]));

        let all = request_all(&mut state, &third);
        let names_past_the_limit = request_names(&mut state, &fourth, &snapshot_names(&["p-4"]));
        let all_again = request_all(&mut state, &fourth);
        let no_names_left = request_all(&mut state, &agent_snapshots("fifth"));

        assert!(all.overflow && names_past_the_limit.overflow && all_again.overflow);
        assert!(no_names_left.overflow);
        assert_eq!(
            [&first, &second, &third, &fourth].map(|agent| state.cleanups.contains_key(agent)),
            [false, false, true, true]
        );
        assert_eq!(
            (
                state.pending_entries,
                state.pending_names_entries,
                state.pending_names_total
            ),
            (2, 0, 0)
        );
    }

    #[test]
    fn names_past_the_limit_of_one_entry_are_counted_as_leaked() {
        let mut state = limited(10, 100, 2);
        let agent = agent_snapshots("one-entry");

        let first = request_names(&mut state, &agent, &snapshot_names(&["p-3", "p-2", "p-1"]));
        let again = request_names(&mut state, &agent, &snapshot_names(&["p-3"]));

        assert!(first.overflow);
        assert!(!again.overflow);
        assert_eq!(
            take(&mut state).map(|(_, work)| work_names(&work)),
            Some(["p-3", "p-2"].map(String::from).into_iter().collect())
        );
    }

    #[test]
    fn a_request_past_the_bound_of_one_agent_keeps_the_pending_names_and_refuses_the_new_ones() {
        let bound = crate::services::golem_config::FilesystemSnapshotUploadConfig::default()
            .max_pending_deletes_per_agent();
        let mut state = State::with_limits(Limits::new(bound));
        let agent = agent_snapshots("many-names");
        let texts = (0..bound.get())
            .map(|index| format!("p-{index}"))
            .collect::<Vec<_>>();
        let names = snapshot_names(&texts.iter().map(String::as_str).collect::<Vec<_>>());

        let fits = request_names(&mut state, &agent, &names);
        let past = request_names(&mut state, &agent, &snapshot_names(&["p-new"]));
        let pending = take(&mut state).map(|(_, work)| work_names(&work));

        assert_eq!(
            (bound.get(), fits.overflow, past.overflow),
            (1024, false, true)
        );
        assert_eq!(pending, Some(texts.into_iter().collect()));
    }

    #[test]
    fn names_past_the_global_limit_are_counted_as_leaked() {
        let mut state = limited(10, 3, 10);
        let (first, second) = (agent_snapshots("first"), agent_snapshots("second"));

        let fits = request_names(&mut state, &first, &snapshot_names(&["p-1", "p-2"]));
        let past = request_names(&mut state, &second, &snapshot_names(&["p-3", "p-4"]));
        let none_left = request_names(
            &mut state,
            &agent_snapshots("third"),
            &snapshot_names(&["p-5"]),
        );

        assert!(!fits.overflow && past.overflow && none_left.overflow);
        assert_eq!(state.pending_names_total, 3);
        assert_eq!(state.pending_entries, 2);
    }

    #[test]
    fn names_order_stays_below_twice_the_pending_names_entries() {
        let mut state = limited(1000, 100_000, 10);
        let agents = (0..50)
            .map(|index| agent_snapshots(&format!("agent-{index}")))
            .collect::<Vec<_>>();
        let names = snapshot_names(&["p-1"]);

        let within = (0..20)
            .flat_map(|_| {
                let pushed = agents
                    .iter()
                    .map(|agent| {
                        request_names(&mut state, agent, &names);
                        state.names_order.len() <= 2 * state.pending_names_entries + 16
                    })
                    .collect::<Vec<_>>();
                agents.iter().skip(5).for_each(|agent| {
                    request_all(&mut state, agent);
                });
                drain(&mut state);
                pushed
            })
            .collect::<Vec<_>>();

        assert!(within.iter().all(|within| *within));
    }

    #[test]
    fn the_eviction_takes_the_oldest_pending_names_and_skips_an_older_entry_of_newer_names() {
        // A busy agent keeps its entry at the front. The names of `again` are taken, so its first
        // entry is stale behind the front; its new names come after the names of `newer`.
        let mut state = limited(3, 100, 10);
        let (front, again, newer) = (
            agent_snapshots("front"),
            agent_snapshots("again"),
            agent_snapshots("newer"),
        );
        let names = snapshot_names(&["p-1"]);
        request_names(&mut state, &front, &names);
        begin(&mut state, &front, &CallKind::Other);
        request_names(&mut state, &again, &names);
        let taken = take(&mut state).map(|(agent, _)| agent);
        request_names(&mut state, &newer, &names);
        request_names(&mut state, &again, &names);

        request_all(&mut state, &agent_snapshots("first-all"));
        request_all(&mut state, &agent_snapshots("second-all"));

        let holds_names = |agent: &AgentSnapshots| {
            state
                .cleanups
                .get(agent)
                .is_some_and(|cleanup| matches!(cleanup.pending, Some(Work::Names(_))))
        };
        assert_eq!(taken, Some(again.clone()));
        assert_eq!(
            [&front, &newer, &again].map(holds_names),
            [false, false, true]
        );
    }

    #[test]
    fn names_order_stays_within_its_bound_when_the_live_entries_drop_before_a_push() {
        // A busy agent keeps its entry at the front, so the entries of the names that are taken
        // stay behind it while the live entries drop to one.
        let mut state = limited(1000, 100_000, 10);
        let front = agent_snapshots("front");
        let names = snapshot_names(&["p-1"]);
        request_names(&mut state, &front, &names);
        begin(&mut state, &front, &CallKind::Other);
        (0..30).for_each(|index| {
            request_names(
                &mut state,
                &agent_snapshots(&format!("agent-{index}")),
                &names,
            );
        });
        drain(&mut state);
        let before = (state.names_order.len(), state.pending_names_entries);

        request_names(&mut state, &agent_snapshots("pushed"), &names);

        assert_eq!(before, (31, 1));
        assert!(
            state.names_order.len() <= 2 * state.pending_names_entries + 16,
            "{} entries for {} live ones",
            state.names_order.len(),
            state.pending_names_entries
        );
    }

    #[test]
    fn the_compaction_of_names_order_is_amortized() {
        let mut state = limited(100_000, 1_000_000, 10);
        let names = snapshot_names(&["p-1"]);
        // A live entry at the front keeps the stale entries behind it until a compaction.
        let front = agent_snapshots("front");
        let job = FilesystemSnapshotName::periodic();
        let blocked = SnapshotName::new(job.as_str()).unwrap();
        admitted(&mut state, &front, &job);
        request_names(&mut state, &front, std::slice::from_ref(&blocked));
        // Many live entries make each compaction cost much, so a threshold that does not grow
        // with twice the live entries compacts too often.
        (0..1000).for_each(|index| {
            request_names(
                &mut state,
                &agent_snapshots(&format!("live-{index}")),
                &names,
            );
        });
        let pushes = 10_000usize;

        let compacted = (0..pushes)
            .map(|index| {
                let agent = agent_snapshots(&format!("agent-{index}"));
                let before = state.names_order.len();
                request_names(&mut state, &agent, &names);
                let after = state.names_order.len();
                request_all(&mut state, &agent);
                // A push that left the order shorter than one more entry compacted it, at the
                // cost of the order before the compaction.
                if after <= before { before + 1 } else { 0 }
            })
            .sum::<usize>();

        assert!(
            compacted <= 3 * pushes,
            "the compactions read {compacted} entries for {pushes} pushes"
        );
        assert!(compacted > 0);
        assert!(state.names_order.len() <= 2 * state.pending_names_entries + 16);
    }

    #[test]
    fn a_delete_of_all_snapshots_runs_when_the_tail_of_a_stopped_job_ends() {
        let mut state = State::default();
        let agent = agent_snapshots("tail");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut state, &agent, &name);
        assert!(begin(&mut state, &agent, &CallKind::Save { job: 0 }));

        let stop = delete_all_snapshots(&mut state, &agent);
        end_job(&mut state, &agent, id);
        let while_the_save_runs = take(&mut state);
        let second_save = begin(&mut state, &agent, &CallKind::Save { job: 0 });
        ended_call(&mut state, &agent, &CallKind::Save { job: 0 });
        let after = take(&mut state).map(|(_, work)| work);

        assert!(stop.is_some());
        assert!(while_the_save_runs.is_none());
        assert!(!second_save);
        assert_eq!(after, Some(Work::All));
    }

    #[test]
    fn a_delete_of_all_snapshots_starts_when_the_last_detached_call_of_its_agent_returns() {
        let mut state = State::default();
        let (agent, target) = (agent_snapshots("source"), agent_snapshots("target"));
        let copy = CallKind::Copy { to: target.clone() };
        assert!(begin(&mut state, &agent, &CallKind::Other));
        assert!(begin(&mut state, &agent, &copy));

        delete_all_snapshots(&mut state, &agent);
        delete_all_snapshots(&mut state, &target);
        ended_call(&mut state, &agent, &CallKind::Other);
        let after_one = take(&mut state);
        ended_call(&mut state, &agent, &copy);
        let after_both = std::iter::from_fn(|| take(&mut state))
            .map(|(agent, _)| agent)
            .collect::<std::collections::HashSet<_>>();

        assert!(after_one.is_none());
        assert_eq!(
            after_both,
            std::collections::HashSet::from([agent.clone(), target.clone()])
        );
        assert!(state.busy.is_empty());
    }

    /// The phase of the store call of a job in the table of [`replaceable`].
    #[derive(Clone, Copy, Debug)]
    enum Call {
        NotWaiting,
        AfterFailure,
        ForLateWrites,
    }

    /// Who holds the save mark of the agent in the table of [`replaceable`].
    #[derive(Clone, Copy, Debug)]
    enum Mark {
        Own,
        Other,
        Free,
    }

    /// Whether an admission replaces a job of `kind` whose run phase is `call`, which decided when
    /// `decided`, while `mark` holds the save mark.
    fn replaceable_case(kind: SnapshotKind, call: Call, decided: bool, mark: Mark) -> bool {
        let mut state = State::default();
        let agent = agent_snapshots("replaceable");
        let name = FilesystemSnapshotName::periodic();
        let id = admit_kind(
            &mut state,
            &agent,
            &name,
            kind,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .unwrap()
        .id;
        saving(&mut state, &agent, id);
        match call {
            Call::NotWaiting => {}
            Call::AfterFailure => step(&mut state, |state| super::run_failed(state, &agent, id)),
            Call::ForLateWrites => step(&mut state, |state| {
                run_waiting_for_late_writes(state, &agent, id)
            }),
        }
        if decided {
            decide(&mut state, &agent, id, JobDecision::SaveFailed);
        }
        match mark {
            Mark::Own => assert!(begin(&mut state, &agent, &CallKind::Save { job: id })),
            Mark::Other => assert!(begin(&mut state, &agent, &CallKind::Save { job: id + 1 })),
            Mark::Free => {}
        }
        is_replaceable(&state, &agent, id)
    }

    #[test]
    fn only_a_periodic_job_whose_live_call_waits_and_that_has_not_decided_is_replaceable() {
        let cases = [SnapshotKind::Periodic, SnapshotKind::Update]
            .into_iter()
            .flat_map(|kind| {
                [Call::NotWaiting, Call::AfterFailure, Call::ForLateWrites]
                    .into_iter()
                    .flat_map(move |call| {
                        [false, true].into_iter().flat_map(move |decided| {
                            [Mark::Own, Mark::Other, Mark::Free]
                                .into_iter()
                                .map(move |mark| (kind, call, decided, mark))
                        })
                    })
            })
            .map(|(kind, call, decided, mark)| {
                let replaceable = replaceable_case(kind, call, decided, mark);
                let expected = kind == SnapshotKind::Periodic
                    && matches!(call, Call::AfterFailure | Call::ForLateWrites)
                    && !decided
                    && matches!(mark, Mark::Own);
                (
                    format!("{kind:?} {call:?} decided={decided} {mark:?}"),
                    replaceable == expected,
                )
            })
            .filter(|(_, right)| !right)
            .map(|(case, _)| case)
            .collect::<Vec<_>>();

        assert_eq!(cases, Vec::<String>::new());
    }

    #[test]
    fn a_replacement_frees_the_save_mark_of_the_old_call_and_its_end_leaves_the_new_mark() {
        let mut state = State::default();
        let agent = agent_snapshots("replaced-tail");
        let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        saving(&mut state, &agent, old);
        assert!(begin(&mut state, &agent, &CallKind::Save { job: old }));
        step(&mut state, |state| {
            run_waiting_for_late_writes(state, &agent, old)
        });

        let new = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        let new_save = begin(&mut state, &agent, &CallKind::Save { job: new });
        let busy_with_both = busy(&state, &agent);
        let old_run_granted = granted(&mut state, &agent, old);
        ended_call(&mut state, &agent, &CallKind::Save { job: old });
        let after_the_tail = (
            state.save_running.get(&agent).copied(),
            busy(&state, &agent),
        );
        step(&mut state, |state| {
            run_waiting_for_late_writes(state, &agent, old)
        });
        saving(&mut state, &agent, new);
        step(&mut state, |state| {
            run_waiting_for_late_writes(state, &agent, new)
        });
        let new_replaceable_while_its_call_lives = is_replaceable(&state, &agent, new);
        ended_call(&mut state, &agent, &CallKind::Save { job: new });
        let new_replaceable_after_its_call = is_replaceable(&state, &agent, new);

        assert_eq!(
            (
                new_save,
                busy_with_both,
                old_run_granted,
                after_the_tail,
                new_replaceable_while_its_call_lives,
                new_replaceable_after_its_call,
            ),
            (true, 2, false, (Some(new), 1), true, false)
        );
    }

    #[test]
    fn a_second_save_of_an_agent_is_refused_while_the_first_runs() {
        let mut state = State::default();
        let agent = agent_snapshots("one-save");

        let first = begin(&mut state, &agent, &CallKind::Save { job: 0 });
        let second = begin(&mut state, &agent, &CallKind::Save { job: 0 });
        let other = begin(&mut state, &agent, &CallKind::Other);
        let running = save_running(&state, &agent);
        ended_call(&mut state, &agent, &CallKind::Save { job: 0 });
        let after = (save_running(&state, &agent), busy(&state, &agent));

        assert_eq!((first, second, other, running), (true, false, true, true));
        assert_eq!(after, (false, 1));
    }

    #[test]
    fn fork_ended_gives_the_stage_scope_only_for_an_attempt_that_never_published() {
        let source = agent_snapshots("source");
        let flight = (
            AgentId {
                component_id: ComponentId::new(),
                agent_id: "target".to_string(),
            },
            [7; 32],
        );
        let end_of = |publishing: bool, published: bool| {
            let mut state = State::default();
            let stage = agent_snapshots("stage");
            let began = step(&mut state, |state| fork_began(state, &source, &flight));
            let again = step(&mut state, |state| fork_began(state, &source, &flight));
            if publishing {
                step(&mut state, |state| fork_publishing(state, &flight));
            }
            let end = step(&mut state, |state| {
                fork_ended(
                    state,
                    &source,
                    &flight,
                    Some(&stage),
                    published.then_some(ForkOutcome::Published),
                )
            });
            (
                began,
                again,
                end,
                all_requested(&state, &stage),
                busy(&state, &source),
            )
        };

        assert_eq!(
            [
                end_of(false, false),
                end_of(true, false),
                end_of(true, true)
            ],
            [
                (
                    true,
                    false,
                    ForkEnd::StageDeleted { overflow: false },
                    true,
                    0
                ),
                (true, false, ForkEnd::StageLeaked, false, 0),
                (true, false, ForkEnd::Done, false, 0),
            ]
        );
    }

    #[test]
    fn the_end_of_a_fork_attempt_deletes_its_stage_only_when_nothing_can_publish_it() {
        let source = agent_snapshots("source");
        let flight = (
            AgentId {
                component_id: ComponentId::new(),
                agent_id: "target".to_string(),
            },
            [8; 32],
        );
        let end_of = |publishing: bool, stage: bool, outcome: Option<ForkOutcome>| {
            let mut state = State::default();
            let staged = agent_snapshots("stage");
            step(&mut state, |state| fork_began(state, &source, &flight));
            if publishing {
                step(&mut state, |state| fork_publishing(state, &flight));
            }
            let end = step(&mut state, |state| {
                fork_ended(state, &source, &flight, stage.then_some(&staged), outcome)
            });
            (end, all_requested(&state, &staged))
        };
        let deleted = (ForkEnd::StageDeleted { overflow: false }, true);

        assert_eq!(
            [
                end_of(false, false, None),
                end_of(false, true, None),
                end_of(false, true, Some(ForkOutcome::Lost)),
                end_of(false, true, Some(ForkOutcome::Published)),
                end_of(true, true, Some(ForkOutcome::Lost)),
                end_of(true, true, Some(ForkOutcome::Unknown)),
                end_of(true, true, None),
                end_of(true, true, Some(ForkOutcome::Published)),
            ],
            [
                (ForkEnd::Done, false),
                deleted,
                deleted,
                (ForkEnd::Done, false),
                deleted,
                (ForkEnd::StageLeaked, false),
                (ForkEnd::StageLeaked, false),
                (ForkEnd::Done, false),
            ]
        );
    }

    #[test]
    fn a_fork_of_a_source_whose_snapshots_are_being_deleted_is_refused() {
        let mut state = State::default();
        let source = agent_snapshots("source");
        let flight = (
            AgentId {
                component_id: ComponentId::new(),
                agent_id: "target".to_string(),
            },
            [1; 32],
        );

        delete_all_snapshots(&mut state, &source);
        let began = step(&mut state, |state| fork_began(state, &source, &flight));

        assert!(!began);
    }

    /// One transition of the rules, over agents and names chosen by index.
    #[derive(Clone, Debug)]
    enum Step {
        Admit(usize, usize),
        End(usize),
        RequestNames(usize, Vec<usize>),
        RequestAll(usize),
        Take,
        CleanupEnded(usize),
        BeginCall(usize, Option<usize>, bool),
        StoreCallEnded,
        ForkBegan(usize, usize),
        ForkPublishing,
        ForkEnded(usize, bool),
        RunGranted(usize),
        RunFailed(usize),
        EndReplaced(usize),
    }

    fn step_strategy() -> impl proptest::strategy::Strategy<Value = Step> {
        use proptest::prelude::*;
        let agent = 0usize..3;
        let name = 0usize..3;
        prop_oneof![
            (agent.clone(), name.clone()).prop_map(|(agent, name)| Step::Admit(agent, name)),
            agent.clone().prop_map(Step::End),
            (agent.clone(), proptest::collection::vec(name, 0..3))
                .prop_map(|(agent, names)| Step::RequestNames(agent, names)),
            agent.clone().prop_map(Step::RequestAll),
            Just(Step::Take),
            agent.clone().prop_map(Step::CleanupEnded),
            (
                agent.clone(),
                proptest::option::of(agent.clone()),
                any::<bool>()
            )
                .prop_map(|(agent, to, save)| Step::BeginCall(agent, to, save)),
            Just(Step::StoreCallEnded),
            (agent.clone(), agent.clone()).prop_map(|(from, stage)| Step::ForkBegan(from, stage)),
            Just(Step::ForkPublishing),
            (0usize..3, any::<bool>())
                .prop_map(|(index, published)| Step::ForkEnded(index, published)),
            agent.clone().prop_map(Step::RunGranted),
            agent.clone().prop_map(Step::RunFailed),
            (0usize..3).prop_map(Step::EndReplaced),
        ]
    }

    /// The agents that are ready, and the agents that are in the ready queue with `queued`.
    fn ready_and_queued(
        state: &State,
        agents: &[AgentSnapshots],
    ) -> (Vec<AgentSnapshots>, Vec<AgentSnapshots>) {
        let ready = agents
            .iter()
            .filter(|agent| is_ready(state, agent))
            .cloned()
            .collect();
        let queued = agents
            .iter()
            .filter(|agent| {
                state.ready.contains(agent)
                    && state
                        .cleanups
                        .get(agent)
                        .is_some_and(|cleanup| cleanup.queued)
            })
            .cloned()
            .collect();
        (ready, queued)
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

        #[test]
        fn every_ready_agent_is_queued_after_any_sequence_of_transitions(
            steps in proptest::collection::vec(step_strategy(), 1..40),
            pending_cleanups in 1usize..3,
            names_per_cleanup in 1usize..4,
        ) {
            let agents = (0..3)
                .map(|index| agent_snapshots(&format!("agent-{index}")))
                .collect::<Vec<_>>();
            let stages = (0..3)
                .map(|index| agent_snapshots(&format!("stage-{index}")))
                .collect::<Vec<_>>();
            let job_names = (0..3)
                .map(|_| FilesystemSnapshotName::periodic())
                .collect::<Vec<_>>();
            let names = job_names
                .iter()
                .map(|name| SnapshotName::new(name.as_str()).unwrap())
                .collect::<Vec<_>>();
            let mut state = limited(pending_cleanups, 4, names_per_cleanup);
            let mut jobs: HashMap<usize, JobId> = HashMap::new();
            let mut replaced: Vec<(usize, JobId)> = Vec::new();
            let mut calls: Vec<(AgentSnapshots, CallKind)> = Vec::new();
            let mut forks: Vec<(AgentSnapshots, Flight, AgentSnapshots, bool)> = Vec::new();
            let mut fork_counter = 0u8;
            let all = agents.iter().chain(stages.iter()).cloned().collect::<Vec<_>>();
            steps.into_iter().for_each(|next| {
                match next {
                    Step::Admit(agent, name) => {
                        let name = job_names[name].clone();
                        if let Ok(id) = try_admit(&mut state, &agents[agent], &name, true)
                            && let Some(old) = jobs.insert(agent, id)
                        {
                            replaced.push((agent, old));
                        }
                    }
                    Step::End(agent) => {
                        if let Some(id) = jobs.remove(&agent) {
                            end_job(&mut state, &agents[agent], id);
                        }
                    }
                    Step::RequestNames(agent, chosen) => {
                        let chosen = chosen.into_iter().map(|index| names[index].clone()).collect::<Vec<_>>();
                        request_names(&mut state, &agents[agent], &chosen);
                    }
                    Step::RequestAll(agent) => {
                        request_all(&mut state, &agents[agent]);
                    }
                    Step::Take => {
                        take(&mut state);
                    }
                    Step::CleanupEnded(agent) => {
                        ended_cleanup(&mut state, &agents[agent]);
                    }
                    Step::BeginCall(agent, to, save) => {
                        let kind = match (to, save) {
                            (Some(to), _) => CallKind::Copy { to: agents[to].clone() },
                            (None, true) => CallKind::Save { job: 0 },
                            (None, false) => CallKind::Other,
                        };
                        if begin(&mut state, &agents[agent], &kind) {
                            calls.push((agents[agent].clone(), kind));
                        }
                    }
                    Step::StoreCallEnded => {
                        if let Some((agent, kind)) = calls.pop() {
                            ended_call(&mut state, &agent, &kind);
                        }
                    }
                    Step::ForkBegan(from, stage) => {
                        fork_counter = fork_counter.wrapping_add(1);
                        let flight = (AgentId { component_id: ComponentId::new(), agent_id: "target".to_string() }, [fork_counter; 32]);
                        if step(&mut state, |state| fork_began(state, &agents[from], &flight)) {
                            forks.push((agents[from].clone(), flight, stages[stage].clone(), false));
                        }
                    }
                    Step::ForkPublishing => {
                        if let Some((_, flight, _, publishing)) = forks.last_mut() {
                            step(&mut state, |state| fork_publishing(state, flight));
                            *publishing = true;
                        }
                    }
                    Step::ForkEnded(index, published) => {
                        if !forks.is_empty() {
                            let (from, flight, stage, _) = forks.remove(index % forks.len());
                            step(&mut state, |state| fork_ended(state, &from, &flight, Some(&stage), published.then_some(ForkOutcome::Published)));
                        }
                    }
                    Step::RunGranted(agent) => {
                        if let Some(id) = jobs.get(&agent) {
                            granted(&mut state, &agents[agent], *id);
                        }
                    }
                    Step::RunFailed(agent) => {
                        if let Some(id) = jobs.get(&agent) {
                            failed_run(&mut state, &agents[agent], *id);
                        }
                    }
                    Step::EndReplaced(index) => {
                        if !replaced.is_empty() {
                            let (agent, id) = replaced.remove(index % replaced.len());
                            end_job(&mut state, &agents[agent], id);
                        }
                    }
                }
                let (ready, queued) = ready_and_queued(&state, &all);
                assert!(
                    ready.iter().all(|agent| queued.contains(agent)),
                    "ready {ready:?} queued {queued:?}"
                );
                assert!(state.pending_names_total <= 4);
            });
            // The queue gives `None` only when no agent is ready.
            let (ready_before, _) = ready_and_queued(&state, &all);
            let taken = take(&mut state);
            proptest::prop_assert!(taken.is_some() || ready_before.is_empty());
        }
    }

    #[test]
    fn a_refused_admission_carries_the_retention_stop_of_the_running_job() {
        let mut state = State::default();
        let agent = agent_snapshots("retention-stop");
        let name = FilesystemSnapshotName::periodic();
        let stop = CancellationToken::new();
        let retention_stop = stop.child_token();
        let admitted = admit(
            &mut state,
            &agent,
            &name,
            stop.clone(),
            retention_stop.clone(),
            true,
        );

        let refusal = try_admit(&mut state, &agent, &name, true).err();
        if let Some(running) = refusal
            .as_ref()
            .and_then(|refusal| refusal.running.as_ref())
        {
            running.retention_stop.cancel();
        }

        assert!(admitted.is_ok());
        assert_eq!(
            refusal.map(|refusal| refusal.skip),
            Some(SnapshotSkip::UploadInFlight)
        );
        assert!(retention_stop.is_cancelled());
        assert!(!stop.is_cancelled());
    }

    #[test]
    fn a_manual_update_asks_again_when_its_job_ended_or_waits_after_a_failed_run() {
        let mut state = State::default();
        let agent = agent_snapshots("agent");
        let first = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        granted(&mut state, &agent, first);
        let while_saving = update_may_ask_again(&state, &agent, first);
        failed_run(&mut state, &agent, first);
        let after_a_failed_run = update_may_ask_again(&state, &agent, first);
        end_job(&mut state, &agent, first);
        let after_the_end = update_may_ask_again(&state, &agent, first);

        assert_eq!(
            (while_saving, after_a_failed_run, after_the_end),
            (false, true, true)
        );
    }

    /// The state after an admission of a periodic job of `agent` whose run failed, and the next
    /// state that a replacing admission gives.
    fn replacing(
        agent: &AgentSnapshots,
        name: &FilesystemSnapshotName,
    ) -> Next<Result<Admitted, Refusal>> {
        let mut state = State::default();
        let old = admitted(&mut state, agent, name);
        failed_run(&mut state, agent, old);
        super::admit(
            state,
            agent,
            name,
            SnapshotKind::Periodic,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
    }

    /// Admits a job of `kind` with new stops, and gives its id and the stop of the job that it
    /// replaced.
    fn admit_new(
        state: &mut State,
        agent: &AgentSnapshots,
        kind: SnapshotKind,
    ) -> Result<(JobId, Option<CancellationToken>), Option<JobId>> {
        admit_kind(
            state,
            agent,
            &FilesystemSnapshotName::periodic(),
            kind,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .map(|admitted| (admitted.id, admitted.replaced))
        .map_err(|refusal| refusal.running.map(|running| running.id))
    }

    #[test]
    fn a_periodic_admission_replaces_a_periodic_job_that_waits_after_a_failure() {
        let mut state = State::default();
        let agent = agent_snapshots("replaced");
        let stop = CancellationToken::new();
        let old = admit(
            &mut state,
            &agent,
            &FilesystemSnapshotName::periodic(),
            stop.clone(),
            stop.child_token(),
            true,
        )
        .unwrap();
        saving(&mut state, &agent, old);
        failed_run(&mut state, &agent, old);

        let new = admit_new(&mut state, &agent, SnapshotKind::Periodic);
        let update = admit_new(&mut state, &agent, SnapshotKind::Update);

        let (new_id, replaced) = new.unwrap();
        assert!(new_id > old);
        assert!(replaced.is_some_and(|replaced| !replaced.is_cancelled()));
        assert!(!stop.is_cancelled());
        assert_eq!(update.err(), Some(Some(new_id)));
        assert!(has_ended(&state, &agent, old));
    }

    #[test]
    fn a_replacement_records_replaced_and_gives_the_old_stop_without_cancelling_it() {
        let mut state = State::default();
        let agent = agent_snapshots("replaced-decision");
        let name = FilesystemSnapshotName::periodic();
        let stop = CancellationToken::new();
        let old = admit(
            &mut state,
            &agent,
            &name,
            stop.clone(),
            stop.child_token(),
            true,
        )
        .unwrap();
        saving(&mut state, &agent, old);
        let waiting = wait(&mut state, &agent, &name);
        failed_run(&mut state, &agent, old);

        let replaced = admit_new(&mut state, &agent, SnapshotKind::Periodic)
            .unwrap()
            .1;

        assert_eq!(waiting, Ok(old));
        assert_eq!(
            decision_of(&state, &agent, old),
            Some(JobDecision::Replaced)
        );
        assert!(replaced.is_some_and(|replaced| {
            let same = !replaced.is_cancelled();
            replaced.cancel();
            same && stop.is_cancelled()
        }));
    }

    #[test]
    fn an_admission_refuses_a_job_that_is_not_waiting_after_a_failure_or_is_decided() {
        let agent = agent_snapshots("not-replaced");
        let not_waiting = {
            let mut state = State::default();
            let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
            saving(&mut state, &agent, old);
            admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
        };
        let granted_again = {
            let mut state = State::default();
            let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
            failed_run(&mut state, &agent, old);
            saving(&mut state, &agent, old);
            admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
        };
        let decided = {
            let mut state = State::default();
            let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
            failed_run(&mut state, &agent, old);
            decide(&mut state, &agent, old, JobDecision::SaveFailed);
            admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
        };

        assert_eq!(
            [not_waiting, granted_again, decided],
            [Some(Some(1)), Some(Some(1)), Some(Some(1))]
        );
    }

    #[test]
    fn an_update_job_is_never_replaced() {
        let mut state = State::default();
        let agent = agent_snapshots("update-kept");
        let (old, _) = admit_new(&mut state, &agent, SnapshotKind::Update).unwrap();
        failed_run(&mut state, &agent, old);

        assert_eq!(
            [
                admit_new(&mut state, &agent, SnapshotKind::Periodic).err(),
                admit_new(&mut state, &agent, SnapshotKind::Update).err(),
            ],
            [Some(Some(old)), Some(Some(old))]
        );
    }

    #[test]
    fn the_grant_of_a_replaced_job_is_refused() {
        let mut state = State::default();
        let agent = agent_snapshots("stale-grant");
        let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        failed_run(&mut state, &agent, old);
        let (new, _) = admit_new(&mut state, &agent, SnapshotKind::Periodic).unwrap();

        assert_eq!(
            [
                granted(&mut state, &agent, old),
                granted(&mut state, &agent, new)
            ],
            [false, true]
        );
    }

    #[test]
    fn a_stale_run_report_of_a_replaced_job_changes_nothing() {
        let mut state = State::default();
        let agent = agent_snapshots("stale-report");
        let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        failed_run(&mut state, &agent, old);
        let (new, _) = admit_new(&mut state, &agent, SnapshotKind::Periodic).unwrap();
        saving(&mut state, &agent, new);

        failed_run(&mut state, &agent, old);

        assert_eq!(
            (
                is_replaceable(&state, &agent, new),
                admit_new(&mut state, &agent, SnapshotKind::Periodic).err()
            ),
            (false, Some(Some(new)))
        );
    }

    #[test]
    fn no_replacement_while_a_delete_of_all_snapshots_is_pending() {
        let mut state = State::default();
        let agent = agent_snapshots("no-replacement-while-deleting");
        let old = admitted(&mut state, &agent, &FilesystemSnapshotName::periodic());
        failed_run(&mut state, &agent, old);
        delete_all_snapshots(&mut state, &agent);

        let refusal = admit_kind(
            &mut state,
            &agent,
            &FilesystemSnapshotName::periodic(),
            SnapshotKind::Periodic,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .err()
        .map(|refusal| refusal.skip);

        assert_eq!(refusal, Some(SnapshotSkip::DeletingAllSnapshots));
        assert!(!has_ended(&state, &agent, old));
    }

    /// One step of the property of the replacement.
    #[derive(Clone, Debug)]
    enum ReplaceStep {
        Admit(usize, bool),
        Granted(usize, u64),
        Failed(usize, u64),
        Decide(usize, u64),
        End(usize, u64),
    }

    fn replace_step_strategy() -> impl proptest::strategy::Strategy<Value = ReplaceStep> {
        use proptest::prelude::*;
        prop_oneof![
            (0..2usize, any::<bool>())
                .prop_map(|(agent, periodic)| ReplaceStep::Admit(agent, periodic)),
            (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::Granted(agent, id)),
            (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::Failed(agent, id)),
            (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::Decide(agent, id)),
            (0..2usize, 1..8u64).prop_map(|(agent, id)| ReplaceStep::End(agent, id)),
        ]
    }

    proptest::proptest! {
        #[test]
        fn replacements_keep_one_live_job_and_refuse_stale_reports(
            steps in proptest::collection::vec(replace_step_strategy(), 1..40)
        ) {
            let agents = [agent_snapshots("one"), agent_snapshots("two")];
            let mut state = State::default();
            let mut replaced = HashSet::new();
            // A manual update waits for the job of an agent until the job is gone or an admission
            // can replace it. Each transition that makes that true for a job must wake the waiters.
            let ready_for_an_update = |state: &State, agent: &AgentSnapshots, id: JobId| {
                has_ended(state, agent, id) || is_replaceable(state, agent, id)
            };
            steps.into_iter().for_each(|step| {
                let waited_on = agents
                    .iter()
                    .filter_map(|agent| {
                        state
                            .jobs
                            .get(agent)
                            .map(|job| (agent.clone(), job.id))
                            .filter(|(agent, id)| !ready_for_an_update(&state, agent, *id))
                    })
                    .collect::<Vec<_>>();
                let wakes = match step {
                    ReplaceStep::Admit(agent, periodic) => {
                        let kind = if periodic { SnapshotKind::Periodic } else { SnapshotKind::Update };
                        let before = state.jobs.get(&agents[agent]).map(|job| job.id);
                        let next = super::admit(
                            std::mem::take(&mut state),
                            &agents[agent],
                            &FilesystemSnapshotName::periodic(),
                            kind,
                            CancellationToken::new(),
                            CancellationToken::new(),
                            true,
                        );
                        let wakes = next.wakes();
                        let (next, answer) = next.into_parts();
                        state = next;
                        if let (Ok(admitted), Some(before)) = (&answer, before) {
                            assert!(admitted.replaced.is_some());
                            replaced.insert(before);
                        }
                        wakes
                    }
                    ReplaceStep::Granted(agent, id) => {
                        let live = state.jobs.get(&agents[agent]).is_some_and(|job| job.id == id);
                        let next = super::run_granted(std::mem::take(&mut state), &agents[agent], id);
                        let wakes = next.wakes();
                        let (next, granted) = next.into_parts();
                        state = next;
                        assert_eq!(granted, live);
                        assert!(!(granted && replaced.contains(&id)));
                        wakes
                    }
                    ReplaceStep::Failed(agent, id) => {
                        let before = state.jobs.get(&agents[agent]).map(|job| (job.id, job.run_phase));
                        let next = super::run_failed(std::mem::take(&mut state), &agents[agent], id);
                        let wakes = next.wakes();
                        state = next.into_parts().0;
                        let after = state.jobs.get(&agents[agent]).map(|job| (job.id, job.run_phase));
                        if before.is_none_or(|(live, _)| live != id) {
                            assert_eq!(before, after);
                        }
                        wakes
                    }
                    ReplaceStep::Decide(agent, id) => {
                        let next = super::decide(
                            std::mem::take(&mut state),
                            &agents[agent],
                            id,
                            JobDecision::SaveFailed,
                        );
                        let wakes = next.wakes();
                        state = next.into_parts().0;
                        wakes
                    }
                    ReplaceStep::End(agent, id) => {
                        let next = end(std::mem::take(&mut state), &agents[agent], id);
                        let wakes = next.wakes();
                        state = next.into_parts().0;
                        wakes
                    }
                };
                // After each step: no replaced job is live, and a waiter whose job became ready
                // for an update was woken.
                agents.iter().for_each(|agent| {
                    assert!(state.jobs.get(agent).is_none_or(|job| !replaced.contains(&job.id)));
                });
                waited_on.iter().for_each(|(agent, id)| {
                    assert!(
                        !ready_for_an_update(&state, agent, *id) || wakes,
                        "the job {id} became ready for an update without a wake"
                    );
                });
            });
        }
    }
}
